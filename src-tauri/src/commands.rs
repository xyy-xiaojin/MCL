//! Tauri IPC 命令层。
//!
//! 这一层只做三件事：参数校验、调用 [`crate::engine`]、把结果原样交回前端。
//! 任何"下载 / 文件 IO / 哈希 / 启动"的实现都不在这里 —— 前端也无权触碰文件系统。
use tauri::{AppHandle, Manager, State};

/// `/select,` 参数里带引号路径，必须用 raw_arg 才能原样传给 explorer
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt as _;

use crate::engine::auth;
use crate::engine::queue::QueueItem;
use crate::engine::{
    browse, export, imports, install, java, jdk, launch, loader, memory, modpack, mods, skin,
    speedtest,
};
use crate::error::Result;
use crate::state::{AppState, Settings};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVersion {
    pub id: String,
    /// `release` / `snapshot` / `old_beta` …
    pub kind: String,
    pub installed: bool,
}

/// 远端版本清单：附带最新正式版 / 最新快照，前端可以直接打「最新」标记。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionList {
    pub latest_release: String,
    pub latest_snapshot: String,
    pub versions: Vec<RemoteVersion>,
}

#[tauri::command]
pub async fn list_remote_versions(state: State<'_, AppState>) -> Result<VersionList> {
    let manifest = install::fetch_manifest(&state).await?;
    let paths = state.paths.clone();
    let latest = (manifest.latest.release, manifest.latest.snapshot);
    let raw: Vec<(String, String)> = manifest
        .versions
        .into_iter()
        .map(|entry| (entry.id, entry.kind))
        .collect();
    // 800 多个版本 × 2 次 stat 是纯阻塞调用，整批丢进 blocking 池，
    // 否则这一屏列表在拉取期间要占着一个 async worker。
    let versions = tokio::task::spawn_blocking(move || {
        raw.into_iter()
            .map(|(id, kind)| RemoteVersion {
                installed: install::is_installed(&paths, &id),
                id,
                kind,
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| crate::error::Error::msg(format!("读取本地版本失败：{e}")))?;
    Ok(VersionList {
        latest_release: latest.0,
        latest_snapshot: latest.1,
        versions,
    })
}

#[tauri::command(async)]
pub fn list_local_versions(state: State<'_, AppState>) -> Vec<install::LocalVersion> {
    install::list_local(&state)
}

/// 删除一个版本（含它的私有游戏目录）。共享的 libraries / assets 不动。
#[tauri::command(async)]
pub fn delete_version(
    state: State<'_, AppState>,
    id: String,
) -> Result<install::DeleteReport> {
    install::delete(&state, &id)
}

/// 安装版本。下载进度通过 `queue://progress` 事件推送。
#[tauri::command]
pub async fn install_version(
    state: State<'_, AppState>,
    id: String,
) -> Result<install::InstallReport> {
    install::install(&state, &id).await
}

/// 启动游戏。游戏输出通过 `log://line` 事件推送。
///
/// 启动前先把账户的 token 续好（皮肤站模式）、authlib-injector 备齐，
/// 这些都要走网络，不能塞进同步的 launch 里。
#[tauri::command]
pub async fn launch_version(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<launch::LaunchReport> {
    let account = auth::ensure_token(&state).await?;
    let injector = auth::prepare_injector(&state, &account).await?;
    launch::launch(&app, &state, &id, account, injector)
}

#[tauri::command(async)]
pub fn detect_java(state: State<'_, AppState>, force: bool) -> Option<java::JavaInfo> {
    state.java_info(force)
}

/// 自动下载 Java 运行时（Adoptium Temurin JRE，TUNA 镜像优先）。
///
/// 下载进度走 `queue://progress` 事件（下载抽屉里能看到），完成后自动
/// 写进设置里的 Java 路径，之后启动、内存计算都走正常流程。
#[tauri::command]
pub async fn download_java(
    state: State<'_, AppState>,
    major: i64,
) -> Result<java::JavaInfo> {
    jdk::install(&state, major).await
}

/// 正在运行的游戏进程列表（前端显示「结束游戏」按钮用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningGameView {
    pub pid: u32,
    pub version: String,
    pub started_at_ms: u64,
}

#[tauri::command(async)]
pub fn game_status(state: State<'_, AppState>) -> Vec<RunningGameView> {
    state
        .running_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|g| RunningGameView {
            pid: g.pid,
            version: g.version.clone(),
            started_at_ms: g.started_at_ms,
        })
        .collect()
}

/// 强制结束游戏。`version` 为空时结束所有 MCL 拉起的游戏。
#[tauri::command(async)]
pub fn game_kill(state: State<'_, AppState>, version: Option<String>) -> Result<usize> {
    let mut killed = 0usize;
    let targets: Vec<crate::state::RunningGame> = {
        let mut games = state.running_games.lock().unwrap_or_else(|e| e.into_inner());
        let (keep, kill): (Vec<_>, Vec<_>) = games
            .drain(..)
            .partition(|g| version.as_deref().map(|v| g.version != v).unwrap_or(false));
        *games = keep;
        kill
    };
    for game in targets {
        if launch::kill_pid(game.pid) {
            killed += 1;
        }
    }
    Ok(killed)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    let normalized = settings.normalized();
    normalized.save(&state.paths)?;
    if let Ok(mut guard) = state.settings.write() {
        *guard = normalized.clone();
    }
    // 下载源要同步给队列，之后新入队的 URL 才会走新源
    state.queue.set_source(normalized.source.clone());
    Ok(normalized)
}

#[tauri::command]
pub fn queue_snapshot(state: State<'_, AppState>) -> Vec<QueueItem> {
    state.queue.snapshot()
}

#[tauri::command]
pub fn queue_pause(state: State<'_, AppState>) {
    state.queue.pause()
}

#[tauri::command]
pub fn queue_resume(state: State<'_, AppState>) {
    state.queue.resume()
}

#[tauri::command]
pub fn queue_cancel(state: State<'_, AppState>, id: String) {
    state.queue.cancel(&id)
}

#[tauri::command]
pub fn queue_cancel_group(state: State<'_, AppState>, group_id: String) {
    state.queue.cancel_group(&group_id)
}

#[tauri::command]
pub fn queue_retry_failed(state: State<'_, AppState>, id: String) -> bool {
    state.queue.retry_failed(&id)
}

#[tauri::command]
pub fn queue_clear(state: State<'_, AppState>) {
    state.queue.clear_finished()
}

#[tauri::command]
pub async fn search_modpacks(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<modpack::ModpackHit>> {
    modpack::search(&state, &query).await
}

/// 分类浏览 Modrinth。
///
/// `query` 留空 + `index` 传 `downloads` 就是「热门」列表，
/// 点进分类时前端默认就这么请求，不用先搜索。
#[tauri::command]
pub async fn browse_search(
    state: State<'_, AppState>,
    kind: String,
    query: String,
    game_version: String,
    loader: String,
    index: String,
    offset: u32,
) -> Result<Vec<browse::BrowseHit>> {
    browse::search(
        &state,
        &kind,
        &query,
        &game_version,
        &loader,
        &index,
        offset,
    )
    .await
}

/// 安装某个项目：解析兼容版本后入队下载（整合包会等下载完再导入）。
///
/// `target_version` 是当前选中的版本，资源会装进它自己的目录。
#[tauri::command]
pub async fn browse_install(
    state: State<'_, AppState>,
    kind: String,
    project_id: String,
    game_version: String,
    loader: String,
    title: String,
    target_version: Option<String>,
) -> Result<browse::ProjectInstallReport> {
    browse::install(
        &state,
        &kind,
        &project_id,
        &game_version,
        &loader,
        &title,
        target_version.as_deref().unwrap_or(""),
    )
    .await
}

/// 导入整合包（.mrpack 或普通 zip）。会新建一个独立版本。
///
/// 里面可能要联网补装原版与加载器，所以是 async。
#[tauri::command]
pub async fn import_modpack(
    state: State<'_, AppState>,
    path: String,
    base_version: Option<String>,
) -> Result<modpack::ImportReport> {
    modpack::import_any(&state, &path, base_version.as_deref().unwrap_or("")).await
}

/// 安装 Fabric / Quilt 加载器，生成一个新版本。
#[tauri::command]
pub async fn install_loader(
    state: State<'_, AppState>,
    loader: String,
    game_version: String,
    loader_version: Option<String>,
    target_id: Option<String>,
) -> Result<loader::LoaderReport> {
    let kind = loader::Loader::parse(&loader)?;
    loader::install(
        &state,
        kind,
        &game_version,
        loader_version.as_deref(),
        target_id.as_deref(),
    )
    .await
}

/// 列出某个加载器支持的所有版本（给下拉用）。
#[tauri::command]
pub async fn list_loader_versions(
    state: State<'_, AppState>,
    loader: String,
    game_version: String,
) -> Result<Vec<String>> {
    let kind = loader::Loader::parse(&loader)?;
    loader::available(&state, kind, &game_version).await
}

/// 内存方案的完整视图：推荐值、实际生效值、GC 参数都摊开给前端。
///
/// 前端据此显示「检测到 187 个 mod，建议 5.2 GB」，也能在设置页里
/// 把将要传给 JVM 的参数原样列出来，不用猜。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub plan: memory::RamPlan,
    /// 这次启动实际会用的 `-Xmx`
    pub effective_max_mb: u32,
    /// 这次启动实际会用的 `-Xms`
    pub effective_min_mb: u32,
    /// 手动模式下设置里的值，方便前端回填滑块
    pub manual_max_mb: u32,
    /// 建议上限：物理内存的 70%，超过它前端标红
    pub safe_limit_mb: u64,
    pub gc_mode: String,
    pub gc_args: Vec<String>,
}

#[tauri::command(async)]
pub fn memory_plan(state: State<'_, AppState>, version_id: Option<String>) -> MemoryView {
    let settings = state.settings().normalized();
    let optifine = version_id
        .as_deref()
        .map(|id| id.to_ascii_lowercase().contains("optifine"))
        .unwrap_or(false);

    // 没有选中版本时给一个不存在的路径：`recommend` 会对不存在的目录返回 0 个 mod，
    // 结果就是"按原版算"。不能传空 id，那会拼出 `versions//game/mods`。
    let mods_dir = match version_id.as_deref() {
        Some(id) if !id.trim().is_empty() => state.paths.mods(id),
        _ => state.paths.root.join("versions").join("__none__").join("mods"),
    };
    let plan = memory::recommend(&mods_dir, optifine);
    let auto = settings.ram_type != "manual";

    let effective_max_mb = if auto {
        plan.recommend_mb
    } else {
        settings.max_memory_mb
    };
    let effective_min_mb = if auto {
        let half = (effective_max_mb / 2).max(512);
        // 和 launch::resolve_memory 同一套钳制：下限地板放在可用性钳制之前，
        // 否则可用内存不足 640MB 时 "不超过可用 80%" 的守卫会被 max 抬穿
        let available = plan.available_mb as u32;
        if available > 0 {
            half.max(512)
                .min(available * 8 / 10)
                .min(effective_max_mb)
                .max(256)
        } else {
            half.min(effective_max_mb)
        }
    } else {
        settings.min_memory_mb
    };

    // 走缓存：探测结果不会自己变，没必要每次算内存都起一个 java 进程
    let java_major = state.java_info(false).map(|info| info.major).unwrap_or(0);

    MemoryView {
        plan,
        effective_max_mb,
        effective_min_mb,
        manual_max_mb: settings.max_memory_mb,
        safe_limit_mb: memory::total_mb() * 7 / 10,
        gc_mode: settings.gc_mode.clone(),
        gc_args: memory::gc_args(memory::GcMode::parse(&settings.gc_mode), java_major),
    }
}

/// 导出实例为 zip。
///
/// 打包 + 压缩是纯阻塞活（上 GB 的 mod 目录），必须丢进 blocking 池：
/// 直接在 async 函数里跑会占住一个 tokio worker，期间别的命令全在后面排队。
#[tauri::command]
pub async fn export_instance(
    state: State<'_, AppState>,
    options: export::ExportOptions,
    version_id: String,
) -> Result<export::ExportReport> {
    let paths = state.paths.clone();
    tokio::task::spawn_blocking(move || export::export(&paths, &options, &version_id))
        .await
        .map_err(|e| crate::error::Error::msg(format!("导出任务异常：{e}")))?
}

/// 拖拽导入。路径由 Tauri 的拖放事件给出，这里按包内结构分发。
///
/// 返回逐个文件的结果而不是一个笼统的成功/失败：一次拖 5 个文件时，
/// 玩家需要知道哪个进了 mods、哪个被当成了材质包、哪个失败了。
///
/// `target_version` 是当前选中的版本；整合包会新建版本、不用它。
///
/// 返回 `Result` 而不是裸 Vec：导入整合包要联网补原版和加载器，
/// async 命令带 `State<'_>` 引用时 Tauri 要求这个签名。
#[tauri::command]
pub async fn import_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
    target_version: Option<String>,
) -> Result<Vec<imports::ImportOutcome>> {
    Ok(imports::import_files(&state, &paths, target_version.as_deref().unwrap_or("")).await)
}

/// mod 列表。按版本隔离，所以要传当前版本。
#[tauri::command(async)]
pub fn list_mods(state: State<'_, AppState>, version_id: String) -> Vec<mods::ModEntry> {
    if version_id.trim().is_empty() {
        return Vec::new();
    }
    mods::list(&state.paths.mods(&version_id))
}

#[tauri::command(async)]
pub fn toggle_mod(
    state: State<'_, AppState>,
    version_id: String,
    file_name: String,
) -> Result<bool> {
    if version_id.trim().is_empty() {
        return Err(crate::error::Error::msg("请先选择一个版本"));
    }
    mods::toggle(&state.paths.mods(&version_id), &file_name)
}

/// 测两个下载源的实时速度。前端据此提示"官方 480ms / BMCLAPI 120ms"。
#[tauri::command]
pub async fn test_sources(
    state: State<'_, AppState>,
) -> Result<Vec<speedtest::SourceProbe>> {
    Ok(speedtest::test_all(&state.client).await)
}


/// 检查 MCL 数据目录是否可写。只创建并立即删除一个小型临时文件，不会修改玩家存档。
#[tauri::command]
pub fn check_data_directory(state: State<'_, AppState>) -> serde_json::Value {
    let root = &state.paths.root;
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(root)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let probe = root.join(format!(".mcl-write-check-{}-{stamp}.tmp", std::process::id()));
        std::fs::write(&probe, b"MCL write check")?;
        std::fs::remove_file(probe)?;
        Ok(())
    })();
    serde_json::json!({
        "ok": result.is_ok(),
        "path": root.to_string_lossy(),
        "message": result.err().map(|e| e.to_string()).unwrap_or_else(|| "数据目录可以正常写入".to_string()),
    })
}

