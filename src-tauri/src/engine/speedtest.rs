//! 下载源测速。
//!
//! 国内用户最常见的"下载慢"其实是选错了源：官方 `piston-meta.mojang.com`
//! 在国外，跨境链路经常只有几十 KB/s；BMCLAPI 在国内，通常快一个数量级。
//! 但 BMCLAPI 偶尔也会抽风，那时候官方反而更快。
//!
//! 所以这里对两个源各测一次「拉取版本清单前 32 KB 的耗时」，让玩家看到实际
//! 数字再决定，而不是凭感觉猜。
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;

use crate::engine::net::{MIRROR, OFFICIAL_META};

/// 只读这么多字节就足够判断链路质量。
const SAMPLE: usize = 32 * 1024;
/// 单个源最多等这么久，超时就当它不可用。
const LIMIT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProbe {
    /// `official` | `bmclapi`
    pub source: String,
    /// 成功时是毫秒数，失败时是 `None`
    pub ms: Option<u64>,
    /// 拉到的字节数，用来区分「快但只拿到 0 字节」这种假成功
    pub bytes: usize,
    /// 失败原因
    pub error: String,
}

impl SourceProbe {
    fn failed(source: &str, error: impl Into<String>) -> Self {
        Self {
            source: source.to_string(),
            ms: None,
            bytes: 0,
            error: error.into(),
        }
    }

    /// 拿到 0 字节不算成功：有的代理会返回 200 + 空体。
    pub fn ok(&self) -> bool {
        self.ms.is_some() && self.bytes > 0
    }

    /// 排序用的分数：越小越快，失败排最后。
    ///
    /// 直接复用 [`Self::ok`]：200 但空体的源必须算失败，
    /// 否则"最快的"会选中一个根本下不动东西的地址。
    pub fn score(&self) -> u64 {
        if !self.ok() {
            return u64::MAX;
        }
        self.ms.unwrap_or(u64::MAX)
    }
}

fn manifest_url(source: &str) -> String {
    if source == "official" {
        format!("{OFFICIAL_META}/mc/game/version_manifest_v2.json")
    } else {
        format!("{MIRROR}/mc/game/version_manifest_v2.json")
    }
}

/// 测一个源的耗时。
///
/// 用 GET 而不是 HEAD：官方的元数据站走 Cloudflare，HEAD 可能被拦，
/// 而真正的下载本来就是 GET。读到 32 KB 就断开，不会真的下完整个清单。
pub async fn probe(client: &reqwest::Client, source: &str) -> SourceProbe {
    let url = manifest_url(source);
    let started = Instant::now();

    let read = async {
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("连接失败：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let mut stream = resp.bytes_stream();
        let mut got = 0usize;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    got += bytes.len();
                    if got >= SAMPLE {
                        break;
                    }
                }
                Err(e) => return Err(format!("读取中断：{e}")),
            }
        }
        if got == 0 {
            return Err("没有收到数据".to_string());
        }
        Ok(got)
    };

    match tokio::time::timeout(LIMIT, read).await {
        Ok(Ok(bytes)) => SourceProbe {
            source: source.to_string(),
            ms: Some(started.elapsed().as_millis() as u64),
            bytes,
            error: String::new(),
        },
        Ok(Err(message)) => SourceProbe::failed(source, message),
        Err(_) => SourceProbe::failed(source, format!("超时（{} 秒）", LIMIT.as_secs())),
    }
}

/// 并发测两个源，按快慢排序返回。
///
/// 并发而不是串行：串行时如果第一个源超时，玩家要白等 8 秒才看到第二个结果。
pub async fn test_all(client: &reqwest::Client) -> Vec<SourceProbe> {
    let (official, mirror) = tokio::join!(probe(client, "official"), probe(client, "bmclapi"));
    let mut list = vec![official, mirror];
    list.sort_by_key(|p| p.score());
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_urls_point_at_the_right_hosts() {
        assert!(manifest_url("official").starts_with(OFFICIAL_META));
        assert!(manifest_url("bmclapi").starts_with(MIRROR));
    }

    #[test]
    fn failed_probe_sorts_last() {
        let ok = SourceProbe {
            source: "official".into(),
            ms: Some(320),
            bytes: SAMPLE,
            error: String::new(),
        };
        let slow = SourceProbe {
            source: "bmclapi".into(),
            ms: Some(900),
            bytes: SAMPLE,
            error: String::new(),
        };
        let dead = SourceProbe::failed("bmclapi", "超时");
        assert!(ok.score() < slow.score());
        assert!(slow.score() < dead.score());
        assert!(!dead.ok());
    }

    #[test]
    fn fastest_skips_failed_sources() {
        let dead = SourceProbe::failed("official", "超时");
        let alive = SourceProbe {
            source: "bmclapi".into(),
            ms: Some(150),
            bytes: SAMPLE,
            error: String::new(),
        };
        // 前端按 score 排序取第一个可用的，这里验证排序语义
        let mut list = [dead.clone(), alive];
        list.sort_by_key(|p| p.score());
        assert_eq!(list[0].source, "bmclapi");
        assert!(list.iter().all(|p| p.score() < u64::MAX || p.source == "official"));
    }

    /// 拿到 0 字节不能算成功：有的代理会返回 200 + 空体。
    #[test]
    fn zero_bytes_is_not_a_success() {
        let probe = SourceProbe {
            source: "official".into(),
            ms: Some(10),
            bytes: 0,
            error: String::new(),
        };
        assert!(!probe.ok());
        assert_eq!(probe.score(), u64::MAX);
    }
}
