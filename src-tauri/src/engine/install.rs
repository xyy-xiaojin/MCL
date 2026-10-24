//! 安装：拉元数据 → 生成下载清单 → 交给全局队列 → 解压 natives。
//!
//! 下载本身完全复用 [`crate::engine::queue`]，所以"下载页"能看到安装过程中的每一个文件，
//! 也能整体暂停。安装只是"算清楚要下什么，然后等队列跑完"。
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::meta::{
    self, AssetIndex, Platform, VersionJson, VersionManifest,
};
use crate::engine::net;
use crate::engine::queue::NewTask;
use crate::error::{Error, Result};
use crate::state::AppState;

/// natives 压缩包及其解压黑名单。
#[derive(Clone)]
struct NativeJar {
    jar: PathBuf,
    exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub version: String,
    pub files: usize,
    pub bytes: u64,
    pub natives: usize,
    /// 老版本（1.6 及更早）按原名铺开的资源数量；新版本恒为 0
    pub legacy: usize,
}

/// 版本清单缓存多久算过期。
///
/// 清单本身 200–400 KB，跨越国境链路要一两秒。而"最近版本"一天最多变一两次，
/// 30 分钟完全够用。之前每次进版本页 / 每次官方源装版本都要重拉一遍。
const MANIFEST_TTL_SECS: u64 = 30 * 60;

/// 拉版本清单。带本地缓存，缓存失效或读不出来才走网络。
pub async fn fetch_manifest(state: &AppState) -> Result<VersionManifest> {
    let cache = state.paths.cache().join("version_manifest.json");
    if let Some(manifest) = read_cached_manifest(&cache) {
        return Ok(manifest);
    }

    let url = format!("{}/mc/game/version_manifest.json", state.base_url());
    let text = net::get_text(&state.client, &url).await?;
    let manifest: VersionManifest = serde_json::from_str(&text)?;

    // 写缓存失败不影响主流程（比如目录只读），只是下次还得联网
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&cache, &text);
    Ok(manifest)
}

