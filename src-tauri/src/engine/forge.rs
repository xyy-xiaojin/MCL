//! Forge / NeoForge 安装：跑官方 installer.jar 的无头模式。
//!
//! 这两个加载器没有 Fabric 那种「meta 服务直接给 version.json」的待遇——
//! 现代 Forge（1.13+）的客户端类要靠 installer 里的处理器对原版 jar 打二进制补丁，
//! 这一步没法（也不值得）自己重新实现。所以做法是业界通用的：
//!
//! 1. 确保原版已安装（installer 要读 `versions/<v>/<v>.jar` 并校验哈希）；
//! 2. 确保 Java 可用（没有就触发 [`crate::engine::jdk`] 自动下载——
//!    这正好闭环了「小白拿到手什么都没有」的场景）；
//! 3. 下载 installer.jar：Forge 走 **BMCLAPI 优先**（`/forge/download/<build>`，
//!    302 到自家 maven 镜像），官方 maven 兜底；NeoForge 只有官方 maven；
//! 4. `java -jar installer --installClient <root>`，让官方安装器自己干活；
//! 5. 对比 `versions/` 前后差异找出它新建的版本目录，
//!    需要「归化」成我们自己的版本 id 时（整合包导入）把目录改名 + 改写 JSON 的 id。
//!
//! 之后启动照常走 `inheritsFrom` 合并链，客户端 jar / natives 沿链解析——
//! 这些 0.01 就实现了，Forge 版本不需要任何特殊照顾。
//!
//! 已知边界：Forge 只支持 1.13+（更早的 universal.jar 时代装法完全不同）；
//! NeoForge 只支持 1.20.2+（1.20.1 的 NeoForge 用的是旧的 net.neoforged.forge 坐标）；
//! NeoForge 的 maven 在国内可达性差，下载失败时会明确提示。
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use tauri::Emitter;

use crate::engine::install;
use crate::engine::loader::Loader;
use crate::engine::meta;
use crate::engine::net::{self, RetryPolicy};
use crate::engine::queue::NewTask;
use crate::engine::{jdk, meta::VersionJson};
use crate::error::{Error, Result};
use crate::state::AppState;

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";
/// NeoForge 1.20.1 及更早没有 net.neoforged.neoforge 坐标，明确拒绝而不是给一个装不上的版本。
const NEOFORGE_MIN_GAME: (i64, i64) = (20, 2);

