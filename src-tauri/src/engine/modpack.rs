//! 整合包：Modrinth 搜索 + `.mrpack` / 普通 zip 导入。
//!
//! **导入 = 新建一个独立版本**（和 PCL 一致）：
//!
//! ```text
//! versions/<包名>/<包名>.json      ← 版本 JSON（薄壳 inheritsFrom，或加载器给的完整 JSON）
//! versions/<包名>/game/            ← 这个包自己的 mods / config / saves
//! ```
//!
//! `.mrpack` 里的 `dependencies` 会告诉我们「这个包要哪个 Minecraft 版本、
//! 哪个加载器版本」，所以能自动把原版和 Fabric 装好再铺 overrides。
//! 普通 zip 没有这些信息，得靠调用方传一个基础版本。
//!
//! 下载走全局队列：几百个 mod 的进度会出现在下载页，可以暂停、可以看到进度，
//! 而不是卡在一个看不到内部的循环里。
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::loader::{self, Loader};
use crate::engine::queue::NewTask;
use crate::engine::{install, net};
use crate::error::{Error, Result};
use crate::state::AppState;

const MODRINTH: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModpackHit {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub icon_url: String,
    pub downloads: u64,
    pub versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<RawHit>,
}

#[derive(Debug, Deserialize)]
struct RawHit {
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    versions: Vec<String>,
}