fn read_cached_manifest(path: &std::path::Path) -> Option<VersionManifest> {
    let meta = std::fs::metadata(path).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age.as_secs() > MANIFEST_TTL_SECS {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 拉单个版本的 version.json 原文。
async fn fetch_version_text(state: &AppState, id: &str) -> Result<String> {
    let source = state.source();
    let url = if source == "official" {
        // 官方源没有 /version/<id>/json，必须走清单里的 url
        let manifest = fetch_manifest(state).await?;
        manifest
            .versions
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.url.clone())
            .ok_or_else(|| Error::msg(format!("版本清单里找不到 {id}")))?
    } else {
        format!("{}/version/{}/json", net::MIRROR, id)
    };
    net::get_text(&state.client, &url).await
}

/// 安装（或修复）一个原版版本。
pub async fn install(state: &AppState, id: &str) -> Result<InstallReport> {
    if !meta::is_safe_id(id) {
        return Err(Error::msg(format!("版本 ID 不合法：{id}")));
    }
    let platform = Platform::current();
    let source = state.source();

    // 1) 元数据落盘：后续启动完全依赖这份本地 JSON，不再联网
    let text = fetch_version_text(state, id).await?;
    let version: VersionJson = serde_json::from_str(&text)
        .map_err(|e| Error::msg(format!("version.json 解析失败：{e}")))?;
    if !meta::is_safe_id(&version.id) {
        return Err(Error::msg("version.json 里的 id 不合法"));
    }
    let vdir = state.paths.version_dir(&version.id);
    std::fs::create_dir_all(&vdir)?;
    std::fs::write(vdir.join(format!("{}.json", version.id)), &text)?;

    // 2) 客户端 jar + 类库 + 资源索引
    // 一次安装的两批下载（游戏文件、资源对象）共用同一个任务组：
    // 前端下载抽屉里它们折叠成一条「安装 Minecraft <id>」，整组取消也是一次到位。
    let group_id = state.queue.alloc_group();
    let group_label = format!("安装 Minecraft {}", version.id);
    let (mut tasks, natives) = plan(&version, state, &platform, &source)?;
    for task in &mut tasks {
        task.set_group(&group_id, &group_label);
    }
    let mut bytes: u64 = tasks.iter().filter_map(|t| t.size).sum();
    let mut files = tasks.len();
    // 磁盘预检：总大小清单里就有。512 MB 余量给 natives 解压、日志和临时文件。
    // 之前磁盘不够会一路下到写满才失败，玩家看到的还是一条莫名其妙的"写入失败"。
    crate::engine::disk::ensure_free(
        &state.paths.root,
        bytes + 512 * 1024 * 1024,
        &format!("版本 {}", version.id),
    )?;
    let failures = enqueue_and_wait(state, tasks).await?;
    if !failures.is_empty() {
        return Err(Error::msg(format!(
            "{} 个文件下载失败：{}",
            failures.len(),
            failures.join("；")
        )));
    }

    // 3) 资源对象（需要先读回索引，才知道有哪些哈希）+ 老版本布局
    let mut legacy = 0usize;
    if let Some(index) = load_asset_index(state, &version).await? {
        let mut asset_tasks = asset_object_tasks(&index, state, &source);
        bytes += asset_tasks.iter().filter_map(|t| t.size).sum::<u64>();
        files += asset_tasks.len();
        if !asset_tasks.is_empty() {
            for task in &mut asset_tasks {
                task.set_group(&group_id, &group_label);
            }
            // 资源对象是大头（几百 MB 到几 GB），入队前再查一次：
            // 客户端下完的这段时间里磁盘可能又被别的东西写满了
            crate::engine::disk::ensure_free(
                &state.paths.root,
                asset_tasks.iter().filter_map(|t| t.size).sum::<u64>()
                    + 512 * 1024 * 1024,
                "游戏资源",
            )?;
            let failed = enqueue_and_wait(state, asset_tasks).await?;
            if !failed.is_empty() {
                return Err(Error::msg(format!(
                    "{} 个资源文件下载失败：{}",
                    failed.len(),
                    failed.join("；")
                )));
            }
        }

        // 1.6 及更早：索引里会带 virtual / map_to_resources，
        // 要求把 objects 里的内容按原始文件名再铺一份，否则没音效没语言文件。
        if let Some(index_ref) = &version.asset_index {
            // 几千次硬链接 / 拷贝，放 blocking 池里跑
            let assets = state.paths.assets();
            let game = state.paths.instance_game(&version.id);
            let index_id = index_ref.id.clone();
            legacy = tokio::task::spawn_blocking(move || {
                materialize_legacy(&index, &assets, &game, &index_id)
            })
            .await
            .map_err(|e| Error::msg(format!("展开旧版资源失败：{e}")))??;
        }
    }

    // 4) natives
    {
        let natives_dir = state.paths.natives_dir(&version.id);
        let jars = natives.clone();
        tokio::task::spawn_blocking(move || extract_natives(&jars, &natives_dir))
            .await
            .map_err(|e| Error::msg(format!("解压原生库失败：{e}")))??;
    }

    Ok(InstallReport {
        version: version.id,
        files,
        bytes,
        natives: natives.len(),
        legacy,
    })
}

/// 把任务入队并等它跑完，返回失败项（"标签：原因"）。
async fn enqueue_and_wait(state: &AppState, tasks: Vec<NewTask>) -> Result<Vec<String>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let ids = state.queue.enqueue(tasks);
    let failures = state.queue.wait(&ids).await;
    Ok(failures
        .into_iter()
        .map(|(label, reason)| format!("{label}：{reason}"))
        .collect())
}