/// BMCLAPI 的 Forge 构建条目（`GET /forge/minecraft/<游戏版本>`）。
#[derive(Debug, Clone, Deserialize)]
struct ForgeBuild {
    #[serde(default)]
    build: u64,
    #[serde(default)]
    version: String,
    #[serde(default)]
    mcversion: String,
    #[serde(default)]
    files: Vec<ForgeFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ForgeFile {
    #[serde(default)]
    format: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    hash: String,
}

impl ForgeBuild {
    /// installer jar 的 SHA-1（BMCLAPI 列表里有；官方列表不带，交给安装器自己校验）。
    fn installer_sha1(&self) -> Option<String> {
        self.files
            .iter()
            .find(|f| f.category == "installer" && f.format == "jar")
            .map(|f| f.hash.clone())
            .filter(|h| !h.is_empty())
    }
}

/// 某个游戏版本的 Forge 构建列表（按 build 降序，最新在前）。
async fn forge_builds(state: &AppState, game_version: &str) -> Result<Vec<ForgeBuild>> {
    let url = format!(
        "{}/forge/minecraft/{}",
        net::MIRROR,
        net::percent_encode(game_version)
    );
    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    let mut list: Vec<ForgeBuild> = serde_json::from_str(&text)
        .map_err(|e| Error::msg(format!("Forge 版本列表解析失败：{e}")))?;
    list.retain(|b| !b.version.is_empty() && b.mcversion == game_version);
    list.sort_by_key(|b| std::cmp::Reverse(b.build));
    if list.is_empty() {
        return Err(Error::msg(format!("Forge 没有支持 {game_version} 的版本")));
    }
    Ok(list)
}

/// NeoForge 版本列表：解析 maven-metadata.xml，按「NeoForge 主.次 ↔ MC 1.主.次」过滤。
async fn neoforge_versions(state: &AppState, game_version: &str) -> Result<Vec<String>> {
    let (major, minor) = mc_major_minor(game_version)?;
    if (major, minor) < NEOFORGE_MIN_GAME {
        return Err(Error::msg(
            "NeoForge 只支持 1.20.2 及以上（1.20.1 的 NeoForge 请改用 Forge）",
        ));
    }
    let url = format!("{NEOFORGE_MAVEN}/net/neoforged/neoforge/maven-metadata.xml");
    let text = net::get_text_multi(&state.client, &[url], RetryPolicy::default()).await?;
    let mut out: Vec<String> = regex::Regex::new(r"<version>([^<]+)</version>")
        .unwrap()
        .captures_iter(&text)
        .filter_map(|c| c.get(1).map(|v| v.as_str().to_string()))
        .filter(|v| v.starts_with(&format!("{major}.{minor}.")))
        .collect();
    // metadata 通常已按发布顺序排列，这里再排一次保证"最后一个 = 最新"
    out.sort_by(|a, b| neoforge_version_key(b).cmp(&neoforge_version_key(a)));
    if out.is_empty() {
        return Err(Error::msg(format!(
            "NeoForge 没有支持 {game_version} 的版本"
        )));
    }
    Ok(out)
}

/// `1.20.4` → (20, 4)。不认识的版本号直接报错。
///
/// 不能用 `trim_start_matches('1')`：它会把 "1.21" 剥成 ".21"，连前导 1 都剥光。
fn mc_major_minor(game_version: &str) -> Result<(i64, i64)> {
    let rest = game_version
        .strip_prefix("1.")
        .ok_or_else(|| bad_version(game_version))?;
    let mut parts = rest.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad_version(game_version))?;
    let minor = parts
        .next()
        .unwrap_or("0")
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad_version(game_version))?;
    Ok((major, minor))
}

fn bad_version(game_version: &str) -> Error {
    Error::msg(format!("不认识的游戏版本号：{game_version}"))
}

/// NeoForge 版本号的排序键：`21.1.228` → (21, 1, 228, 正式版)。
/// 第四位让平手的 `-beta` / `-rc` 排在正式版后面。
fn neoforge_version_key(version: &str) -> (i64, i64, i64, bool) {
    let mut nums = [0i64; 3];
    for (i, part) in version.split('.').take(3).enumerate() {
        nums[i] = part
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }
    (nums[0], nums[1], nums[2], !version.contains('-'))
}

/// 某个游戏版本可装的加载器版本列表（前端下拉 / 自动挑最新用）。
pub async fn available(state: &AppState, loader: Loader, game_version: &str) -> Result<Vec<String>> {
    match loader {
        Loader::Forge => Ok(forge_builds(state, game_version)
            .await?
            .into_iter()
            .map(|b| b.version)
            .collect()),
        Loader::NeoForge => neoforge_versions(state, game_version).await,
        _ => Err(Error::msg("这个加载器不走 installer 流程")),
    }
}