/// 在系统文件管理器中打开一个目录。
///
/// 设置页「目录结构」里每行一个「打开」按钮，玩家点一下就能跳到对应文件夹。
/// 纯 `std::process::Command`，不引入额外插件：
/// - Windows：`explorer <dir>`（打开并定位，目录无需存在）
/// - macOS：`open <dir>`
/// - Linux：`xdg-open <dir>`
#[tauri::command(async)]
pub fn reveal_in_explorer(path: String) -> Result<()> {
    let dir = path.trim();
    if dir.is_empty() {
        return Err(crate::error::Error::msg("路径为空"));
    }
    let target = std::path::Path::new(dir);
    // 传文件路径时是"打开所在文件夹并选中它"（崩溃日志的入口就是这么调的），
    // 传目录时保持原语义"打开这个目录"。
    let is_file = target.is_file();
    #[cfg(target_os = "windows")]
    {
        // `explorer` 对不存在的目录会报错，先确保存在再开，体验更顺
        if let Some(parent) = target.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        // explorer.exe 即使成功打开目录也常以退出码 1 结束（它的返回值不可信），
        // 所以这里只保证能拉起进程，不等待、也不检查退出码。
        if is_file {
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{}\"", dir))
                .spawn()?;
        } else {
            if !target.exists() {
                std::fs::create_dir_all(target)?;
            }
            std::process::Command::new("explorer").arg(dir).spawn()?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let dir = if is_file {
            target.parent().unwrap_or(target)
        } else {
            target
        };
        let status = std::process::Command::new("open").arg(dir).status()?;
        if !status.success() {
            return Err(crate::error::Error::msg("打开文件管理器失败"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let dir = if is_file {
            target.parent().unwrap_or(target)
        } else {
            target
        };
        let status = std::process::Command::new("xdg-open").arg(dir).status()?;
        if !status.success() {
            return Err(crate::error::Error::msg("打开文件管理器失败"));
        }
    }
    Ok(())
}

/// 玩家头像：查一次皮肤并回 data URL；离线 / 没皮肤时交给前端用内置 Steve/Alex。
///
/// 皮肤站账户直接用角色的 UUID 走皮肤站的 sessionserver（uuid 参数被忽略），
/// 离线 / 正版账号保持原有行为。不做成错误——头像拉不到不该让界面报错。
#[tauri::command]
pub async fn skin_avatar(
    state: State<'_, AppState>,
    name: Option<String>,
    uuid: Option<String>,
) -> Result<skin::Avatar> {
    let account = auth::Account::load(&state.paths);
    if account.active() {
        // 皮肤站走它自己的 sessionserver；微软正版走官方端点（server_url 为空）
        let session_api = if account.kind == auth::KIND_AUTHLIB {
            Some(account.server_url.as_str())
        } else {
            None
        };
        return Ok(skin::resolve(
            &state,
            &account.profile_name,
            Some(&account.profile_id),
            session_api,
        )
        .await);
    }
    let name = name.unwrap_or_else(|| state.settings().username);
    Ok(skin::resolve(&state, &name, uuid.as_deref(), None).await)
}

/// 导入一张离线皮肤。
///
/// `data` 是前端 `<input type="file">` 读出来的 data URL（也接受裸 base64）。
/// 走 IPC 传 base64 而不是传路径，是因为浏览器沙箱里拿不到真实文件路径——
/// 而一张 64×64 的皮肤只有几 KB，base64 的开销可以忽略。
#[tauri::command]
pub fn import_skin(state: State<'_, AppState>, data: String) -> Result<skin::Avatar> {
    let bytes = skin::decode_skin_data(&data)?;
    skin::save_local_skin(&state, &bytes)
}

/// 恢复默认皮肤（删掉导入的 PNG，头像回到内置 Steve）。
#[tauri::command]
pub fn reset_skin(state: State<'_, AppState>) -> Result<()> {
    skin::reset_local_skin(&state)
}

/// 把目录暴露给前端做展示（设置页显示"游戏目录在哪"）。
#[tauri::command(async)]
pub fn app_paths(state: State<'_, AppState>, version_id: Option<String>) -> serde_json::Value {
    let current = version_id.unwrap_or_default();
    let isolated = !current.trim().is_empty();
    // mods 是按版本隔离的（versions/<id>/game/mods），没选版本时给空串，
    // 前端据此显示"先选一个版本"，而不是 undefined。
    let mods = if isolated {
        state.paths.mods(&current).to_string_lossy().to_string()
    } else {
        String::new()
    };
    serde_json::json!({
        "root": state.paths.root.to_string_lossy(),
        "versions": state.paths.versions().to_string_lossy(),
        "libraries": state.paths.libraries().to_string_lossy(),
        "assets": state.paths.assets().to_string_lossy(),
        "instances": state.paths.instances().to_string_lossy(),
        "mods": mods,
        // 有选中版本时显示这个版本的私有目录，没选就显示版本根目录
        "game": if isolated {
            state.paths.instance_game(&current).to_string_lossy().to_string()
        } else {
            state.paths.versions().to_string_lossy().to_string()
        },
    })
}

// ---------------------------------------------------------------------------
// 联机
// ---------------------------------------------------------------------------

use crate::engine::netplay::{self, easytier, frp, portmap, Channel, Slot};

/// 联机页一次性要的全部状态。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetplayView {
    pub easytier: Slot,
    pub portmap: Slot,
    pub frp: Slot,
    /// 已经存过访问密钥（前端据此决定要不要显示登录表单）
    pub has_token: bool,
    pub last_tunnel: Option<i64>,
    /// 随包分发的组件是否齐全；缺哪个就报哪个名字
    pub missing: Vec<String>,
    /// 当前进程有没有管理员权限（EasyTier 建网卡需要）
    pub is_admin: bool,
}

#[tauri::command(async)]
pub fn netplay_status(app: AppHandle) -> NetplayView {
    let creds = frp::Credentials::load(&app);
    let [easytier, portmap, frp] = app
        .try_state::<AppState>()
        .map(|s| s.netplay.all())
        .unwrap_or_default();

    let mut missing = Vec::new();
    for name in ["easytier-core.exe", "easytier-cli.exe", "frpc.exe"] {
        if !netplay::tool_dir(&app).join(name).is_file() {
            missing.push(name.to_string());
        }
    }

    NetplayView {
        easytier,
        portmap,
        frp,
        has_token: !creds.token.is_empty(),
        last_tunnel: creds.last_tunnel,
        missing,
        is_admin: netplay::is_admin(),
    }
}

/// 用管理员权限重启自己（EasyTier 需要）。返回是否成功拉起新实例。
#[tauri::command(async)]
pub fn netplay_relaunch_admin() -> bool {
    netplay::relaunch_as_admin()
}

/// 停掉一条通路（前端点"断开"）。
#[tauri::command(async)]
pub fn netplay_stop(app: AppHandle, channel: String) {
    let Some(channel) = channel_of(&channel) else {
        return;
    };
    if let Some(state) = app.try_state::<AppState>() {
        state.netplay.stop(channel);
    }
    netplay::publish(&app, channel, Slot::default());
}

/// 本机在局域网里的地址 + 游戏端口是否可以直连。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanInfo {
    /// 同 WiFi 下的朋友填这个；拿不到就是空串
    pub lan_ip: String,
    /// 本地端口通不通（游戏是不是已经开了局域网）
    pub port_open: bool,
}