/// 拼装下载清单。
fn plan(
    version: &VersionJson,
    state: &AppState,
    platform: &Platform,
    source: &str,
) -> Result<(Vec<NewTask>, Vec<NativeJar>)> {
    let paths = &state.paths;
    let mut tasks = Vec::new();
    let mut natives = Vec::new();

    // 客户端 jar
    let client_dest = paths.version_dir(&version.id).join(format!("{}.jar", version.id));
    let client = version
        .downloads
        .as_ref()
        .and_then(|d| d.get("client"));
    let official_url = client.and_then(|c| c.url.clone());
    // 镜像的客户端 jar 不在 /v1/objects/ 下，走专门的 /version/<id>/client 端点，
    // 所以不能靠 rewrite 推，得单独拼；两边互为备份。
    let mirror_url = {
        let url = net::client_endpoint("bmclapi", &version.id);
        if url.is_empty() { None } else { Some(url) }
    };
    let (client_url, client_fallbacks) = if source == "official" {
        (official_url.clone(), mirror_url.clone())
    } else {
        (
            mirror_url.or_else(|| official_url.clone()),
            official_url.clone(),
        )
    };
    if let Some(url) = client_url {
        let fallbacks: Vec<String> = client_fallbacks
            .into_iter()
            .filter(|f| *f != url)
            .collect();
        tasks.push(
            NewTask::new("client", format!("{}.jar", version.id), url, client_dest)
                .with_fallbacks(fallbacks)
                .with_sha1(client.and_then(|c| c.sha1.clone()))
                .with_size(client.and_then(|c| c.size)),
        );
    } else {
        return Err(Error::msg(
            "version.json 里没有客户端下载地址（downloads.client）",
        ));
    }

    // 资源索引
    if let Some(index) = &version.asset_index {
        if let Some(url) = &index.url {
            tasks.push(
                NewTask::from_official(
                    "asset-index",
                    format!("assets/{}.json", index.id),
                    url,
                    source,
                    paths.assets().join("indexes").join(format!("{}.json", index.id)),
                )
                .with_sha1(index.sha1.clone())
                // 一定要用 size：totalSize 是索引里全部资源的总量（几百 MB），
                // 拿它当文件大小会让下载器永远判定"没下完"。
                .with_size(index.size),
            );
        }
    }

    // 类库与原生库
    for library in &version.libraries {
        if !meta::allowed(library.rules.as_deref(), platform) {
            continue;
        }

        if !meta::is_native_only(library, platform) {
            if let Some(rel) = meta::library_path(library) {
                let dest = paths.libraries().join(meta::rel_path(&rel));
                let artifact = library.downloads.as_ref().and_then(|d| d.artifact.as_ref());
                let url = artifact
                    .and_then(|a| a.url.clone())
                    .or_else(|| repo_url(library, &rel));
                // 少数老类库只给坐标不给仓库：没有 url 就跳过下载，但仍然进 classpath
                if let Some(url) = url {
                    tasks.push(
                        NewTask::from_official(
                            "library",
                            short_name(&library.name),
                            &url,
                            source,
                            dest,
                        )
                        .with_sha1(artifact.and_then(|a| a.sha1.clone()))
                        .with_size(artifact.and_then(|a| a.size)),
                    );
                }
            }
        }

        // 原生库
        let Some(classifier) = meta::natives_classifier(library, platform) else {
            continue;
        };
        let artifact = library
            .downloads
            .as_ref()
            .and_then(|d| d.classifiers.get(&classifier));
        let rel = artifact
            .and_then(|a| a.path.clone())
            .or_else(|| meta::maven_to_path(&format!("{}:{}", library.name, classifier)));
        let Some(rel) = rel else { continue };
        let dest = paths.libraries().join(meta::rel_path(&rel));
        let url = artifact
            .and_then(|a| a.url.clone())
            .or_else(|| repo_url(library, &rel));
        // 既没有下载地址、本地也没有现成文件的 native 库进了解压清单的话，
        // 会在下载全部成功后的最后一步 extract_natives 整体报错——不如跳过。
        if url.is_some() || dest.is_file() {
            if let Some(url) = url {
                tasks.push(
                    NewTask::from_official(
                        "native",
                        short_name(&library.name),
                        &url,
                        source,
                        dest.clone(),
                    )
                    .with_sha1(artifact.and_then(|a| a.sha1.clone()))
                    .with_size(artifact.and_then(|a| a.size)),
                );
            }
            natives.push(NativeJar {
                jar: dest,
                exclude: library
                    .extract
                    .as_ref()
                    .map(|e| e.exclude.clone())
                    .unwrap_or_default(),
            });
        }
    }

    Ok((tasks, natives))
}

fn repo_url(library: &meta::Library, rel: &str) -> Option<String> {
    let base = library.url.as_ref()?.trim_end_matches('/');
    Some(format!("{base}/{rel}"))
}

/// `org.lwjgl:lwjgl:3.3.1` → `lwjgl 3.3.1`，队列里显示得下。
fn short_name(coordinate: &str) -> String {
    let parts: Vec<&str> = coordinate.split(':').collect();
    match (parts.get(1), parts.get(2)) {
        (Some(artifact), Some(version)) => format!("{artifact} {version}"),
        _ => coordinate.to_string(),
    }
}

