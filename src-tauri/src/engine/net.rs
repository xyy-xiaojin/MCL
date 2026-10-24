//! 网络层：共享 HTTP 客户端 + 镜像地址重写 + 带重试的文本拉取。
//!
//! 重试这块有三个容易写错的地方，都在这里收口：
//! 1. **不是所有失败都值得重试**——403/404 重试三次只是让用户多等；
//! 2. **重试要换源**——镜像抖了就换官方，官方不通再回镜像，比死磕一个地址有用；
//! 3. **限流（429）要退避得久一点**，否则越试越被限。
use std::time::Duration;

use crate::error::{Error, Result};

/// Mojang 元数据站。
pub const OFFICIAL_META: &str = "https://piston-meta.mojang.com";
/// Mojang 资源站（assets 对象）。
pub const OFFICIAL_RESOURCES: &str = "https://resources.download.minecraft.net";
/// 国内镜像。
pub const MIRROR: &str = "https://bmclapi2.bangbang93.com";

/// 下载源 → 元数据基址。
pub fn base_url_for(source: &str) -> &'static str {
    if source == "official" {
        OFFICIAL_META
    } else {
        MIRROR
    }
}

/// 全局共享的 reqwest 客户端。
///
/// 注意**不要**设 `timeout`：reqwest 的 `timeout` 覆盖整个请求（含响应体），
/// 客户端 jar 有几十 MB，一刀切的总超时会把正常下载判成失败。
/// 真正需要防的是"连不上"，所以只设 `connect_timeout`。
///
/// 连接复用对下载速度影响很大：一次 TLS 握手在跨运营商链路上可能要
/// 200–400 ms，几千个 assets 文件全量下载时，不复用连接光握手就吃掉几十秒。
/// 所以把空闲连接池调大、空闲时间拉长。
pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("MCL/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        // 16 段分片 + 10 个文件并发时，峰值可能接近 160 个连接。
        // 这里增加空闲连接保留量，减少连续下载同一镜像时反复 TLS 握手。
        .pool_max_idle_per_host(128)
        .pool_idle_timeout(Duration::from_secs(300))
        // 关闭 Nagle：小分片请求不必等 40ms 的合并窗口
        .tcp_nodelay(true)
        .build()?)
}

/// 把官方地址重写到镜像。
///
/// 只替换已知的几个域名，避免误伤第三方 maven（Forge / Fabric 的仓库
/// 有自己的镜像规则，乱改会 404）。
pub fn rewrite(url: &str, source: &str) -> String {
    if source == "official" {
        return url.to_string();
    }
    // 客户端 jar 挂在 piston-data 的 /v1/objects/ 下，镜像没有这个路径。
    // 原样返回，由调用方改用 `client_endpoint`（/version/<id>/client）。
    if url.starts_with("https://piston-data.mojang.com") {
        return url.to_string();
    }
    url.replace("https://piston-meta.mojang.com", MIRROR)
        .replace("https://launchermeta.mojang.com", MIRROR)
        .replace("https://launcher.mojang.com", MIRROR)
        // BMCLAPI 的类库目录叫 /maven，不是 /libraries
        .replace(
            "https://libraries.minecraft.net",
            &format!("{MIRROR}/maven"),
        )
        .replace(
            "https://resources.download.minecraft.net",
            &format!("{MIRROR}/assets"),
        )
}

/// 镜像的客户端 jar 规范端点。
pub fn client_endpoint(source: &str, version_id: &str) -> String {
    if source == "official" {
        // 官方没有这个端点，调用方应改用 version.json 里的 downloads.client.url
        String::new()
    } else {
        format!("{MIRROR}/version/{version_id}/client")
    }
}

/// 同一个文件的候选地址：先当前下载源，再另一头。
///
/// 调用方传进来的是**官方原始 URL**，这里负责给出「镜像 → 官方」或
/// 「官方 → 镜像」两条路。两段相等时说明这个 URL 换不了源（比如第三方 maven）。
pub fn endpoints(original_url: &str, source: &str) -> Vec<String> {
    let primary = rewrite(original_url, source);
    // 站在另一头看同一个文件：官方源就把镜像推出来，镜像源就用原始地址
    let fallback = if source == "official" {
        rewrite(original_url, "bmclapi")
    } else {
        original_url.to_string()
    };
    if fallback == primary {
        vec![primary]
    } else {
        vec![primary, fallback]
    }
}

