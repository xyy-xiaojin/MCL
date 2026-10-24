//! 樱花 FRP（SakuraFrp）：把本地端口挂到服务商的公网节点上。
//!
//! 这条路是**兜底**：不走打洞、不看 NAT 脸色，代价是流量过中继、有带宽上限。
//! 前两条（EasyTier / 端口映射）都不通的时候再用它。
//!
//! 三步：
//! 1. 拿访问密钥调 `/v4/tunnels` 列出账号下的隧道；
//! 2. 对选中的隧道调 `/v4/tunnel/config`，服务端直接返回能被 frpc 吃的配置文件
//!    （省得我们自己拼 `server_addr` / `sakura_mode` 这些容易写错的字段）；
//! 3. 改掉配置里的 `local_port` 再启动 frpc——隧道是建的时候填的端口，
//!    而玩家每次开世界的端口可能不同。
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::{lock, publish, tool, Channel, Fact, Slot};

const API: &str = "https://api.natfrp.com/v4";
/// 拉配置时声明要哪个版本的 frpc，必须和我们内置的那份对上。
pub const FRPC_VERSION: &str = "0.51.0-sakura-15";

/// 账号下的一条隧道（只保留界面要用的字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub id: i64,
    pub name: String,
    /// `tcp` / `udp` / `http` …
    pub kind: String,
    pub node: i64,
    pub online: bool,
    /// 被封禁的隧道不给选
    pub banned: bool,
    pub local_port: u16,
    /// 公网地址，形如 `frp-act.top:50887`
    pub remote: String,
    pub note: String,
}

/// 落盘的凭据：访问密钥 + 上次选的隧道。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Credentials {
    pub token: String,
    pub last_tunnel: Option<i64>,
}

impl Credentials {
    fn file(app: &AppHandle) -> crate::error::Result<PathBuf> {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        let dir = state.paths.netplay();
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join("frp.json"))
    }

    pub fn load(app: &AppHandle) -> Self {
        Self::file(app)
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, app: &AppHandle) -> crate::error::Result<()> {
        let path = Self::file(app)?;
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// 记住访问密钥。顺便立刻验一次：存下来却用不了比不存更让人困惑。
pub async fn save_token(app: AppHandle, token: String) -> crate::error::Result<Vec<Tunnel>> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(crate::error::Error::msg("访问密钥不能为空"));
    }
    let tunnels = list_tunnels(&app, &token).await?;
    let mut creds = Credentials::load(&app);
    creds.token = token;
    creds.save(&app)?;
    Ok(tunnels)
}

/// 拉隧道列表。
pub async fn list_tunnels(app: &AppHandle, token: &str) -> crate::error::Result<Vec<Tunnel>> {
    let client = client(app)?;
    let resp = client
        .get(format!("{API}/tunnels"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| crate::error::Error::msg(format!("连不上樱花接口：{e}")))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(crate::error::Error::msg(explain(status.as_u16(), &text)));
    }

    #[derive(Deserialize)]
    struct Raw {
        id: i64,
        name: String,
        #[serde(rename = "type")]
        kind: String,
        node: i64,
        #[serde(default)]
        online: bool,
        #[serde(default)]
        status: i64,
        #[serde(default)]
        local_port: i64,
        #[serde(default)]
        remote: String,
        #[serde(default)]
        note: String,
    }

    let raw: Vec<Raw> = serde_json::from_str(&text)
        .map_err(|e| crate::error::Error::msg(format!("樱花返回的数据看不懂：{e}")))?;

    Ok(raw
        .into_iter()
        .map(|t| Tunnel {
            id: t.id,
            name: t.name,
            kind: t.kind,
            node: t.node,
            online: t.online,
            banned: t.status == 2,
            local_port: t.local_port.clamp(0, 65535) as u16,
            remote: t.remote,
            note: t.note,
        })
        .collect())
}