/// 读回资源索引（几百 KB 的 JSON，3598 个对象）。
///
/// 读文件 + 解析都在 blocking 池里做：`serde_json` 解析这个尺寸的索引要十几毫秒，
/// 挂在 async worker 上就是白占一个线程。
async fn load_asset_index(state: &AppState, version: &VersionJson) -> Result<Option<AssetIndex>> {
    let Some(index_ref) = &version.asset_index else {
        return Ok(None);
    };
    let path = state
        .paths
        .assets()
        .join("indexes")
        .join(format!("{}.json", index_ref.id));
    tokio::task::spawn_blocking(move || -> Result<Option<AssetIndex>> {
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&text)?))
    })
    .await
    .map_err(|e| Error::msg(format!("读取资源索引失败：{e}")))?
}

fn asset_object_tasks(index: &AssetIndex, state: &AppState, source: &str) -> Vec<NewTask> {
    index
        .objects
        .values()
        .filter_map(|object| {
            let dest = meta::asset_object_path(&state.paths.assets(), &object.hash)?;
            let hash = object.hash.to_ascii_lowercase();
            // 先取出前缀：直接写 &hash[..2] 再 move hash 会触发借用检查
            let prefix = hash[..2].to_string();
            // 官方地址是规范的，镜像那条由 endpoints 推出来
            let official = format!("{}/{prefix}/{hash}", net::OFFICIAL_RESOURCES);
            Some(
                NewTask::from_official(
                    "asset",
                    format!("asset {}", &hash[..8]),
                    &official,
                    source,
                    dest,
                )
                .with_sha1(Some(hash))
                .with_size(Some(object.size)),
            )
        })
        .collect()
}

/// 老版本资源布局：把内容寻址的对象按原始文件名铺开。
///
/// - `virtual: true` → `assets/virtual/<索引>/<逻辑名>`（1.6 左右）
/// - `map_to_resources: true` → `<game>/resources/<逻辑名>`（更早）
///
/// 优先硬链接：同一份内容在两个位置各出现一次，硬链接不占额外空间；
/// 跨盘或文件系统不支持时退回复制。返回实际铺开的文件数。
fn materialize_legacy(
    index: &AssetIndex,
    assets: &Path,
    game: &Path,
    index_id: &str,
) -> Result<usize> {
    let want_virtual = index.is_virtual.unwrap_or(false);
    let want_resources = index.map_to_resources.unwrap_or(false);
    if !want_virtual && !want_resources {
        return Ok(0);
    }

    // 版本隔离：resources 属于这个版本自己的工作目录
    std::fs::create_dir_all(game)?;
    let mut count = 0usize;

    for (name, object) in &index.objects {
        let Some(source) = meta::asset_object_path(assets, &object.hash) else {
            continue;
        };
        if !source.is_file() {
            continue;
        }
        // 逻辑名同样不能逃出目标目录（索引来自网络）
        let Some(relative) = safe_relative(name) else {
            continue;
        };
        if want_virtual {
            count += link_or_copy(&source, &assets.join("virtual").join(index_id).join(&relative))?;
        }
        if want_resources {
            count += link_or_copy(&source, &game.join("resources").join(&relative))?;
        }
    }
    Ok(count)
}