/// 搜索整合包。只有 `project_type:modpack` 的项目才会返回。
pub async fn search(state: &AppState, query: &str) -> Result<Vec<ModpackHit>> {
    let facets = net::percent_encode(r#"[["project_type:modpack"]]"#);
    let url = format!(
        "{MODRINTH}/search?limit=12&index=relevance&query={}&facets={facets}",
        net::percent_encode(query.trim())
    );
    let text = net::get_text(&state.client, &url).await?;
    let parsed: SearchResponse = serde_json::from_str(&text)?;
    Ok(parsed
        .hits
        .into_iter()
        .map(|hit| ModpackHit {
            id: hit.project_id,
            slug: hit.slug,
            title: hit.title,
            description: hit.description,
            icon_url: hit.icon_url.unwrap_or_default(),
            downloads: hit.downloads,
            versions: hit.versions,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub instance: String,
    /// 新建出来的版本 id —— 前端据此切到新版本
    pub version_id: String,
    /// 基底版本（原版号）
    pub game_version: String,
    /// 加载器描述，例如 `Fabric 0.16.9` / `原版`
    pub loader: String,
    pub dir: String,
    /// 解压出来的 overrides 文件数
    pub overrides: usize,
    /// 丢进下载队列的模组数
    pub files: usize,
    /// 因为目标已存在而跳过的文件数
    pub skipped: usize,
    /// 需要玩家注意的事（比如「这个包要 Forge，还不能自动装」）
    pub note: String,
}

#[derive(Debug, Deserialize)]
struct ModrinthIndex {
    #[serde(default)]
    name: String,
    #[serde(rename = "versionId", default)]
    version_id: Option<String>,
    #[serde(default)]
    files: Vec<IndexFile>,
    #[serde(default)]
    dependencies: Dependencies,
}

/// `.mrpack` 的依赖声明。键名带连字符，必须逐个 rename。
#[derive(Debug, Default, Deserialize)]
struct Dependencies {
    #[serde(default)]
    minecraft: Option<String>,
    #[serde(rename = "fabric-loader", default)]
    fabric_loader: Option<String>,
    #[serde(rename = "quilt-loader", default)]
    quilt_loader: Option<String>,
    #[serde(default)]
    forge: Option<String>,
    #[serde(default)]
    neoforge: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    hashes: Hashes,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(rename = "fileSize", default)]
    file_size: Option<u64>,
    #[serde(default)]
    env: FileEnv,
}

/// `.mrpack` 里的 `env` 段。
///
/// 规范允许把某个文件标成 `client: "unsupported"`（服务端专用 mod，比如
/// 权限插件、区块预生成器）。这类文件**不能**装进客户端：装上轻则白占空间，
/// 重则一进游戏就崩。
#[derive(Debug, Default, Deserialize)]
struct FileEnv {
    #[serde(default)]
    client: Option<String>,
}

impl FileEnv {
    fn client_unsupported(&self) -> bool {
        self.client
            .as_deref()
            .map(|value| value.trim().eq_ignore_ascii_case("unsupported"))
            .unwrap_or(false)
    }
}

#[derive(Debug, Default, Deserialize)]
struct Hashes {
    #[serde(default)]
    sha1: Option<String>,
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "modpack".to_string()
    } else {
        trimmed
    }
}

/// 判断压缩包/索引里的相对路径能不能安全地落到实例目录里。
///
/// 除了 `..`，还要挡绝对路径（`/etc/x`）和 Windows 盘符/ADS（`C:/x`、`a:b`），
/// 否则 `instance_game.join("C:/x")` 会直接跳出实例目录。
fn is_safe_relative(relative: &str) -> bool {
    if relative.is_empty() || relative.starts_with('/') || relative.contains(':') {
        return false;
    }
    relative
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// 找一个没被占用的版本 id：`fabulously-optimized`、`fabulously-optimized-2`…
///
/// 绝不复用已有目录 —— 导入一个新包不该覆盖玩家之前的实例。
fn unique_version_id(state: &AppState, name: &str) -> String {
    let slug = slugify(name);
    if !state.paths.version_dir(&slug).exists() {
        return slug;
    }
    for index in 2..1000u32 {
        let candidate = format!("{slug}-{index}");
        if !state.paths.version_dir(&candidate).exists() {
            return candidate;
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{slug}-{stamp}")
}

/// 写一个"薄壳"版本 JSON：只声明继承自哪个版本，其余全部复用基底。
///
/// 这是原版 + 只换游戏目录时的最小写法：客户端 jar、assets、类库都从
/// `inheritsFrom` 指向的版本拿，config/mods 落在自己的 `game/` 里。
fn write_thin_version(state: &AppState, id: &str, base: &str, note: &str) -> Result<()> {
    let dir = state.paths.version_dir(id);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::json!({
        "id": id,
        "inheritsFrom": base,
        "type": "release",
        "mcl": { "note": note },
    });
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&json)?,
    )?;
    Ok(())
}

/// 导入任意整合包。
///
/// `.mrpack` 自带依赖信息，会新建版本并自动补原版 / 加载器；
/// 普通 zip 没有这些信息，`base_version` 必须是一个已装好的版本。
pub async fn import_any(
    state: &AppState,
    archive: &str,
    base_version: &str,
) -> Result<ImportReport> {
    let path = PathBuf::from(archive);
    if !path.is_file() {
        return Err(Error::msg(format!("找不到整合包文件：{archive}")));
    }

    // 探测类型：读中央目录也是阻塞 I/O，放 blocking 池里做
    let probe = path.clone();
    let is_mrpack = tokio::task::spawn_blocking(move || -> bool {
        let Ok(file) = std::fs::File::open(&probe) else {
            return false;
        };
        let Ok(mut zip) = zip::ZipArchive::new(file) else {
            return false;
        };
        let found = zip.by_name("modrinth.index.json").is_ok();
        found
    })
    .await
    .unwrap_or(false);
    if is_mrpack {
        return import_mrpack(state, &path, archive).await;
    }
    import_plain_zip(state, &path, base_version).await
}

/// 读出压缩包里某个文本条目。
fn read_entry_text(archive: &PathBuf, name: &str) -> Result<String> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entry = zip
        .by_name(name)
        .map_err(|_| Error::msg(format!("这个压缩包里没有 {name}")))?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut entry, &mut text)?;
    Ok(text)
}

/// 解压 `overrides/` 与 `client-overrides/` 到实例目录。
///
/// 规范里同名文件以 client-overrides 为准，所以第二遍覆盖写入。
/// 几千个文件解压是纯阻塞活，调用方负责丢进 blocking 池。
fn extract_overrides(archive: &PathBuf, dest: &Path) -> Result<(usize, usize)> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut extracted = 0usize;
    let mut skipped = 0usize;
    for prefix in ["overrides/", "client-overrides/"] {
        let overwrite = prefix == "client-overrides/";
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let normalized = name.to_string_lossy().replace('\\', "/");
            let Some(relative) = normalized.strip_prefix(prefix) else {
                continue;
            };
            if !is_safe_relative(relative) {
                continue;
            }
            let target = dest.join(relative);
            if target.exists() && !overwrite {
                skipped += 1;
                continue;
            }
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut out)?;
            extracted += 1;
        }
    }
    Ok((extracted, skipped))
}