/// 安装 Forge / NeoForge，生成一个可启动的新版本。
///
/// `loader_version` 支持 `47.2.0` 和 `1.20.1-47.2.0` 两种写法（.mrpack 的
/// forge 依赖写的是带游戏版本前缀的完整 id，这里顺手归一化）。
/// `target_id` 给出时把安装结果归化成这个名字（整合包导入要用），否则保留
/// 官方安装器生成的 id（`1.20.1-47.2.0`）。
pub async fn install(
    state: &AppState,
    loader: Loader,
    game_version: &str,
    loader_version: Option<&str>,
    target_id: Option<&str>,
) -> Result<crate::engine::loader::LoaderReport> {
    let game_version = game_version.trim();
    if !meta::is_safe_id(game_version) {
        return Err(Error::msg(format!("游戏版本 ID 不合法：{game_version}")));
    }
    let is_neoforge = loader == Loader::NeoForge;

    // 0) 幂等：目标版本已存在就直接用（重复点按钮、或上次装完 json 已在）
    let target_id = target_id.map(str::trim).filter(|v| !v.is_empty());
    if let Some(target) = target_id {
        if !meta::is_safe_id(target) {
            return Err(Error::msg(format!("版本 ID 不合法：{target}")));
        }
        if state.paths.version_dir(target).join(format!("{target}.json")).is_file() {
            return Ok(crate::engine::loader::LoaderReport {
                version_id: target.to_string(),
                loader: loader.as_str().to_string(),
                loader_version: loader_version.unwrap_or_default().to_string(),
                game_version: game_version.to_string(),
                libraries: 0,
                warning: None,
            });
        }
    }

    // 1) 原版必须就位：installer 会读 versions/<v>/<v>.jar 并可能校验哈希
    if !install::is_installed(&state.paths, game_version) {
        install::install(state, game_version).await?;
    }

    // 2) 选版本 + 拿下载地址
    let (loader_version, installer_urls, sha1) = resolve_installer(
        state,
        is_neoforge,
        game_version,
        loader_version,
    )
    .await?;

    // 3) Java：installer 自己也是 Java 程序。检测到的 Java 够新就直接用，
    //    否则触发自动下载——朋友的裸机走到这里也不会卡死。
    //    java -version 是同步起子进程（130~170ms，杀软下更久），
    //    包进 blocking 池，别占着 async worker。
    let needed = base_java_major(state, game_version).unwrap_or(8);
    let java_path = state.settings().java_path.trim().to_string();
    let detected = tokio::task::spawn_blocking(move || crate::engine::java::detect(&java_path))
        .await
        .map_err(|e| Error::msg(format!("Java 探测任务异常：{e}")))?;
    let info = match detected {
        Some(found) if found.major >= needed => found,
        _ => jdk::install(state, needed).await?,
    };

    // 4) 下载 installer（走全局队列，可见进度、BMCLAPI 优先）
    let installer_path = state
        .paths
        .cache()
        .join("forge")
        .join(format!("{game_version}-{loader_version}-installer.jar"));
    std::fs::create_dir_all(installer_path.parent().unwrap())?;
    let ids = state.queue.enqueue(vec![{
        let mut task = NewTask::new(
            "loader",
            format!("{} {} 安装器", loader.display(), loader_version),
            installer_urls.first().cloned().unwrap_or_default(),
            installer_path.clone(),
        )
        .with_fallbacks(installer_urls.iter().skip(1).cloned().collect());
        if sha1.is_some() {
            task = task.with_sha1(sha1.clone());
        }
        task
    }]);
    let failed = state.queue.wait(&ids).await;
    if let Some((label, reason)) = failed.into_iter().next() {
        let _ = std::fs::remove_file(&installer_path);
        return Err(Error::msg(format!(
            "{} 安装器下载失败（{label}）：{reason}",
            loader.display()
        )));
    }

    // 5) 跑官方安装器（无头模式），前后对比 versions/ 找出它新建的目录
    let root = state.paths.root.clone();
    // installer 会检查目标目录里有没有 launcher_profiles.json（官方启动器的
    // 存在证明），没有就直接拒绝："There is no Minecraft launcher profile"。
    // 这是实测踩到的坑；HMCL / PCL 也是这么补桩的。已有文件绝不覆盖。
    write_profiles_stub(&root, game_version)?;
    let before = version_dirs(&root);
    // 官方产出的 id 是 `{游戏版本}-forge-{forge版本}`（实测 1.20.1 装
    // 47.4.28 得到 `1.20.1-forge-47.4.28`），NeoForge 同理用 neoforge。
    // 前后 diff 和失败清理都只认这个前缀：installer 要跑几分钟，
    // 期间并发装的其他版本也会出现在 diff 里，不能误认/误删。
    let kind = if is_neoforge { "neoforge" } else { "forge" };
    let prefix = format!("{game_version}-{kind}-");
    let installer_for_run = installer_path.clone();
    let root_for_run = root.clone();
    let app_for_run = state.app.clone();
    let loader_name = loader.as_str();
    let run = tokio::task::spawn_blocking(move || {
        run_installer(&app_for_run, std::path::Path::new(&info.path), &installer_for_run, &root_for_run, loader_name)
    })
    .await
    .map_err(|e| Error::msg(format!("安装器任务异常：{e}")));
    // 无论成败都关掉前端的"安装中"面板；成败本身由命令的返回值通知
    let _ = state.app.emit(
        EVENT_INSTALLER_DONE,
        serde_json::json!({ "loader": loader.as_str(), "ok": run.as_ref().map(|r| r.is_ok()).unwrap_or(false) }),
    );
    if let Err(err) = run {
        let _ = std::fs::remove_file(&installer_path);
        // 失败清理：installer 失败前可能已经写下半成品版本目录（json 在、
        // 补丁 jar 不在）。留着的话版本页会把它标成"完整"，点启动就翻车，
        // 重装还会被"已存在"分支误判。只删本游戏版本前缀的新目录，
        // 并发装的其他版本不碰。
        for id in version_dirs(&root) {
            if !before.contains(&id) && id.starts_with(&prefix) {
                let _ = std::fs::remove_dir_all(state.paths.version_dir(&id));
            }
        }
        return Err(err);
    }

    let created = version_dirs(&root)
        .into_iter()
        .filter(|id| !before.contains(id) && id.starts_with(&prefix))
        .find(|id| state.paths.version_dir(id).join(format!("{id}.json")).is_file());
    let created = match created {
        Some(id) => id,
        None => {
            // 安装器正常退出但没建新目录：多半是重复安装。
            let predicted = format!("{prefix}{loader_version}");
            if state
                .paths
                .version_dir(&predicted)
                .join(format!("{predicted}.json"))
                .is_file()
            {
                predicted
            } else {
                let _ = std::fs::remove_file(&installer_path);
                return Err(Error::msg(
                    "安装器没有产出版本目录，可能是版本已被安装过；到「版本管理」刷新看看",
                ));
            }
        }
    };
    let _ = std::fs::remove_file(&installer_path);

    // 6) 归化版本 id：目录改名 + 内部文件改名 + JSON 的 id 字段改写。
    //    启动时按「版本 id 找 <id>.jar」，内部文件不改名会丢掉补丁过的客户端 jar。
    let version_id = match target_id {
        Some(target) if target != created => {
            rehome(&state.paths, &created, target)?;
            target.to_string()
        }
        _ => created,
    };

    Ok(crate::engine::loader::LoaderReport {
        version_id,
        loader: loader.as_str().to_string(),
        loader_version,
        game_version: game_version.to_string(),
        libraries: 0,
        warning: None,
    })
}