/// 把索引里的逻辑名转成安全的相对路径。
fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains('\\')
            || part.contains(':')
        {
            return None;
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn link_or_copy(source: &Path, dest: &Path) -> Result<usize> {
    if dest.is_file() {
        return Ok(0);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::hard_link(source, dest) {
        Ok(()) => Ok(1),
        Err(_) => {
            std::fs::copy(source, dest)?;
            Ok(1)
        }
    }
}

/// 解压 natives：先在同级暂存目录里完整解压，成功后再整体替换。
fn extract_natives(natives: &[NativeJar], out_dir: &Path) -> Result<()> {
    if natives.is_empty() {
        return Ok(());
    }
    let parent = out_dir
        .parent()
        .ok_or_else(|| Error::msg("natives 目录没有父目录"))?;
    std::fs::create_dir_all(parent)?;
    let staged = parent.join(format!(
        ".natives-stage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)?;

    for native in natives {
        if !native.jar.is_file() {
            return Err(Error::msg(format!(
                "缺少原生库文件：{}",
                native.jar.display()
            )));
        }
        let file = std::fs::File::open(&native.jar)?;
        let mut archive = zip::ZipArchive::new(file)?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            // enclosed_name 会拒绝 ../ 逃逸，恶意压缩包也写不出去
            let Some(relative) = entry.enclosed_name() else {
                continue;
            };
            let normalized = relative.to_string_lossy().replace('\\', "/");
            // 版本 JSON 里声明的黑名单（通常是 META-INF/）
            if native.exclude.iter().any(|p| normalized.starts_with(p)) {
                continue;
            }
            let target = staged.join(relative);
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut out = std::fs::File::create(target)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }

    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)?;
    }
    std::fs::rename(&staged, out_dir)?;
    Ok(())
}

/// 已安装的版本列表。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalVersion {
    pub id: String,
    pub kind: String,
    pub java_major: i64,
    pub dir: String,
    /// json 与 jar 都在 —— 否则说明装了一半，不能直接启动
    pub complete: bool,
    /// 占用的磁盘字节数（版本目录 + 实例游戏目录）
    pub size: u64,
    /// 实例私有游戏目录里有没有存档 —— 删之前必须让玩家知道
    pub has_saves: bool,
}

/// 递归统计目录大小。算不出来就返回 0，不影响功能。
fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_file() => meta.len(),
                Ok(meta) if meta.is_dir() => dir_size(&path),
                _ => 0,
            }
        })
        .sum()
}

/// 删除结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    pub version: String,
    /// 释放的字节数
    pub freed: u64,
    /// 实际删掉的目录，方便在提示里说清楚
    pub removed: Vec<String>,
}

/// 删除一个版本及其私有游戏目录。
///
/// 只删 `versions/<id>/`，不碰 libraries / assets —— 那些是多个版本共享的，
/// 删了会连累别的版本（真要清理缓存是另一个功能）。
pub fn delete(state: &AppState, id: &str) -> Result<DeleteReport> {
    let id = id.trim();
    if id.is_empty()
        || id.contains("..")
        || id.contains('/')
        || id.contains('\\')
        || id.contains(':')
    {
        return Err(Error::msg(format!("版本名不合法：{id}")));
    }

    let versions_root = state.paths.versions();
    let dir = versions_root.join(id);
    // 双保险：拼出来的路径必须真的落在 versions/ 下
    if !dir.starts_with(&versions_root) {
        return Err(Error::msg("版本目录越界"));
    }
    if !dir.is_dir() {
        return Err(Error::msg(format!("版本 {id} 不存在")));
    }

    let mut removed = Vec::new();
    let mut freed = dir_size(&dir);
    std::fs::remove_dir_all(&dir)?;
    removed.push(dir.to_string_lossy().to_string());

    // 整合包导入时还会在 instances/<id>/ 写一份 instance.json 记录，
    // 版本删了记录也该跟着走，不然留下指向已删版本的孤儿元数据。
    // 删不掉（被占用之类）就先留着，不影响版本本身的删除。
    let instance_dir = state.paths.instances().join(id);
    if instance_dir.is_dir() {
        freed += dir_size(&instance_dir);
        if std::fs::remove_dir_all(&instance_dir).is_ok() {
            removed.push(instance_dir.to_string_lossy().to_string());
        }
    }

    Ok(DeleteReport {
        version: id.to_string(),
        freed,
        removed,
    })
}

/// 把一个版本目录读成一条展示记录；缺 json 的目录返回 `None`。
fn describe(state: &AppState, root: &Path, dir: &Path) -> Option<LocalVersion> {
    let id = dir.file_name()?.to_string_lossy().to_string();
    let json = dir.join(format!("{id}.json"));
    if !json.is_file() {
        return None;
    }
    // Fabric / Quilt / 整合包的版本目录是 inheritsFrom 薄壳，jar 在底层
    // 父版本目录里，所以必须沿继承链找（和 launch.rs 保持同一套规则）。
    let merged = meta::load_merged_with_chain(root, &id).ok();
    let version = merged.as_ref().map(|(v, _)| v);
    let has_jar = match &merged {
        Some((_, chain)) => chain
            .iter()
            .any(|v| root.join(v).join(format!("{v}.jar")).is_file()),
        // 元数据坏了就退回最保守的判断
        None => dir.join(format!("{id}.jar")).is_file(),
    };
    let saves = state.paths.saves(&id);
    Some(LocalVersion {
        java_major: version.map(required_java_major).unwrap_or(8),
        kind: version
            .and_then(|v| v.version_type.clone())
            .unwrap_or_else(|| "release".to_string()),
        dir: dir.to_string_lossy().to_string(),
        // 只出 json 没有 jar 说明装了一半，用标记而不是改 id ——
        // id 是要回传给后端的，拼上后缀会让启动阶段找不到版本目录
        complete: has_jar,
        size: dir_size(dir),
        has_saves: saves.is_dir()
            && std::fs::read_dir(&saves)
                .map(|d| d.flatten().next().is_some())
                .unwrap_or(false),
        id,
    })
}

