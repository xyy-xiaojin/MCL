//! EasyTier 组网：把两台机器放进同一个虚拟局域网，MC 直接连虚拟 IP。
//!
//! 这条通路在三条里**最稳**：只要双方的 EasyTier 都能连上任意一个公共节点，
//! 之后就是 P2P 直连（打不通才走中继），NAT 类型基本不影响。
//!
//! 房主固定拿 `10.144.1.1`，加入者由 EasyTier 自己分配 `10.144.1.x`。
//! 朋友在 MC 里填的就是 `10.144.1.1:<端口>`——这个地址不会变，
//! 不像端口映射那样每次重启都换一个公网端口。

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::{lock, publish, tool, Channel, Fact, Slot};

/// 房间使用的虚拟网段。
///
/// 选 `10.144.1.0/24` 而不是 `192.168.x`：家用路由器几乎不会用 10.144 段，
/// 撞上用户现有局域网的概率最低。房主永远是 `.1`，朋友不用记变化的值。
pub const SUBNET: &str = "10.144.1.0/24";
pub const HOST_IP: &str = "10.144.1.1/24";

/// 默认种子节点。EasyTier 官方的公共 rendezvous，用来让两端互相发现。
pub const DEFAULT_SEED: &str = "tcp://public.easytier.top:11010";
/// 备用种子。主节点抽风时换一个还能连上。
pub const FALLBACK_SEED: &str = "tcp://public2.easytier.top:11010";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomRequest {
    /// 房间名，两端必须一模一样
    pub room: String,
    /// 房间密码，两端必须一模一样
    pub password: String,
    /// 自己是不是房主（房主固定 .1）
    pub host: bool,
    /// 游戏本地端口，只用于提示文案
    pub local_port: u16,
}

/// 起 EasyTier 并把虚拟 IP 探出来。
pub async fn start(app: AppHandle, req: RoomRequest) -> crate::error::Result<Slot> {
    let room = req.room.trim().to_string();
    if room.is_empty() {
        return Err(crate::error::Error::msg("房间名不能为空"));
    }
    if room.chars().count() > 32 {
        return Err(crate::error::Error::msg("房间名最多 32 个字符"));
    }
    if req.password.trim().len() < 4 {
        return Err(crate::error::Error::msg("房间密码至少 4 位，太短容易被撞"));
    }
    // 提前拦：没管理员权限时 core 会在建网卡那一步静默退出，
    // 等它报错不如现在就告诉玩家怎么办
    if !super::is_admin() {
        return Err(crate::error::Error::msg(
            "EasyTier 要装虚拟网卡，必须用管理员权限运行。点左下角的「以管理员身份重启」即可。",
        ));
    }

    let core = tool(&app, "easytier-core.exe")?;
    let cli = tool(&app, "easytier-cli.exe")?;
    let workdir = super::tool_dir(&app);

    // 把所有通道（包括自己这条）停干净：重复点「启动」时，旧 core 还占着
    // TUN 网卡和 11010 端口，不杀掉的话新 core 大概率绑不上端口直接失败
    {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        state.netplay.stop_all();
    }

    publish(
        &app,
        Channel::Easytier,
        Slot::starting(
            req.local_port,
            if req.host {
                "正在创建房间…"
            } else {
                "正在加入房间…"
            },
        ),
    );

    let args = build_args(&room, req.password.trim(), req.host, req.local_port);
    let child = super::spawn(&app, Channel::Easytier, &core, &args, &workdir);
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            // 不把界面留在「正在创建房间…」：起不来就把状态落成失败
            publish(&app, Channel::Easytier, Slot::fail(req.local_port, e.to_string()));
            return Err(e);
        }
    };
    {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        let mut rt = lock(state.netplay.runtime(Channel::Easytier));
        // 上一次启动的进程还挂在 slot 上（两次启动贴得极近时 stop_all 之后
        // 又可能塞进来一个新的）：先杀再覆盖，绝不能让它变成孤儿进程
        if let Some(mut old) = rt.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        rt.child = Some(child);
    }
    super::watch(app.clone(), Channel::Easytier);

    // 虚拟网卡起来 + 握手需要几秒；轮询 CLI 比死等固定时间靠谱
    let (vpn_ip, nat_hint) = wait_for_ip(&cli, &workdir).await;

    // 等待期间进程退了（权限被撤、被杀、自己崩了）：watch 已经把状态标成
    // error，这里绝不能再 publish 把它盖回 running，否则界面上是个僵尸房间
    let alive = app
        .try_state::<crate::state::AppState>()
        .map(|s| {
            let mut rt = lock(s.netplay.runtime(Channel::Easytier));
            match rt.child.as_mut() {
                Some(child) => matches!(child.try_wait(), Ok(None)),
                None => false,
            }
        })
        .unwrap_or(false);
    if !alive {
        return Err(crate::error::Error::msg(
            "EasyTier 进程已退出，看下面日志找原因（最常见：需要管理员权限）",
        ));
    }

    let mut slot = Slot {
        state: "running".to_string(),
        summary: if req.host {
            "房间已创建，等朋友加入".to_string()
        } else {
            "已加入房间".to_string()
        },
        address: String::new(),
        local_port: req.local_port,
        error: String::new(),
        facts: Vec::new(),
    };

    match vpn_ip {
        Some(ip) => {
            slot.facts.push(Fact::new("我的虚拟 IP", ip));
            if req.host {
                slot.address = format!("{HOST_IP_ADDR}:{}", req.local_port);
                slot.facts.push(Fact::new("房间网段", SUBNET));
                slot.facts
                    .push(Fact::new("朋友填这个", slot.address.clone()));
            } else {
                slot.facts
                    .push(Fact::new("房主地址", format!("{HOST_IP_ADDR}:{}", req.local_port)));
            }
        }
        None => {
            // 网卡没起来也别把人劝退：进程还在，可能只是慢
            slot.summary = "已启动，等待虚拟网卡就绪".to_string();
            slot.facts
                .push(Fact::new("提示", "网卡尚未就绪，稍等片刻或查看日志"));
            if req.host {
                slot.address = format!("{HOST_IP_ADDR}:{}", req.local_port);
            }
        }
    }

    slot.facts.push(Fact::new("房间名", room));
    slot.facts.push(Fact::new("种子节点", host_of(DEFAULT_SEED)));
    if let Some(hint) = nat_hint {
        slot.facts.push(Fact::new("公网出口 / NAT", hint));
    }
    publish(&app, Channel::Easytier, slot.clone());
    Ok(slot)
}