#[tauri::command(async)]
pub fn netplay_lan_info(local_port: Option<u16>) -> LanInfo {
    let port = local_port.unwrap_or(25565);
    LanInfo {
        lan_ip: netplay::lan_ip().unwrap_or_default(),
        port_open: netplay::port_open(port),
    }
}

#[tauri::command]
pub async fn netplay_easytier_start(
    app: AppHandle,
    req: easytier::RoomRequest,
) -> Result<Slot> {
    easytier::start(app, req).await
}

#[tauri::command]
pub async fn netplay_portmap_start(app: AppHandle, req: portmap::MapRequest) -> Result<Slot> {
    portmap::start(app, req).await
}

/// 用已保存的密钥拉隧道列表（进入联机页时自动调用）。
#[tauri::command]
pub async fn netplay_frp_login(app: AppHandle) -> Result<Vec<frp::Tunnel>> {
    let creds = frp::Credentials::load(&app);
    if creds.token.is_empty() {
        return Ok(Vec::new());
    }
    frp::list_tunnels(&app, &creds.token).await
}

/// 存新密钥并立刻验证。
#[tauri::command]
pub async fn netplay_frp_save_token(
    app: AppHandle,
    token: String,
) -> Result<Vec<frp::Tunnel>> {
    frp::save_token(app, token).await
}