/// `.mrpack`：按 dependencies 补原版与加载器，再铺 overrides。
async fn import_mrpack(state: &AppState, path: &Path, archive: &str) -> Result<ImportReport> {
    let read_from = path.to_path_buf();
    let index_text = tokio::task::spawn_blocking(move || {
        read_entry_text(&read_from, "modrinth.index.json")
            .map_err(|_| Error::msg("这不是有效的 .mrpack：缺少 modrinth.index.json"))
    })
    .await
    .map_err(|e| Error::msg(format!("读取整合包失败：{e}")))??;
    let index: ModrinthIndex = serde_json::from_str(&index_text)
        .map_err(|e| Error::msg(format!("整合包索引解析失败：{e}")))?;

    let game_version = index
        .dependencies
        .minecraft
        .clone()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| Error::msg("这个整合包没有声明 Minecraft 版本，无法自动安装"))?;

    let display_name = if index.name.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "modpack".to_string())
    } else {
        index.name.clone()
    };
    let target_id = unique_version_id(state, &display_name);

    // 1) 原版：加载器要叠在它上面，客户端 jar / assets 也都从这里来。
    //    只判断 json 在不在是不够的——上次装到一半（json 有了、jar 没下完）
    //    再导入包，会得到一个"看着装好了、一启动就少 jar"的实例。
    if !install::is_installed(&state.paths, &game_version) {
        install::install(state, &game_version).await?;
    }

    // 2) 加载器
    let (loader_label, mut note) = match (
        index.dependencies.fabric_loader.clone(),
        index.dependencies.quilt_loader.clone(),
    ) {
        (Some(version), _) => {
            let report = loader::install(
                state,
                Loader::Fabric,
                &game_version,
                Some(&version),
                Some(&target_id),
            )
            .await?;
            let label = format!("Fabric {}", report.loader_version);
            (label, String::new())
        }
        (None, Some(version)) => {
            let report = loader::install(
                state,
                Loader::Quilt,
                &game_version,
                Some(&version),
                Some(&target_id),
            )
            .await?;
            let label = format!("Quilt {}", report.loader_version);
            (label, String::new())
        }
        (None, None) if index.dependencies.forge.is_some()
            || index.dependencies.neoforge.is_some() =>
        {
            // Forge / NeoForge 依赖：走真实安装（下载官方 installer 无头跑）。
            // forge::install 会保证原版就位、Java 可用，并把安装结果归化成
            // target_id，overrides 随后照常铺进这个版本的 game/。
            let kind = if index.dependencies.forge.is_some() {
                Loader::Forge
            } else {
                Loader::NeoForge
            };
            let version = index
                .dependencies
                .forge
                .clone()
                .or_else(|| index.dependencies.neoforge.clone())
                .unwrap_or_default();
            let report = loader::install(state, kind, &game_version, Some(&version), Some(&target_id)).await?;
            (
                format!("{} {}", kind.display(), report.loader_version),
                String::new(),
            )
        }
        (None, None) => {
            write_thin_version(state, &target_id, &game_version, "原版")?;
            ("原版".to_string(), String::new())
        }
    };

    let instance_game = state.paths.instance_game(&target_id);
    std::fs::create_dir_all(&instance_game)?;

    // 3) 解压 overrides/（配置、资源包、默认模组配置都在这儿）
    //    规范里还有 client-overrides/，同名文件应以 client 为准，所以第二遍覆盖写入。
    let extract_from = path.to_path_buf();
    let extract_to = instance_game.clone();
    let (overrides, skipped) = tokio::task::spawn_blocking(move || {
        extract_overrides(&extract_from, &extract_to)
    })
    .await
    .map_err(|e| Error::msg(format!("解压整合包失败：{e}")))??;

    // 4) 需求文件 → 下载队列
    let mut tasks = Vec::new();
    let mut skipped_server_only = 0usize;
    for item in &index.files {
        // 服务端专用的文件不进客户端（规范里的 env.client = "unsupported"）
        if item.env.client_unsupported() {
            skipped_server_only += 1;
            continue;
        }
        let Some(url) = item.downloads.first() else {
            continue;
        };
        // path 形如 "mods/xxx.jar"，但恶意包可以写 "../../x" 或 "C:/Windows/x"，
        // 统一走白名单校验，绝对路径/盘符/.. 一律丢掉。
        let relative = item.path.replace('\\', "/");
        if !is_safe_relative(&relative) {
            continue;
        }
        tasks.push(
            NewTask::new(
                "mod",
                relative.rsplit('/').next().unwrap_or(&relative),
                net::rewrite(url, &state.source()),
                instance_game.join(&relative),
            )
            .with_sha1(item.hashes.sha1.clone())
            .with_size(item.file_size),
        );
    }
    let files = tasks.len();
    // 这批文件来自整合包而不是零散的社区资源，组名要如实反映：
    // kind 都是 "mod"，按默认规则会被标成「社区资源下载」，玩家会误解。
    let group_id = state.queue.alloc_group();
    for task in &mut tasks {
        task.set_group(&group_id, "整合包文件下载");
    }
    state.queue.enqueue(tasks);

    // 提醒玩家：模组还在后台下载，别现在就去点启动
    if files > 0 {
        if !note.is_empty() {
            note.push('；');
        }
        note.push_str(&format!("{files} 个模组正在后台下载，可在「下载」页看进度"));
    }
    if skipped_server_only > 0 {
        if !note.is_empty() {
            note.push('；');
        }
        note.push_str(&format!("已跳过 {skipped_server_only} 个服务端专用文件"));
    }

    // 5) 记一份来源信息，方便以后识别这个实例
    write_instance_record(
        state,
        &target_id,
        &display_name,
        archive,
        index.version_id.as_deref(),
        &game_version,
        &loader_label,
    )?;

    Ok(ImportReport {
        instance: display_name,
        version_id: target_id,
        game_version,
        loader: loader_label,
        dir: instance_game.to_string_lossy().to_string(),
        overrides,
        files,
        skipped,
        note,
    })
}