/// 房主虚拟地址（去掉掩码）。
const HOST_IP_ADDR: &str = "10.144.1.1";
/// 本机 RPC 端口：`easytier-cli` 靠它和 core 通信，必须两边一致。
const RPC_PORTAL: &str = "127.0.0.1:15888";

/// 拼 EasyTier 命令行。
///
/// **两端都必须带 `-i`**：EasyTier 的规则是"不给 IP 就只当转发节点、不建 TUN 设备"，
/// 而 MC 要连虚拟 IP 必须有那块网卡。所以房主固定 `.1`，加入者随机挑一个 `.x`。
fn build_args(room: &str, password: &str, host: bool, local_port: u16) -> Vec<String> {
    let args = vec![
        "--network-name".to_string(),
        room.to_string(),
        "--network-secret".to_string(),
        password.to_string(),
        "--hostname".to_string(),
        // 主机名能让对端列表一眼看出是谁开的游戏
        if host {
            format!("mcl-host-{local_port}")
        } else {
            "mcl-join".to_string()
        },
        "--listeners".to_string(),
        "tcp://0.0.0.0:11010".to_string(),
        "--rpc-portal".to_string(),
        RPC_PORTAL.to_string(),
        // 两端都去连种子；碰面之后 EasyTier 自己升级成 P2P 直连
        "-p".to_string(),
        DEFAULT_SEED.to_string(),
        "-p".to_string(),
        FALLBACK_SEED.to_string(),
        "-i".to_string(),
        if host { HOST_IP.to_string() } else { joiner_ip() },
    ];
    args
}

/// 加入者从 `10.144.1.2` ~ `10.144.1.251` 里挑一个。
///
/// 不用 `-d`（DHCP）：DHCP 的默认网段是 `10.0.0.0/24`，和房主的 `10.144.1.x`
/// 根本不在一个网里，拿到的地址连不通。真正的 IP 冲突由 EasyTier 自己处理
/// （重复的会被踢），换个房号重开即可。
fn joiner_ip() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let octet = (nanos % 250) + 2;
    format!("10.144.1.{octet}/24")
}