#[tauri::command]
pub async fn netplay_frp_start(
    app: AppHandle,
    tunnel_id: i64,
    local_port: u16,
) -> Result<Slot> {
    frp::start(app, tunnel_id, local_port).await
}

fn channel_of(name: &str) -> Option<Channel> {
    match name {
        "easytier" => Some(Channel::Easytier),
        "portmap" => Some(Channel::Portmap),
        "frp" => Some(Channel::Frp),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 皮肤站账户（authlib-injector / yggdrasil）
// ---------------------------------------------------------------------------

/// 当前账户（脱敏）。前端据此渲染「离线 / 皮肤站」两张卡。
#[tauri::command(async)]
pub fn auth_status(state: State<'_, AppState>) -> auth::AccountView {
    auth::Account::load(&state.paths).view()
}

/// 验证皮肤站地址可用，回元数据（名字等）。自定义地址的「测试连接」用。
#[tauri::command]
pub async fn auth_server_info(
    state: State<'_, AppState>,
    server_url: String,
) -> Result<auth::ServerInfo> {
    auth::server_info(&state.client, &server_url).await
}

/// 登录。`profiles` 非空表示账号下有多个角色，前端让玩家选完调
/// [`auth_select_profile`]；为空即已完成登录。
#[tauri::command]
pub async fn auth_login(
    state: State<'_, AppState>,
    server_url: String,
    email: String,
    password: String,
) -> Result<auth::LoginOutcome> {
    auth::login(&state.client, &state.paths, &server_url, &email, &password).await
}

/// 多角色登录的收尾：把选中的角色补进已保存的会话。
#[tauri::command(async)]
pub fn auth_select_profile(
    state: State<'_, AppState>,
    profile_id: String,
    profile_name: String,
) -> Result<auth::AccountView> {
    auth::select_profile(&state.paths, &profile_id, &profile_name)
}

/// 登出：作废 token（尽力而为）并回到离线模式。
#[tauri::command]
pub async fn auth_logout(state: State<'_, AppState>) -> Result<()> {
    auth::logout(&state.client, &state.paths).await
}

// ---------------------------------------------------------------------------
// 微软正版登录（设备码流程）
// ---------------------------------------------------------------------------

/// 发起设备码登录：前端展示 user_code，让玩家去 verification_uri 输入。
#[tauri::command]
pub async fn auth_ms_begin(state: State<'_, AppState>) -> Result<auth::DeviceCodeStart> {
    auth::ms_begin(&state.client).await
}

/// 轮询一次设备码。`state: "pending"` 表示继续轮，`"done"` 即登录完成。
#[tauri::command]
pub async fn auth_ms_poll(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<auth::MsPoll> {
    auth::ms_poll(&state.client, &state.paths, &device_code).await
}

/// 用系统默认浏览器打开一个网址（设备码登录的验证页）。
/// 与 reveal_in_explorer 同一套做法：拉起进程即算成功，不检查退出码。
#[tauri::command(async)]
pub fn open_in_browser(url: String) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        return Err(crate::error::Error::msg("网址为空"));
    }
    #[cfg(target_os = "windows")]
    {
        // explorer.exe 打开 URL 会走默认浏览器
        std::process::Command::new("explorer").arg(url).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}