/// 决定 installer 下载地址与校验值。
/// 返回（归一化的 loader 版本，候选地址列表 [主, 备…]，BMCLAPI 给的 sha1）。
async fn resolve_installer(
    state: &AppState,
    is_neoforge: bool,
    game_version: &str,
    loader_version: Option<&str>,
) -> Result<(String, Vec<String>, Option<String>)> {
    if is_neoforge {
        let versions = neoforge_versions(state, game_version).await?;
        let version = match loader_version.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => {
                if versions.iter().any(|c| c == v) {
                    v.to_string()
                } else {
                    return Err(Error::msg(format!(
                        "NeoForge {v} 不在支持 {game_version} 的版本列表里"
                    )));
                }
            }
            None => versions
                .first()
                .cloned()
                .ok_or_else(|| Error::msg("没有可用的 NeoForge 版本"))?,
        };
        let url = format!(
            "{NEOFORGE_MAVEN}/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar"
        );
        Ok((version, vec![url], None))
    } else {
        let builds = forge_builds(state, game_version).await?;
        let wanted = loader_version
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.trim_start_matches(&format!("{game_version}-")).to_string());
        let picked = match wanted {
            Some(v) => builds
                .iter()
                .find(|b| b.version == v)
                .cloned()
                .ok_or_else(|| Error::msg(format!("Forge {v} 不在支持 {game_version} 的版本列表里")))?,
            None => builds
                .first()
                .cloned()
                .ok_or_else(|| Error::msg("没有可用的 Forge 版本"))?,
        };
        // BMCLAPI 的 /forge/download/<build> 302 到自家 maven 镜像；
        // 官方 maven 直连兜底（Cloudflare CDN，国内一般可达）。
        let official = format!(
            "{FORGE_MAVEN}/net/minecraftforge/forge/{game_version}-{}/forge-{game_version}-{}-installer.jar",
            picked.version, picked.version
        );
        let bmclapi = format!("{}/forge/download/{}", net::MIRROR, picked.build);
        let urls = if state.source() == "official" {
            vec![official.clone(), bmclapi]
        } else {
            vec![bmclapi, official]
        };
        let sha1 = picked.installer_sha1();
        Ok((picked.version, urls, sha1))
    }
}