/// 起隧道。
pub async fn start(app: AppHandle, tunnel_id: i64, local_port: u16) -> crate::error::Result<Slot> {
    if local_port == 0 {
        return Err(crate::error::Error::msg("本地端口不能是 0"));
    }
    let creds = Credentials::load(&app);
    if creds.token.is_empty() {
        return Err(crate::error::Error::msg("还没有填访问密钥"));
    }

    let frpc = tool(&app, "frpc.exe")?;
    let workdir = super::tool_dir(&app);

    // 把所有通道（包括自己这条）停干净：重复点「启动」时旧的 frpc 还在跑，
    // 不杀掉的话会变成孤儿进程继续吃流量
    {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        state.netplay.stop_all();
    }

    publish(&app, Channel::Frp, Slot::starting(local_port, "正在拉取隧道配置…"));

    // 服务端现算的配置，比本机拼的可靠。
    // 之后任何一步失败都要把 slot 落成失败状态：不 publish 的话界面
    // 会永远停在「正在拉取隧道配置…」，只能重启 MCL 才能再启动。
    let prepared = async {
        let tunnels = list_tunnels(&app, &creds.token).await?;
        let tunnel = tunnels
            .iter()
            .find(|t| t.id == tunnel_id)
            .cloned()
            .ok_or_else(|| crate::error::Error::msg("账号下找不到这条隧道，可能已被删除"))?;
        if tunnel.banned {
            return Err(crate::error::Error::msg("这条隧道被封禁了，换个隧道"));
        }

        let config = fetch_config(&app, &creds.token, tunnel_id).await?;
        let patched = retarget(&config, local_port);

        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        let config_path = state.paths.netplay().join(format!("frpc-{tunnel_id}.toml"));
        std::fs::create_dir_all(state.paths.netplay())?;
        std::fs::write(&config_path, &patched)?;
        Ok((tunnel, config_path))
    }
    .await;

    let (tunnel, config_path) = match prepared {
        Ok(pair) => pair,
        Err(e) => {
            publish(&app, Channel::Frp, Slot::fail(local_port, e.to_string()));
            return Err(e);
        }
    };

    let args = vec![
        "-c".to_string(),
        config_path.display().to_string(),
    ];
    let child = super::spawn(&app, Channel::Frp, &frpc, &args, &workdir);
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            publish(&app, Channel::Frp, Slot::fail(local_port, e.to_string()));
            return Err(e);
        }
    };
    {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        let mut rt = lock(state.netplay.runtime(Channel::Frp));
        // 与 easytier 同理：先杀掉残留的旧 frpc 再覆盖
        if let Some(mut old) = rt.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        rt.child = Some(child);
    }
    super::watch(app.clone(), Channel::Frp);

    // frpc 连节点 → 注册隧道，一般 1~3 秒。给它 8 秒，并用公网地址探测
    // 确认隧道真的注册上了，连不上就如实报错
    let ok = wait_connected(&app, &tunnel.remote, 8).await;
    let mut creds = Credentials::load(&app);
    creds.last_tunnel = Some(tunnel_id);
    let _ = creds.save(&app);

    let mut slot = Slot {
        state: if ok { "running".into() } else { "error".into() },
        summary: if ok {
            format!("隧道「{}」已连接", tunnel.name)
        } else {
            "隧道没连上".to_string()
        },
        address: tunnel.remote.clone(),
        local_port,
        error: if ok {
            String::new()
        } else {
            "frpc 8 秒内没有连上节点，看下面的日志（常见原因：节点不通、流量用完、隧道被占）"
                .to_string()
        },
        facts: vec![
            Fact::new("隧道", format!("{}（#{}）", tunnel.name, tunnel.id)),
            Fact::new("类型", tunnel.kind.to_uppercase()),
            Fact::new("本地端口", local_port.to_string()),
            Fact::new("节点 ID", tunnel.node.to_string()),
        ],
    };
    if !tunnel.note.trim().is_empty() {
        slot.facts.push(Fact::new("备注", tunnel.note.clone()));
    }
    publish(&app, Channel::Frp, slot.clone());
    Ok(slot)
}

