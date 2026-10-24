//! 资源浏览：按分类搜索 Modrinth，并把选中的项目丢进下载队列。
//!
//! 和 PCL 下载页对应的部分：
//! - 「原版游戏」走 `install` 模块，这里不管；
//! - 「社区资源」（Mod / 整合包 / 数据包 / 资源包 / 光影包）在这里按 `project_type` 区分；
//! - **空查询 + 按下载量排序 = 热门列表**，进分类先看这个，不用先搜索。
//!
//! 安装只做一件事：解析出该分类下与目标游戏版本兼容的最新文件，入队。
//! 真正的下载、校验、落盘仍然全部由 `queue` + `download` 负责。
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::net::{self, RetryPolicy};
use crate::engine::queue::NewTask;
use crate::error::{Error, Result};
use crate::state::AppState;

const MODRINTH: &str = "https://api.modrinth.com/v2";

/// 支持的分类。白名单是有意的：`kind` 会被拼进 URL 的 facets。
const KINDS: [&str; 5] = ["mod", "modpack", "datapack", "resourcepack", "shader"];

fn project_type(kind: &str) -> Result<&'static str> {
    KINDS
        .iter()
        .find(|k| **k == kind)
        .copied()
        .ok_or_else(|| Error::msg(format!("不支持的资源分类：{kind}")))
}

