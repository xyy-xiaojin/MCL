//! 安装加载器。
//!
//! Fabric / Quilt 的官方 meta 服务直接给一份完整的 version.json（含
//! mainClass 和 libraries 列表），不需要跑 Java 安装器 —— 整个过程
//! 就是「拉一份 JSON → 补上 inheritsFrom → 落盘 → 类库入队」。
//!
//! Forge / NeoForge 只有 installer.jar，走 [`crate::engine::forge`] 的
//! 无头安装流程（下载官方安装器 → `--installClient` → 归化版本目录）。
use serde::Serialize;

use crate::engine::meta::{self, Platform};
use crate::engine::net::{self, RetryPolicy};
use crate::engine::queue::NewTask;
use crate::error::{Error, Result};
use crate::state::AppState;

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
const QUILT_META: &str = "https://meta.quiltmc.org/v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl Loader {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fabric" => Ok(Self::Fabric),
            "quilt" => Ok(Self::Quilt),
            "forge" => Ok(Self::Forge),
            "neoforge" => Ok(Self::NeoForge),
            other => Err(Error::msg(format!("不认识的加载器：{other}"))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    /// 走 meta 服务（JSON 直接给）还是官方安装器。
    fn uses_meta(self) -> bool {
        matches!(self, Self::Fabric | Self::Quilt)
    }

    fn meta_base(self) -> &'static str {
        match self {
            Self::Fabric => FABRIC_META,
            Self::Quilt => QUILT_META,
            _ => "",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderReport {
    /// 新建的版本 id（`versions/<id>/`）
    pub version_id: String,
    pub loader: String,
    pub loader_version: String,
    pub game_version: String,
    /// 入队下载的类库数量
    pub libraries: usize,
    /// 有类库没下下来时的提示（版本已创建，但启动前需要重试）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

// meta 服务返回的加载器列表条目，只取用得到的字段
#[derive(Debug, serde::Deserialize)]
struct LoaderEntry {
    loader: LoaderMeta,
}

#[derive(Debug, serde::Deserialize)]
struct LoaderMeta {
    #[serde(default)]
    version: String,
    #[serde(default)]
    stable: bool,
}

/// 默认版本名：`fabric-loader-0.16.9-1.21.4`，与官方启动器的命名一致。
pub fn default_version_id(loader: Loader, loader_version: &str, game_version: &str) -> String {
    format!(
        "{}-loader-{}-{}",
        loader.as_str(),
        loader_version,
        game_version
    )
}

/// 挑一个加载器版本：优先稳定版，其次是列表里第一个。
async fn pick_loader_version(
    state: &AppState,
    loader: Loader,
    game_version: &str,
) -> Result<String> {
    let url = format!(
        "{}/versions/loader/{}",
        loader.meta_base(),
        net::percent_encode(game_version)
    );
    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    let list: Vec<LoaderEntry> = serde_json::from_str(&text)
        .map_err(|e| Error::msg(format!("{} 版本列表解析失败：{e}", loader.display())))?;

    list.iter()
        .find(|entry| entry.loader.stable)
        .or_else(|| list.first())
        .map(|entry| entry.loader.version.clone())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            Error::msg(format!(
                "{} 没有支持 {game_version} 的版本",
                loader.display()
            ))
        })
}

/// 安装加载器。Fabric / Quilt 走 meta 服务（见 [`install_meta`]），
/// Forge / NeoForge 走官方安装器（见 [`crate::engine::forge`]）。
pub async fn install(
    state: &AppState,
    loader: Loader,
    game_version: &str,
    loader_version: Option<&str>,
    target_id: Option<&str>,
) -> Result<LoaderReport> {
    if loader.uses_meta() {
        install_meta(state, loader, game_version, loader_version, target_id).await
    } else {
        crate::engine::forge::install(state, loader, game_version, loader_version, target_id).await
    }
}

/// 把加载器装成一个独立版本（Fabric / Quilt 的 meta 服务流程）。
///
/// `target_id` 为空时按官方命名规则生成（`fabric-loader-0.16.9-1.21.4`）。
/// 基础版本必须已经装好：加载器的 JSON 只写了「在哪个版本之上加壳」，
/// 客户端 jar、assets、原版类库都还是要靠基础版本。
async fn install_meta(
    state: &AppState,
    loader: Loader,
    game_version: &str,
    loader_version: Option<&str>,
    target_id: Option<&str>,
) -> Result<LoaderReport> {
    let game_version = game_version.trim();
    if !meta::is_safe_id(game_version) {
        return Err(Error::msg(format!("游戏版本 ID 不合法：{game_version}")));
    }
    let base_json = state
        .paths
        .version_dir(game_version)
        .join(format!("{game_version}.json"));
    if !base_json.is_file() {
        return Err(Error::msg(format!(
            "还没有安装原版 {game_version}。加载器要叠在原版之上，请先安装它"
        )));
    }

    let loader_version = match loader_version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => value.to_string(),
        None => pick_loader_version(state, loader, game_version).await?,
    };

    let profile_url = format!(
        "{}/versions/loader/{}/{}/profile/json",
        loader.meta_base(),
        net::percent_encode(game_version),
        net::percent_encode(&loader_version)
    );
    let text = net::get_text_multi(&state.client, &[profile_url], RetryPolicy::default()).await?;
    // 保留原始 JSON 改写两个字段，而不是反序列化再序列化：
    // `VersionJson` 只实现了 Deserialize，而且原样保留能避免漏字段。
    let mut profile: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        Error::msg(format!("{} profile 解析失败：{e}", loader.display()))
    })?;
    let parsed: meta::VersionJson = serde_json::from_value(profile.clone())
        .map_err(|e| Error::msg(format!("{} profile 结构不对：{e}", loader.display())))?;

    let target = match target_id.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => value.to_string(),
        None => default_version_id(loader, &loader_version, game_version),
    };
    if !meta::is_safe_id(&target) {
        return Err(Error::msg(format!("版本 ID 不合法：{target}")));
    }
    let dir = state.paths.version_dir(&target);
    if dir.join(format!("{target}.json")).is_file() {
        return Err(Error::msg(format!("版本 {target} 已经存在了")));
    }

    // 关键：告诉启动阶段「客户端 jar / assets / 原版类库去哪个版本找」
    profile["id"] = serde_json::Value::String(target.clone());
    profile["inheritsFrom"] = serde_json::Value::String(game_version.to_string());

    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{target}.json")),
        serde_json::to_string_pretty(&profile)?,
    )?;

    // 类库：Fabric 的条目基本只给 maven 坐标 + 仓库地址，没有 downloads
    let platform = Platform::current();
    let source = state.source();
    let mut tasks = Vec::new();
    for library in &parsed.libraries {
        if !meta::allowed(library.rules.as_deref(), &platform) {
            continue;
        }
        let Some(rel) = meta::library_path(library) else {
            continue;
        };
        let dest = state.paths.libraries().join(meta::rel_path(&rel));
        let url = library
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .and_then(|a| a.url.clone())
            .or_else(|| {
                library
                    .url
                    .as_ref()
                    .map(|base| format!("{}/{rel}", base.trim_end_matches('/')))
            });
        // 没有下载地址的条目照样要留在 classpath 里，只是不下载
        let Some(url) = url else { continue };
        let artifact = library.downloads.as_ref().and_then(|d| d.artifact.as_ref());
        tasks.push(
            NewTask::from_official(
                "library",
                short_name(&library.name),
                &url,
                &source,
                dest,
            )
            .with_sha1(artifact.and_then(|a| a.sha1.clone()))
            .with_size(artifact.and_then(|a| a.size)),
        );
    }

    let libraries = tasks.len();
    // 等类库真正下完再返回。前端拿到结果就会切到新版本并放开"启动"按钮，
    // 如果 fabric-loader 还躺在队列里，游戏会直接报
    // "Could not find or load main class net.fabricmc.loader.impl.launch.knot.KnotClient"。
    let mut warning: Option<String> = None;
    if !tasks.is_empty() {
        let ids = state.queue.enqueue(tasks);
        let failed = state.queue.wait(&ids).await;
        if !failed.is_empty() {
            warning = Some(format!(
                "版本已创建，但有 {} 个类库下载失败（{}：{}）。启动前请在下载页重试。",
                failed.len(),
                failed[0].0,
                failed[0].1
            ));
        }
    }

    Ok(LoaderReport {
        version_id: target,
        loader: loader.as_str().to_string(),
        loader_version,
        game_version: game_version.to_string(),
        libraries,
        warning,
    })
}