/// 向服务端要配置文件。
async fn fetch_config(
    app: &AppHandle,
    token: &str,
    tunnel_id: i64,
) -> crate::error::Result<String> {
    let client = client(app)?;
    let resp = client
        .post(format!("{API}/tunnel/config"))
        .bearer_auth(token)
        .form(&[
            ("query", tunnel_id.to_string()),
            ("frpc", FRPC_VERSION.to_string()),
        ])
        .send()
        .await
        .map_err(|e| crate::error::Error::msg(format!("拉取隧道配置失败：{e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(crate::error::Error::msg(explain(status.as_u16(), &text)));
    }
    if text.trim().is_empty() {
        return Err(crate::error::Error::msg("服务端返回了空配置"));
    }
    Ok(text)
}

/// 把配置里的 `local_port` 换成玩家实际的端口。
///
/// 只动 `proxies` 段里的那一行：`[common]` 段没有 local_port，
/// 但万一以后有了，也不该被我们改掉。
fn retarget(config: &str, local_port: u16) -> String {
    let mut in_proxy = false;
    let mut out = String::with_capacity(config.len() + 8);
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[proxies]]") || trimmed.starts_with("[proxies") {
            in_proxy = true;
        } else if trimmed.starts_with('[') {
            in_proxy = false;
        }
        let is_local_port = trimmed
            .split('=')
            .next()
            .map(|k| k.trim() == "local_port" || k.trim() == "localPort")
            .unwrap_or(false);
        if in_proxy && is_local_port {
            out.push_str(&format!("local_port = {local_port}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 等 frpc 真正连上节点。
///
/// 只看「进程还活着」是不够的：密钥错误、节点不通、流量用完时 frpc 会一直
/// 重试而不退出，8 秒后照样"已连接"。所以这里改为探测隧道在节点侧的公网
/// 地址——节点只在隧道注册成功后才接受这个端口上的 TCP 连接，连得上才算通。
async fn wait_connected(app: &AppHandle, remote: &str, seconds: u64) -> bool {
    for _ in 0..(seconds * 2) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // 进程死了就不用探了，watch 会把状态标成 error
        let alive = app
            .try_state::<crate::state::AppState>()
            .map(|s| {
                let mut rt = lock(s.netplay.runtime(Channel::Frp));
                match rt.child.as_mut() {
                    Some(child) => !matches!(child.try_wait(), Ok(Some(_))),
                    None => false,
                }
            })
            .unwrap_or(false);
        if !alive {
            return false;
        }
        if super::remote_open(remote).await {
            return true;
        }
    }
    false
}

fn client(app: &AppHandle) -> crate::error::Result<reqwest::Client> {
    app.try_state::<crate::state::AppState>()
        .map(|s| s.client.clone())
        .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))
}

/// 把 HTTP 状态码翻译成能直接给玩家看的话。
fn explain(status: u16, body: &str) -> String {
    let hint = match status {
        401 | 403 => "访问密钥不对，或者这个密钥没有该权限",
        404 => "接口地址变了，MCL 可能该更新了",
        429 => "请求太频繁，等一会儿再试",
        500..=599 => "樱花服务器出问题了，稍后再试",
        _ => "请求失败",
    };
    let snippet: String = body.chars().take(160).collect();
    if snippet.trim().is_empty() {
        format!("{hint}（HTTP {status}）")
    } else {
        format!("{hint}（HTTP {status}：{snippet}）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[common]
server_addr = "frp-act.top"
server_port = 7000
user = "abc"

[[proxies]]
name = "mc"
type = "tcp"
local_ip = "127.0.0.1"
local_port = 25565
remote_port = 50887
"#;

    #[test]
    fn retarget_only_touches_proxy_local_port() {
        let out = retarget(SAMPLE, 30000);
        assert!(out.contains("local_port = 30000"));
        assert!(!out.contains("local_port = 25565"));
        // 端口以外的东西一个都不能动
        assert!(out.contains("remote_port = 50887"));
        assert!(out.contains("server_port = 7000"));
        assert!(out.contains("[[proxies]]"));
    }

    #[test]
    fn retarget_keeps_common_section_intact() {
        let config = "[common]\nlocal_port = 1\n";
        let out = retarget(config, 2);
        assert!(out.contains("local_port = 1"));
    }

    #[test]
    fn explain_mentions_status() {
        assert!(explain(401, "").contains("访问密钥"));
        assert!(explain(503, "").contains("503"));
    }
}