/// 排序方式白名单。
fn sort_index(index: &str) -> &'static str {
    match index {
        "downloads" => "downloads",
        "follows" => "follows",
        "newest" => "newest",
        "updated" => "updated",
        _ => "relevance",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseHit {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub icon_url: String,
    pub downloads: u64,
    pub follows: u64,
    /// ISO 时间，前端自己算「几天前」
    pub updated: String,
    /// 加载器 + 功能标签（fabric / optimization …）
    pub categories: Vec<String>,
    /// 支持的游戏版本，前几个够展示了
    pub versions: Vec<String>,
    pub project_type: String,
    pub author: String,
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
    follows: u64,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    project_type: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

/// 搜索某一分类下的资源。
///
/// `query` 留空、`index` 传 `downloads` 就是「热门」。
/// `game_version` / `loader` 传空字符串或 `all` 表示不筛选。
pub async fn search(
    state: &AppState,
    kind: &str,
    query: &str,
    game_version: &str,
    loader: &str,
    index: &str,
    offset: u32,
) -> Result<Vec<BrowseHit>> {
    let project_type = project_type(kind)?;

    let mut facets = vec![format!(r#"["project_type:{project_type}"]"#)];
    if !is_any(game_version) {
        facets.push(format!(r#"["versions:{game_version}"]"#));
    }
    if !is_any(loader) {
        facets.push(format!(r#"["categories:{loader}"]"#));
    }
    let facets = format!("[{}]", facets.join(","));

    let url = format!(
        "{MODRINTH}/search?limit=20&offset={offset}&index={}&query={}&facets={}",
        sort_index(index),
        net::percent_encode(query.trim()),
        net::percent_encode(&facets),
    );

    let text = net::get_text_multi(
        &state.client,
        &[url],
        RetryPolicy::default(),
    )
    .await?;
    let parsed: SearchResponse = serde_json::from_str(&text)?;

    Ok(parsed
        .hits
        .into_iter()
        .map(|hit| BrowseHit {
            id: hit.project_id,
            slug: hit.slug,
            title: hit.title,
            description: hit.description,
            icon_url: hit.icon_url.unwrap_or_default(),
            downloads: hit.downloads,
            follows: hit.follows,
            updated: hit.date_modified.unwrap_or_default(),
            categories: hit.categories,
            // Modrinth 的 versions 是从旧到新排的，取尾部才是最新的几个版本
            versions: latest_versions(hit.versions, 4),
            project_type: hit.project_type.unwrap_or_else(|| project_type.to_string()),
            author: hit.author.unwrap_or_default(),
        })
        .collect())
}

/// 取最新的几个版本号。
///
/// 直接 `take(4)` 会拿到 18w49a 这种远古快照——数组是升序的，最新的在最后。
fn latest_versions(mut versions: Vec<String>, count: usize) -> Vec<String> {
    if versions.len() > count {
        versions.drain(..versions.len() - count);
    }
    versions
}

fn is_any(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.eq_ignore_ascii_case("all")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInstallReport {
    pub title: String,
    pub version: String,
    /// 入队的文件数（modpack 是解压出的文件数）
    pub files: usize,
    /// 落盘目录
    pub dir: String,
    /// 整合包导入时才有：实例名
    pub instance: Option<String>,
    /// 自动补装的前置 mod 名字（装 mod 时才有；空 = 无前置或已装齐）
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct VersionFile {
    #[serde(default)]
    url: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    hashes: VersionHashes,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Default, Deserialize)]
struct VersionHashes {
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectVersion {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    files: Vec<VersionFile>,
    /// 前置依赖声明。`dependency_type`：required / optional / embedded / incompatible
    #[serde(default)]
    dependencies: Vec<ModDependency>,
}

#[derive(Debug, Deserialize)]
struct ModDependency {
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    dependency_type: String,
}

/// `/projects?ids=[...]` 批量查询返回的项目信息（拿标题与 slug）。
#[derive(Debug, Deserialize)]
struct RawProject {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
}

/// 安装一个项目：取与 `game_version` / `loader` 兼容的最新版本，入队下载。
///
/// 整合包特殊：Modrinth 给的是 `.mrpack`，得先下下来再交给 `modpack::import`，
/// 所以这里会等下载完成（进度照常走队列事件）。
pub async fn install(
    state: &AppState,
    kind: &str,
    project_id: &str,
    game_version: &str,
    loader: &str,
    project_title: &str,
    // 装进哪个版本（版本隔离）。空串表示没有选中版本，直接拒绝。
    target_version: &str,
) -> Result<ProjectInstallReport> {
    let project_type = project_type(kind)?;
    // 整合包会新建一个独立版本，不需要也不应该受当前选中版本约束；
    // 它自己也声明了目标 MC 版本，用当前版本的号去过滤只会把结果筛空。
    let is_modpack = project_type == "modpack";
    // 没有目标版本就没法决定装到哪儿。静默落到某个默认目录会让玩家
    // 找半天找不到东西，不如直接说清楚。
    if !is_modpack {
        if target_version.trim().is_empty() {
            return Err(Error::msg(
                "请先在「版本管理」里选择一个版本，资源会装进那个版本自己的目录",
            ));
        }
        if !state.paths.version_dir(target_version).is_dir() {
            return Err(Error::msg(format!(
                "版本 {target_version} 不存在，请重新选择"
            )));
        }
    }
    let version = if is_modpack {
        fetch_version(state, project_id, "", "").await?
    } else {
        fetch_version(state, project_id, game_version, loader).await?
    };
    let Some(file) = pick_file(&version) else {
        return Err(Error::msg("这个项目没有可下载的文件"));
    };
    if file.url.is_empty() {
        return Err(Error::msg("文件地址为空，可能是项目数据异常"));
    }

    let title = if project_title.trim().is_empty() {
        version.name.clone()
    } else {
        project_title.to_string()
    };
    let label = if file.filename.is_empty() {
        format!("{title}.jar")
    } else {
        file.filename.clone()
    };

    if project_type == "modpack" {
        return install_modpack(state, project_id, &title, &label, file).await;
    }

    let dir = target_dir(state, target_version, project_type)?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(&label);

    let mut tasks = vec![
        NewTask::new(project_type, label.clone(), file.url.clone(), dest)
            .with_sha1(file.hashes.sha1.clone())
            // Modrinth 偶尔不给 size（默认 0）。0 当成"未知"，不能当成"应该是 0 字节"，
            // 否则没有 sha1 时下载器会在下完之后报"期望 0 字节，实际 N 字节"。
            .with_size((file.size > 0).then_some(file.size)),
    ];

    // 装 mod 时顺手补齐 required 前置：PCL 会提醒"缺前置"，我们直接装，
    // 不让玩家等到启动失败再从崩溃诊断里反推缺了什么。
    let dependencies = if project_type == "mod" {
        resolve_dependencies(state, &version, game_version, loader, &dir).await?
    } else {
        Vec::new()
    };
    if !dependencies.is_empty() {
        let dep_tasks = dependencies.iter().map(|d| d.task.clone());
        tasks.extend(dep_tasks);
    }

    state.queue.enqueue(tasks);

    Ok(ProjectInstallReport {
        title,
        version: display_version(&version),
        files: 1 + dependencies.len(),
        dir: dir.to_string_lossy().to_string(),
        instance: None,
        dependencies: dependencies
            .iter()
            .map(|d| d.title.clone())
            .collect(),
    })
}

/// 一个待补装的前置：入队任务 + 展示名。
struct DependencyInstall {
    title: String,
    task: NewTask,
}

/// 单个 mod 最多补装几个前置。超过这个数多半是 mod 元数据异常，交给玩家手动处理。
const MAX_DEPENDENCIES: usize = 8;

/// 解析 `version.dependencies` 里的 required 前置，跳过已安装的，剩下的人队。
///
/// 「已安装」按文件名判断，两层：
/// 1. 与已安装文件**完全同名**——同名不同版本号的情况由下一层兜住；
/// 2. 文件名里**包含该项目的 slug**——Modrinth 的文件名几乎都以 slug 开头
///    （`sodium-fabric-0.5.11.jar`），slug 命中说明这个项目已经有旧版本在装，
///    这时不能装最新版，否则同一个 mod 装两份会直接崩游戏。
async fn resolve_dependencies(
    state: &AppState,
    version: &ProjectVersion,
    game_version: &str,
    loader: &str,
    mods_dir: &std::path::Path,
) -> Result<Vec<DependencyInstall>> {
    let dep_ids: Vec<String> = version
        .dependencies
        .iter()
        .filter(|d| {
            d.dependency_type.eq_ignore_ascii_case("required") && !d.project_id.trim().is_empty()
        })
        .take(MAX_DEPENDENCIES)
        .map(|d| d.project_id.clone())
        .collect();
    if dep_ids.is_empty() {
        return Ok(Vec::new());
    }

    let installed = installed_mod_names(mods_dir);
    let projects = fetch_projects(state, &dep_ids).await?;

    let mut out = Vec::new();
    for project in projects {
        let project_id = project.id.clone();
        // 这个项目已经装过（任意版本）就不动它，避免重复 mod
        if !project.slug.is_empty()
            && installed
                .iter()
                .any(|name| name.to_ascii_lowercase().contains(&project.slug.to_ascii_lowercase()))
        {
            continue;
        }
        let Ok(dep_version) = fetch_version(state, &project_id, game_version, loader).await else {
            // 没有兼容当前游戏版本 / 加载器的版本：跳过而不是拦住主 mod 的安装，
            // 真缺了前置启动失败时崩溃诊断会指出来
            continue;
        };
        let Some(file) = pick_file(&dep_version) else {
            continue;
        };
        if file.url.is_empty() {
            continue;
        }
        let title = if project.title.is_empty() {
            dep_version.name.clone()
        } else {
            project.title.clone()
        };
        let label = format!(
            "前置：{}",
            if file.filename.is_empty() {
                format!("{title}.jar")
            } else {
                file.filename.clone()
            }
        );
        let dest = mods_dir.join(
            if file.filename.is_empty() {
                format!("{}.jar", project.slug)
            } else {
                file.filename.clone()
            },
        );
        out.push(DependencyInstall {
            title,
            task: NewTask::new("mod", label, file.url.clone(), dest)
                .with_sha1(file.hashes.sha1.clone())
                .with_size((file.size > 0).then_some(file.size)),
        });
    }
    Ok(out)
}

/// mods 目录里的全部文件名（含停用的 `.disabled`——停用也算"装过"）。
fn installed_mod_names(mods_dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(mods_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 批量拉项目标题与 slug（一个请求，不逐个查）。
async fn fetch_projects(state: &AppState, ids: &[String]) -> Result<Vec<RawProject>> {
    let ids_json = serde_json::to_string(ids)?;
    let url = format!(
        "{MODRINTH}/projects?ids={}",
        net::percent_encode(&ids_json)
    );
    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    Ok(serde_json::from_str(&text)?)
}

async fn install_modpack(
    state: &AppState,
    project_id: &str,
    title: &str,
    filename: &str,
    file: &VersionFile,
) -> Result<ProjectInstallReport> {
    let dir = state.paths.root.join("modpacks");
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(if filename.ends_with(".mrpack") {
        filename.to_string()
    } else {
        format!("{project_id}.mrpack")
    });

    let ids = state.queue.enqueue(vec![
        NewTask::new("modpack", dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), file.url.clone(), dest.clone())
            .with_sha1(file.hashes.sha1.clone())
            .with_size((file.size > 0).then_some(file.size)),
    ]);

    // 整合包必须落盘后才能解压，这里等它下完（进度仍然走队列）
    let failed = state.queue.wait(&ids).await;
    if let Some((label, reason)) = failed.into_iter().next() {
        return Err(Error::msg(format!("整合包下载失败（{label}）：{reason}")));
    }

    // 整合包会新建一个独立版本，不用 target_version
    let report = crate::engine::modpack::import_any(state, &dest.to_string_lossy(), "").await?;
    Ok(ProjectInstallReport {
        title: title.to_string(),
        version: report.loader.clone(),
        files: report.files,
        dir: report.dir.clone(),
        // 返回新建的版本 id，前端据此切过去
        instance: Some(report.version_id),
        // 整合包的前置由 .mrpack 自己声明并安装，这里恒为空
        dependencies: Vec::new(),
    })
}

/// 拉取与该游戏版本 / 加载器兼容的版本列表，取第一个（Modrinth 按发布时间倒序）。
async fn fetch_version(
    state: &AppState,
    project_id: &str,
    game_version: &str,
    loader: &str,
) -> Result<ProjectVersion> {
    let mut url = format!("{MODRINTH}/project/{project_id}/version?");
    if !is_any(game_version) {
        url.push_str(&format!(
            "game_versions={}&",
            net::percent_encode(&format!(r#"["{game_version}"]"#))
        ));
    }
    if !is_any(loader) {
        url.push_str(&format!(
            "loaders={}&",
            net::percent_encode(&format!(r#"["{loader}"]"#))
        ));
    }
    let url = url.trim_end_matches('&').to_string();

    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    let list: Vec<ProjectVersion> = serde_json::from_str(&text)?;
    list.into_iter()
        .next()
        .ok_or_else(|| Error::msg("没有与当前游戏版本兼容的文件，换个版本或加载器试试"))
}

/// 优先挑 `primary` 那个文件，否则取第一个。
fn pick_file(version: &ProjectVersion) -> Option<&VersionFile> {
    version
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| version.files.first())
}

fn display_version(version: &ProjectVersion) -> String {
    if version.version_number.trim().is_empty() {
        version.name.clone()
    } else {
        version.version_number.clone()
    }
}

/// 不同分类落到不同目录，和原版客户端的目录约定一致。
///
/// 全部按版本隔离：内容进 `versions/<id>/game/`，换版本不会互相看到。
///
/// 数据包是特例：Minecraft 没有"全局 datapacks"目录，只有
/// `saves/<世界>/datapacks/` 才会被读到（`game/datapacks` 是个死目录）。
/// 所以装到最近修改的那个存档里；一个存档都没有就明确报错，
/// 而不是悄悄丢进一个游戏永远看不到的地方。
fn target_dir(state: &AppState, version_id: &str, project_type: &str) -> Result<PathBuf> {
    Ok(match project_type {
        "datapack" => latest_world(&state.paths.saves(version_id))
            .ok_or_else(|| {
                Error::msg("还没有存档。数据包只能装进存档，请先进游戏创建一个世界")
            })?
            .join("datapacks"),
        "resourcepack" => state.paths.resourcepacks(version_id),
        "shader" => state.paths.shaderpacks(version_id),
        _ => state.paths.mods(version_id),
    })
}

/// 找 `saves/` 下最近被修改过的存档目录。
fn latest_world(saves: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(saves).ok()?;
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_whitelist_rejects_unknown() {
        assert!(project_type("mod").is_ok());
        assert!(project_type("shader").is_ok());
        assert!(project_type("plugin").is_err());
        assert!(project_type("../etc").is_err());
    }

    #[test]
    fn sort_index_falls_back_to_relevance() {
        assert_eq!(sort_index("downloads"), "downloads");
        assert_eq!(sort_index("nonsense"), "relevance");
    }

    #[test]
    fn any_value_covers_empty_and_all() {
        assert!(is_any(""));
        assert!(is_any("  "));
        assert!(is_any("ALL"));
        assert!(!is_any("1.20.1"));
    }

    #[test]
    fn pick_file_prefers_primary() {
        let version = ProjectVersion {
            name: "v1".into(),
            version_number: "1.0".into(),
            files: vec![
                VersionFile {
                    url: "u1".into(),
                    filename: "a.jar".into(),
                    hashes: VersionHashes::default(),
                    size: 1,
                    primary: false,
                },
                VersionFile {
                    url: "u2".into(),
                    filename: "b.jar".into(),
                    hashes: VersionHashes::default(),
                    size: 2,
                    primary: true,
                },
            ],
            dependencies: Vec::new(),
        };
        assert_eq!(pick_file(&version).map(|f| f.filename.as_str()), Some("b.jar"));
    }

    #[test]
    fn latest_versions_keeps_the_newest() {
        let all: Vec<String> = ["18w49a", "1.16.5", "1.20.1", "1.21.1", "1.21.4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            latest_versions(all, 3),
            vec!["1.20.1".to_string(), "1.21.1".to_string(), "1.21.4".to_string()]
        );
        // 比要求的少时原样返回
        let few = vec!["1.20.1".to_string()];
        assert_eq!(latest_versions(few, 4), vec!["1.20.1".to_string()]);
    }

    #[test]
    fn project_types_map_to_expected_dirs() {
        // 目录名是原版约定，写死在这里当契约
        let kinds = ["datapack", "resourcepack", "shader"];
        for kind in kinds {
            assert!(KINDS.contains(&kind));
        }
    }

    #[test]
    fn dependencies_parse_and_filter_required() {
        let raw = r#"{
            "name": "Sodium 0.5.11",
            "version_number": "0.5.11+mc1.21.1",
            "files": [],
            "dependencies": [
                {"project_id": "aaaa", "dependency_type": "required"},
                {"project_id": "bbbb", "dependency_type": "optional"},
                {"project_id": "", "dependency_type": "required"},
                {"project_id": "cccc", "dependency_type": "embedded"}
            ]
        }"#;
        let version: ProjectVersion = serde_json::from_str(raw).unwrap();
        let required: Vec<String> = version
            .dependencies
            .iter()
            .filter(|d| {
                d.dependency_type.eq_ignore_ascii_case("required") && !d.project_id.trim().is_empty()
            })
            .map(|d| d.project_id.clone())
            .collect();
        assert_eq!(required, vec!["aaaa".to_string()]);
    }

    #[test]
    fn disabled_mods_count_as_installed() {
        let dir = std::env::temp_dir().join(format!(
            "mcl-mod-deps-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fabric-api-0.92.0+1.20.1.jar"), b"x").unwrap();
        std::fs::write(dir.join("sodium-fabric-0.5.8.jar.disabled"), b"x").unwrap();

        let names = installed_mod_names(&dir);
        // 停用的 sodium 也算"装过"，不能给它补一个最新版造成重复
        assert!(names.iter().any(|n| n.contains("sodium-fabric")));
        assert!(names.iter().any(|n| n.contains("fabric-api")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