/// `net.fabricmc:fabric-loader:0.16.9` → `fabric-loader 0.16.9`
fn short_name(coordinate: &str) -> String {
    let parts: Vec<&str> = coordinate.split(':').collect();
    match (parts.get(1), parts.get(2)) {
        (Some(artifact), Some(version)) => format!("{artifact} {version}"),
        _ => coordinate.to_string(),
    }
}

/// 列出某个加载器支持的所有版本（给前端下拉用）。
pub async fn available(
    state: &AppState,
    loader: Loader,
    game_version: &str,
) -> Result<Vec<String>> {
    if !loader.uses_meta() {
        return crate::engine::forge::available(state, loader, game_version).await;
    }
    let url = format!(
        "{}/versions/loader/{}",
        loader.meta_base(),
        net::percent_encode(game_version.trim())
    );
    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    let list: Vec<LoaderEntry> = serde_json::from_str(&text)?;
    Ok(list
        .into_iter()
        .map(|entry| entry.loader.version)
        .filter(|v| !v.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_id_matches_official_naming() {
        assert_eq!(
            default_version_id(Loader::Fabric, "0.16.9", "1.21.4"),
            "fabric-loader-0.16.9-1.21.4"
        );
    }

    #[test]
    fn forge_and_neoforge_parse() {
        assert_eq!(Loader::parse("forge").unwrap(), Loader::Forge);
        assert_eq!(Loader::parse("NeoForge").unwrap(), Loader::NeoForge);
        assert!(!Loader::Forge.uses_meta());
        assert!(Loader::Quilt.uses_meta());
    }

    #[test]
    fn unknown_loader_is_rejected() {
        assert!(Loader::parse("liteloader").is_err());
        assert_eq!(Loader::parse("FABRIC").unwrap(), Loader::Fabric);
    }

    #[test]
    fn short_name_keeps_artifact_and_version() {
        assert_eq!(
            short_name("net.fabricmc:fabric-loader:0.16.9"),
            "fabric-loader 0.16.9"
        );
    }
}