/// 普通 zip 的落地：`overrides/` 前缀可有可无，其余按原路径铺。
fn extract_plain(archive: &PathBuf, game_dir: &Path) -> Result<(usize, usize)> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut extracted = 0usize;
    let mut skipped = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let normalized = name.to_string_lossy().replace('\\', "/");
        // 我们自己的导出物会带 mcl-manifest.json，纯元数据，不用落地
        if normalized == "mcl-manifest.json" {
            continue;
        }
        let relative = normalized.strip_prefix("overrides/").unwrap_or(&normalized);
        if relative.is_empty() || relative.split('/').any(|part| part == "..") {
            continue;
        }
        let target = game_dir.join(relative);
        if target.exists() {
            skipped += 1;
            continue;
        }
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        extracted += 1;
    }
    Ok((extracted, skipped))
}

/// 普通 zip：没有依赖信息，只能继承调用方指定的版本。
async fn import_plain_zip(
    state: &AppState,
    path: &Path,
    base_version: &str,
) -> Result<ImportReport> {
    if base_version.trim().is_empty() {
        return Err(Error::msg(
            "普通 zip 整合包没有写游戏版本。请先在「版本管理」里选一个版本作为基底",
        ));
    }
    if !state.paths.version_dir(base_version).is_dir() {
        return Err(Error::msg(format!(
            "基底版本 {base_version} 不存在，请重新选择"
        )));
    }

    let display_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "modpack".to_string());
    let target_id = unique_version_id(state, &display_name);
    write_thin_version(state, &target_id, base_version, "普通 zip")?;
    let game_dir = state.paths.instance_game(&target_id);
    std::fs::create_dir_all(&game_dir)?;

    let extract_from = path.to_path_buf();
    let extract_to = game_dir.clone();
    let (extracted, skipped) = tokio::task::spawn_blocking(move || {
        extract_plain(&extract_from, &extract_to)
    })
    .await
    .map_err(|e| Error::msg(format!("解压整合包失败：{e}")))??;

    if extracted == 0 && skipped == 0 {
        std::fs::remove_dir_all(state.paths.version_dir(&target_id)).ok();
        return Err(Error::msg(
            "压缩包里没有可识别的游戏文件（既不是 .mrpack，也没有 overrides/ 或 mods/）",
        ));
    }

    write_instance_record(
        state,
        &target_id,
        &display_name,
        &path.to_string_lossy(),
        None,
        base_version,
        "继承基底版本",
    )?;

    Ok(ImportReport {
        instance: display_name,
        version_id: target_id,
        game_version: base_version.to_string(),
        loader: format!("继承 {base_version}"),
        dir: game_dir.to_string_lossy().to_string(),
        overrides: extracted,
        files: 0,
        skipped,
        note: String::new(),
    })
}