/// 极简 percent-encode：不引入额外依赖，只处理会影响 URL 的字符。
///
/// Modrinth 的 facets 是一段 JSON（`[["project_type:mod"]]`），
/// 方括号和引号必须编码，否则请求直接 400。
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 失败归类：决定还能不能重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 连不上、超时、5xx、连接被重置——换源或者再试都有意义
    Transient,
    /// 429：被限流，要等得比普通失败久
    Throttled,
    /// 403 / 404：内容真没了，重试只是白等
    Missing,
    /// 内容不对（长度不够、哈希不符）
    Corrupt,
}

impl FailureKind {
    pub fn retryable(self) -> bool {
        matches!(self, Self::Transient | Self::Throttled)
    }
}

/// HTTP 状态码 → 失败归类。
pub fn classify_status(status: u16) -> FailureKind {
    match status {
        429 => FailureKind::Throttled,
        408 | 500..=599 => FailureKind::Transient,
        s if (400..500).contains(&s) => FailureKind::Missing,
        _ => FailureKind::Transient,
    }
}

/// 重试参数。
///
/// 退避是 `base_delay × 2^(n-1)`，最多叠加到 8 秒；429 额外再加 5 秒。
/// 故意不用 PCL 那套 10s/30s/4s 的阶梯：那是给几万个文件的长任务调的，
/// 用户盯着进度条，等 30 秒会以为卡死。
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// 单个地址最多试几次（含第一次）
    pub attempts: u32,
    /// 第一次重试前的等待
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(700),
        }
    }
}

impl RetryPolicy {
    /// 第 `attempt` 次（从 1 开始）失败后应该等多久；返回 `None` 表示不该再试。
    pub fn delay(&self, attempt: u32, kind: FailureKind) -> Option<Duration> {
        if attempt >= self.attempts || !kind.retryable() {
            return None;
        }
        let factor = 1u64 << (attempt.saturating_sub(1)).min(4);
        let base = (self.base_delay.as_millis() as u64).saturating_mul(factor);
        let extra = if kind == FailureKind::Throttled { 5000 } else { 0 };
        Some(Duration::from_millis(base.min(8000) + extra))
    }
}

/// 按主备顺序轮换地址执行一次操作。
///
/// `op` 返回 `Ok(Ok(T))` 表示成功；`Ok(Err((kind, err)))` 表示这次失败但可以换源；
/// `Err` 是调用方自己的不可恢复错误（直接冒泡）。
pub(crate) async fn for_each_endpoint<T, F, Fut>(
    urls: &[String],
    policy: RetryPolicy,
    mut op: F,
) -> Result<T>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, Errored>>,
{
    let mut last: Option<String> = None;
    // 判了死刑（404 之类）的地址，后面每一轮都不必再碰
    let mut dead: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 每轮单独记"最严重"的失败类型：429 要退避得更久，一直按 Transient 算的话
    // policy.delay 里的 +5s 永远不会生效。
    for round in 1..=policy.attempts.max(1) {
        let mut round_kind = FailureKind::Transient;
        // 这一轮里有没有出现过"再等等或许能成"的失败。整轮全是 404 /
        // 哈希不符这种换源也没用的失败时，退避 + 重跑只是让用户白等。
        let mut round_retryable = false;
        for url in urls {
            if dead.contains(url) {
                continue;
            }
            match op(url.clone()).await {
                Ok(value) => return Ok(value),
                Err(failed) => {
                    let kind = failed.kind;
                    last = Some(failed.message);
                    // 这一轮里只要出现过限流，退避就按限流算
                    if kind == FailureKind::Throttled {
                        round_kind = kind;
                    }
                    if kind.retryable() {
                        round_retryable = true;
                    } else {
                        // 这个地址已经判死刑（404 之类），这一轮和后面的轮次都不再试它
                        dead.insert(url.clone());
                    }
                }
            }
        }
        if !round_retryable {
            break;
        }
        if let Some(delay) = policy.delay(round, round_kind) {
            tokio::time::sleep(delay).await;
        } else {
            break;
        }
    }
    Err(Error::msg(
        last.unwrap_or_else(|| "所有下载源都失败了".to_string()),
    ))
}

/// 带归类信息的失败，供 [`for_each_endpoint`] 决定要不要换源。
pub(crate) struct Errored {
    pub kind: FailureKind,
    pub message: String,
}