/// 原版需要的主 Java 版本（读版本 JSON，读不到按 8 兜底）。
fn base_java_major(state: &AppState, game_version: &str) -> Option<i64> {
    let text = std::fs::read_to_string(
        state
            .paths
            .version_dir(game_version)
            .join(format!("{game_version}.json")),
    )
    .ok()?;
    let json: VersionJson = serde_json::from_str(&text).ok()?;
    Some(install::required_java_major(&json))
}

pub const EVENT_INSTALLER_LOG: &str = "installer://log";
pub const EVENT_INSTALLER_DONE: &str = "installer://done";

/// 安装器实时日志的推送间隔（毫秒）。处理器日志一秒能刷几十行，
/// 不节流的话 IPC 会被打满——120ms 与下载进度的节奏一致。
const LOG_FLUSH_MS: u64 = 120;

/// 无头跑安装器，stdout/stderr **逐行转发**为 `installer://log` 事件。
///
/// 处理器管线要跑几分钟，之前是等 `output()` 全部结束才返回——界面上
/// 只有按钮转圈，朋友会以为卡死了。现在安装的每一行都实时可见，
/// 失败时错误信息带上最后几行，有得查。
fn run_installer(
    app: &tauri::AppHandle,
    java: &Path,
    installer: &Path,
    root: &Path,
    loader_name: &str,
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::atomic::Ordering;

    // 日志事件的 loader 标签必须和 installer://done 的一致：
    // 前端按 loader 匹配配对开始/结束，NeoForge 被标成 forge 的话
    // "正在安装"面板永远等不到自己的结束事件
    let loader_tag = loader_name.to_string();
    let mut command = std::process::Command::new(java);
    command
        .args([
            "-jar",
            &installer.to_string_lossy(),
            "--installClient",
            &root.to_string_lossy(),
        ])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Forge 安装器是 Java 控制台子进程，隐藏窗口但继续采集安装日志。
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .map_err(|e| Error::msg(format!("无法启动安装器（{}）：{e}", java.display())))?;

    // 收集日志尾部：失败时拼进错误信息，让"为什么装不上"有现场可查
    let tail: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

    // 转发：stdout / stderr 各一个读取线程，行进队列；
    // 一个推送线程每 120ms 批量 emit（逐行推会把 IPC 打满）。
    // 剩余读取线程数归零且队列排空时，推送线程收工。
    let pending: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // stdout / stderr 类型不同，先各自包成 Box<dyn Read> 再统一处理
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(child.stdout.take().expect("installer stdout")),
        Box::new(child.stderr.take().expect("installer stderr")),
    ];
    for stream in streams {
        let pending = Arc::clone(&pending);
        let tail = Arc::clone(&tail);
        let remaining = Arc::clone(&remaining);
        remaining.fetch_add(1, Ordering::SeqCst);
        readers.push(std::thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines().map_while(std::result::Result::ok) {
                let line = line.trim_end().to_string();
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(mut tail) = tail.lock() {
                    if tail.len() >= 40 {
                        tail.remove(0);
                    }
                    tail.push(line.clone());
                }
                if let Ok(mut queue) = pending.lock() {
                    queue.push(line);
                }
            }
            remaining.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    let emitter = app.clone();
    let flush = {
        let pending = Arc::clone(&pending);
        let remaining = Arc::clone(&remaining);
        // loader_tag 后面的错误文案还要用，这里只给它一份克隆
        let loader_tag = loader_tag.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(LOG_FLUSH_MS));
            let batch: Vec<String> = pending
                .lock()
                .map(|mut q| q.drain(..).collect())
                .unwrap_or_default();
            for line in &batch {
                let _ = emitter.emit(
                    EVENT_INSTALLER_LOG,
                    serde_json::json!({ "loader": loader_tag, "line": line }),
                );
            }
            if batch.is_empty() && remaining.load(Ordering::SeqCst) == 0 {
                break;
            }
        })
    };

    let status = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
    let _ = flush.join();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            let tail_text = tail.lock().map(|t| t.join("\n")).unwrap_or_default();
            Err(Error::msg(format!(
                "{} 安装器运行失败（退出码 {}）：\n{}",
                loader_tag,
                status.code().unwrap_or(-1),
                if tail_text.is_empty() {
                    "（安装器没有输出）".to_string()
                } else {
                    tail_text
                }
            )))
        }
        Err(e) => Err(Error::msg(format!("等待安装器退出失败：{e}"))),
    }
}