/// 已安装版本列表。
///
/// 每个版本目录都要递归统计大小（`saves` / `logs` 动辄上千文件），
/// 这些目录彼此独立，所以按版本切成几份并行扫——版本一多，
/// 串行扫描就是启动首屏最明显的等待点。
pub fn list_local(state: &AppState) -> Vec<LocalVersion> {
    let root = state.paths.versions();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    if dirs.is_empty() {
        return Vec::new();
    }

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(dirs.len());
    let chunk = dirs.len().div_ceil(workers);
    let mut out: Vec<LocalVersion> = Vec::with_capacity(dirs.len());
    let root_ref = &root;
    std::thread::scope(|scope| {
        let handles: Vec<_> = dirs
            .chunks(chunk)
            .map(|slice| {
                scope.spawn(move || {
                    slice
                        .iter()
                        .filter_map(|dir| describe(state, root_ref, dir))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            if let Ok(list) = handle.join() {
                out.extend(list);
            }
        }
    });
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// 版本要求的 Java 主版本。
pub fn required_java_major(version: &VersionJson) -> i64 {
    if let Some(major) = version
        .java_version
        .as_ref()
        .and_then(|j| j.major_version)
        .filter(|m| *m > 0)
    {
        return major;
    }
    // 老版本没有 javaVersion 字段，按版本号推
    let mut parts = version.id.split('.');
    let _ = parts.next();
    match parts
        .next()
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
    {
        17 => 16,
        18.. => 17,
        _ => 8,
    }
}

/// 判断某版本是否已完整安装（json + jar 都在）。
///
/// 只要 [`Paths`]：版本清单有 800 多条，这个判断要能整批丢进 blocking 池里跑，
/// 不能拖着 `AppState` 走。
pub fn is_installed(paths: &crate::state::Paths, id: &str) -> bool {
    let dir = paths.version_dir(id);
    dir.join(format!("{id}.json")).is_file() && dir.join(format!("{id}.jar")).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_trims_group() {
        assert_eq!(short_name("org.lwjgl:lwjgl:3.3.1"), "lwjgl 3.3.1");
        assert_eq!(short_name("weird"), "weird");
    }

    #[test]
    fn java_major_uses_metadata_then_version_number() {
        let with_meta: VersionJson =
            serde_json::from_str(r#"{"id":"1.20.1","javaVersion":{"majorVersion":17}}"#).unwrap();
        assert_eq!(required_java_major(&with_meta), 17);

        let legacy: VersionJson = serde_json::from_str(r#"{"id":"1.12.2"}"#).unwrap();
        assert_eq!(required_java_major(&legacy), 8);

        let modern: VersionJson = serde_json::from_str(r#"{"id":"1.20.4"}"#).unwrap();
        assert_eq!(required_java_major(&modern), 17);
    }

    #[test]
    fn natives_extraction_skips_excluded_prefixes() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "mcl-natives-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let jar = root.join("natives.jar");
        {
            let file = std::fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::FileOptions::default();
            zip.start_file("lwjgl.dll", options).unwrap();
            zip.write_all(b"binary").unwrap();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(b"Manifest-Version: 1.0").unwrap();
            zip.finish().unwrap();
        }
        let out = root.join("natives");
        extract_natives(
            &[NativeJar {
                jar,
                exclude: vec!["META-INF/".to_string()],
            }],
            &out,
        )
        .unwrap();

        assert!(out.join("lwjgl.dll").is_file());
        assert!(!out.join("META-INF/MANIFEST.MF").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