/// 轮询 `easytier-cli -o json node`，直到能读出本机虚拟 IP。
///
/// 顺便把 EasyTier 自己的 STUN 结论带回来——它已经替我们问过公网了，
/// 再单独做一次探测纯属浪费。
async fn wait_for_ip(cli: &std::path::Path, cwd: &std::path::Path) -> (Option<String>, Option<String>) {
    let mut nat_hint = None;
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let cli = cli.to_path_buf();
        let cwd = cwd.to_path_buf();
        let out = tokio::task::spawn_blocking(move || {
            let mut cmd = std::process::Command::new(&cli);
            // JSON 输出比表格稳定，表格的列宽会随内容变化
            cmd.args(["-o", "json", "node"]).current_dir(&cwd);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            cmd.output().ok()
        })
        .await
        .ok()
        .flatten();
        let Some(out) = out else { continue };

        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if nat_hint.is_none() {
            nat_hint = parse_nat_hint(&text);
        }
        if let Some(ip) = parse_virtual_ip(&text) {
            return (Some(ip), nat_hint);
        }
    }
    (None, nat_hint)
}

/// 从 `easytier-cli -o json node` 里抠出本机虚拟 IP。
///
/// 实测输出（v2.6.4）：
/// ```json
/// { "peer_id": 4013277895, "ipv4_addr": "10.144.1.1/24",
///   "stun_info": { "udp_nat_type": 5, "public_ip": ["221.229.123.146"] } }
/// ```
/// **字段名是 `ipv4_addr`**，不是 `ipv4`；`--no-tun` 或没给 `-i` 时它是空串，
/// 那正好是"没有网卡"的信号，不能当成成功。
fn parse_virtual_ip(text: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(ip) = value.get("ipv4_addr").and_then(|v| v.as_str()) {
            let ip = ip.trim();
            if !ip.is_empty() {
                return Some(ip.split('/').next().unwrap_or(ip).to_string());
            }
        }
    }
    // 老版本 / 表格模式的兜底
    for line in text.lines() {
        let lower = line.to_lowercase();
        if !lower.contains("ipv4") {
            continue;
        }
        for token in line.split(['|', ' ', '\t', ':', ',']).filter(|t| !t.is_empty()) {
            let ip = token.trim().trim_matches(|c: char| "\"'[]{}".contains(c));
            if ip.starts_with("10.144.1.") || ip.starts_with("10.126.") {
                return Some(ip.split('/').next().unwrap_or(ip).to_string());
            }
        }
    }
    None
}

/// EasyTier 自己做的 STUN 探测结果，顺手拿来给玩家看网络状况。
fn parse_nat_hint(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let stun = value.get("stun_info")?;
    let public = stun
        .get("public_ip")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let udp_type = stun.get("udp_nat_type").and_then(|v| v.as_i64()).unwrap_or(0);
    if public.is_empty() {
        return None;
    }
    let label = match udp_type {
        1 => "全锥形",
        2 => "受限锥形",
        3 => "端口受限",
        4 | 5 => "对称型",
        _ => "未知",
    };
    Some(format!("{public}（{label}）"))
}

/// `tcp://public.easytier.top:11010` → `public.easytier.top:11010`
fn host_of(seed: &str) -> String {
    seed.split("://").nth(1).unwrap_or(seed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_args_pin_ipv4() {
        let args = build_args("mc", "1234", true, 25565);
        assert!(args.iter().any(|a| a == HOST_IP));
        assert!(args.iter().any(|a| a == "--network-name"));
    }

    #[test]
    fn joiner_also_gets_a_tun_address() {
        // 不给 -i 的话 EasyTier 只转发不建网卡，MC 就连不上虚拟 IP
        let args = build_args("mc", "1234", false, 25565);
        let idx = args.iter().position(|a| a == "-i").expect("加入者也必须有 -i");
        assert!(args[idx + 1].starts_with("10.144.1."));
        assert_ne!(args[idx + 1], HOST_IP);
    }

    #[test]
    fn parses_virtual_ip() {
        let table = "| ipv4 | 10.144.1.1/24 |\n";
        assert_eq!(parse_virtual_ip(table), Some("10.144.1.1".into()));

        let json = r#"{"node_info":{"ipv4":"10.144.1.7/24"}}"#;
        assert_eq!(parse_virtual_ip(json), Some("10.144.1.7".into()));

        assert_eq!(parse_virtual_ip("nothing here"), None);
    }

    #[test]
    fn seed_host_without_scheme() {
        assert_eq!(host_of(DEFAULT_SEED), "public.easytier.top:11010");
    }
}