/// 补一个最小可用的 `launcher_profiles.json`（官方安装器的存在证明）。
///
/// 实测没有这个文件 installer 会直接拒绝："There is no Minecraft launcher
/// profile in <dir>, you need to run the launcher first!"。内容只要能反序列化
/// 就行——installer 只读它、不写它。已存在时**绝不覆盖**：用户自己的
/// launcher 数据装在 MCL 目录里的情况（老玩家改过 data_dir）必须保留原样。
fn write_profiles_stub(root: &Path, game_version: &str) -> Result<()> {
    let path = root.join("launcher_profiles.json");
    if path.is_file() {
        return Ok(());
    }
    let stub = serde_json::json!({
        "profiles": {
            "mcl": {
                "name": "MCL",
                "type": "latest-release",
                "created": "2020-01-01T00:00:00.000Z",
                "lastUsed": "2020-01-01T00:00:00.000Z",
                "lastVersionId": game_version,
            }
        },
        "settings": {},
        "launcherVersion": {
            "name": concat!("MCL/", env!("CARGO_PKG_VERSION")),
            "format": 3,
            "profilesFormat": 3,
        },
    });
    std::fs::write(&path, serde_json::to_string_pretty(&stub)?)?;
    Ok(())
}

/// `versions/` 下已有的版本目录名。
fn version_dirs(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join("versions"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 把安装器产出的版本目录归化成我们自己的 id：目录改名 + 内部文件改名 +
/// JSON 的 id 字段改写。启动时按「版本 id 找 `<id>.jar`」，不改名会丢掉
/// 补丁过的客户端 jar。
pub(crate) fn rehome(paths: &crate::state::Paths, created: &str, target: &str) -> Result<()> {
    let from = paths.version_dir(created);
    let to = paths.version_dir(target);
    // target 目录可能已被建出来过（空目录），先把内容并进去
    std::fs::create_dir_all(&to)?;
    for entry in std::fs::read_dir(&from)?.flatten() {
        let dest = to.join(entry.file_name());
        if std::fs::rename(entry.path(), &dest).is_err() {
            // 跨卷（数据目录被链接到别的盘）时退回拷贝
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    std::fs::remove_dir_all(&from)?;

    let json_path = to.join(format!("{created}.json"));
    let target_json = to.join(format!("{target}.json"));
    if json_path.is_file() {
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path)?)?;
        json["id"] = serde_json::Value::String(target.to_string());
        std::fs::write(&target_json, serde_json::to_string_pretty(&json)?)?;
        if json_path != target_json {
            let _ = std::fs::remove_file(&json_path);
        }
    }
    let old_jar = to.join(format!("{created}.jar"));
    if old_jar.is_file() {
        let _ = std::fs::rename(&old_jar, to.join(format!("{target}.jar")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mc_versions_map_to_neoforge_major_minor() {
        assert_eq!(mc_major_minor("1.20.4").unwrap(), (20, 4));
        assert_eq!(mc_major_minor("1.21.1").unwrap(), (21, 1));
        assert_eq!(mc_major_minor("1.21").unwrap(), (21, 0));
        assert!(mc_major_minor("abc").is_err());
    }

    #[test]
    fn loader_versions_are_normalized() {
        // mrpack 的 forge 依赖写全名，列表匹配时要去掉游戏版本前缀
        let v = "1.20.1-47.2.0".trim_start_matches(&format!("{}-", "1.20.1"));
        assert_eq!(v, "47.2.0");
        assert_eq!("47.2.0".trim_start_matches(&format!("{}-", "1.20.1")), "47.2.0");
    }

    #[test]
    fn neoforge_sort_puts_newest_first() {
        let mut list = vec![
            "21.1.9".to_string(),
            "21.1.228-beta".to_string(),
            "21.1.228".to_string(),
            "20.4.10".to_string(),
        ];
        list.sort_by(|a, b| neoforge_version_key(b).cmp(&neoforge_version_key(a)));
        assert_eq!(list[0], "21.1.228");
        assert_eq!(list.last().unwrap(), "20.4.10");
    }

    #[test]
    fn profiles_stub_is_written_once_and_not_overwritten() {
        let root = std::env::temp_dir().join(format!(
            "mcl-forge-profiles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();

        write_profiles_stub(&root, "1.20.1").unwrap();
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("launcher_profiles.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["profiles"]["mcl"]["lastVersionId"], "1.20.1");

        // 用户自己的文件不被覆盖
        std::fs::write(root.join("launcher_profiles.json"), r#"{"profiles":{"x":1}}"#).unwrap();
        write_profiles_stub(&root, "1.21.1").unwrap();
        let again: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("launcher_profiles.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(again["profiles"]["x"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn predicted_id_matches_official_naming() {
        // 实测：1.20.1 装 forge 47.4.28 产出 versions/1.20.1-forge-47.4.28
        let game_version = "1.20.1";
        let loader_version = "47.4.28";
        assert_eq!(
            format!("{game_version}-forge-{loader_version}"),
            "1.20.1-forge-47.4.28"
        );
    }

    #[test]
    fn rehome_renames_dir_files_and_json_id() {
        let root = std::env::temp_dir().join(format!(
            "mcl-forge-rehome-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let paths = crate::state::Paths { root: root.clone() };
        let created_dir = paths.version_dir("1.20.1-47.2.0");
        std::fs::create_dir_all(&created_dir).unwrap();
        std::fs::write(
            created_dir.join("1.20.1-47.2.0.json"),
            r#"{"id":"1.20.1-47.2.0","inheritsFrom":"1.20.1"}"#,
        )
        .unwrap();
        std::fs::write(created_dir.join("1.20.1-47.2.0.jar"), b"patched").unwrap();

        rehome(&paths, "1.20.1-47.2.0", "我的整合包").unwrap();

        let target_dir = paths.version_dir("我的整合包");
        assert!(!paths.version_dir("1.20.1-47.2.0").exists(), "旧目录应当被移走");
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(target_dir.join("我的整合包.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["id"], "我的整合包");
        assert_eq!(json["inheritsFrom"], "1.20.1");
        assert!(target_dir.join("我的整合包.jar").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