impl Errored {
    pub(crate) fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn transient(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Transient, message)
    }

    pub(crate) fn status(status: u16, url: &str) -> Self {
        Self::new(
            classify_status(status),
            format!("HTTP {status}（{url}）"),
        )
    }
}

/// 拉取文本，最多 3 次（单个地址）。
///
/// 4xx（除 429）直接放弃：这类错误重试结果不会变。
pub async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let urls = vec![url.to_string()];
    get_text_multi(client, &urls, RetryPolicy::default()).await
}

/// 拉取文本，支持主备地址与退避重试。
pub async fn get_text_multi(
    client: &reqwest::Client,
    urls: &[String],
    policy: RetryPolicy,
) -> Result<String> {
    for_each_endpoint(urls, policy, |url| {
        let client = client.clone();
        async move {
            let resp = match client.get(&url).send().await {
                Ok(resp) => resp,
                Err(e) => return Err(Errored::transient(format!("网络请求失败：{e}"))),
            };
            let status = resp.status();
            if !status.is_success() {
                return Err(Errored::status(status.as_u16(), &url));
            }
            match resp.text().await {
                Ok(text) => Ok(text),
                Err(e) => Err(Errored::transient(format!("读取响应失败：{e}"))),
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_source_keeps_urls_untouched() {
        let url = "https://libraries.minecraft.net/org/a/b.jar";
        assert_eq!(rewrite(url, "official"), url);
    }

    #[test]
    fn mirror_rewrites_known_hosts() {
        assert_eq!(
            rewrite("https://piston-meta.mojang.com/mc/game/version_manifest.json", "bmclapi"),
            format!("{MIRROR}/mc/game/version_manifest.json")
        );
        assert_eq!(
            rewrite("https://libraries.minecraft.net/org/a/b.jar", "bmclapi"),
            format!("{MIRROR}/maven/org/a/b.jar")
        );
        assert_eq!(
            rewrite("https://resources.download.minecraft.net/ab/abcdef", "bmclapi"),
            format!("{MIRROR}/assets/ab/abcdef")
        );
    }

    #[test]
    fn third_party_repos_are_not_rewritten() {
        let url = "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/x.jar";
        assert_eq!(rewrite(url, "bmclapi"), url);
    }

    #[test]
    fn client_object_url_is_left_for_the_caller() {
        // 镜像上不存在 /v1/objects/.../client.jar，必须由调用方换端点
        let url = "https://piston-data.mojang.com/v1/objects/abc/client.jar";
        assert_eq!(rewrite(url, "bmclapi"), url);
        assert_eq!(
            client_endpoint("bmclapi", "1.20.1"),
            format!("{MIRROR}/version/1.20.1/client")
        );
    }

    #[test]
    fn endpoints_gives_mirror_then_official() {
        let url = "https://libraries.minecraft.net/a/b.jar";
        let list = endpoints(url, "bmclapi");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], format!("{MIRROR}/maven/a/b.jar"));
        assert_eq!(list[1], url);

        let list = endpoints(url, "official");
        assert_eq!(list[0], url);
        assert_eq!(list[1], format!("{MIRROR}/maven/a/b.jar"));
    }

    #[test]
    fn endpoints_collapses_when_url_cannot_be_rewritten() {
        let url = "https://maven.fabricmc.net/x.jar";
        let list = endpoints(url, "bmclapi");
        assert_eq!(list, vec![url.to_string()]);
    }

    #[test]
    fn retry_policy_stops_on_client_errors() {
        let policy = RetryPolicy::default();
        assert!(policy.delay(1, FailureKind::Transient).is_some());
        assert!(policy.delay(1, FailureKind::Missing).is_none());
        // 第 3 次之后不再重试
        assert!(policy.delay(3, FailureKind::Transient).is_none());
        // 429 要等得更久
        let throttled = policy.delay(1, FailureKind::Throttled).unwrap();
        let transient = policy.delay(1, FailureKind::Transient).unwrap();
        assert!(throttled > transient);
    }

    #[test]
    fn status_classification_matches_expectations() {
        assert_eq!(classify_status(404), FailureKind::Missing);
        assert_eq!(classify_status(403), FailureKind::Missing);
        assert_eq!(classify_status(429), FailureKind::Throttled);
        assert_eq!(classify_status(503), FailureKind::Transient);
    }
}