/// 在 `instances/<id>/instance.json` 记一条来源，纯元数据，不参与游戏读取。
#[allow(clippy::too_many_arguments)]
fn write_instance_record(
    state: &AppState,
    id: &str,
    name: &str,
    source: &str,
    pack_version: Option<&str>,
    game_version: &str,
    loader: &str,
) -> Result<()> {
    let dir = state.paths.instances().join(id);
    std::fs::create_dir_all(&dir)?;
    let record = serde_json::json!({
        "id": id,
        "name": name,
        "source": source,
        "packVersionId": pack_version,
        "gameVersion": game_version,
        "loader": loader,
        "importedAt": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    std::fs::write(
        dir.join("instance.json"),
        serde_json::to_string_pretty(&record)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_makes_safe_directory_names() {
        assert_eq!(slugify("Fabulously Optimized"), "fabulously-optimized");
        assert_eq!(slugify("  A/B 测试  "), "a-b");
        assert_eq!(slugify("***"), "modpack");
        assert_eq!(slugify(""), "modpack");
    }

    #[test]
    fn percent_encode_escapes_url_metacharacters() {
        assert_eq!(
            net::percent_encode(r#"[["project_type:modpack"]]"#),
            "%5B%5B%22project_type%3Amodpack%22%5D%5D"
        );
        assert_eq!(net::percent_encode("sodium"), "sodium");
        assert_eq!(net::percent_encode("a b"), "a%20b");
    }

    #[test]
    fn dependencies_deserialize_with_hyphenated_keys() {
        let text = r#"{
            "minecraft": "1.21.1",
            "fabric-loader": "0.16.9",
            "quilt-loader": "0.26.0"
        }"#;
        let deps: Dependencies = serde_json::from_str(text).unwrap();
        assert_eq!(deps.minecraft.as_deref(), Some("1.21.1"));
        assert_eq!(deps.fabric_loader.as_deref(), Some("0.16.9"));
        assert_eq!(deps.quilt_loader.as_deref(), Some("0.26.0"));
        assert!(deps.forge.is_none());
    }

    /// 服务端专用的文件必须能认出来：装到客户端上轻则白占空间，重则进游戏就崩。
    #[test]
    fn server_only_files_are_flagged() {
        let text = r#"[
            {"path":"mods/client.jar","env":{"client":"required","server":"required"}},
            {"path":"mods/server.jar","env":{"client":"unsupported","server":"required"}},
            {"path":"mods/plain.jar"}
        ]"#;
        let files: Vec<IndexFile> = serde_json::from_str(text).unwrap();
        assert!(!files[0].env.client_unsupported());
        assert!(files[1].env.client_unsupported());
        // 没写 env 的文件照常安装
        assert!(!files[2].env.client_unsupported());
        // 大小写不敏感
        let upper: FileEnv = serde_json::from_str(r#"{"client":"UNSUPPORTED"}"#).unwrap();
        assert!(upper.client_unsupported());
    }
}
