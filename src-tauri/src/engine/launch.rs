//! 启动游戏：拼参数、拉 Java 子进程、把 stdout/stderr 转成事件推给前端。
//!
//! 参数拼装顺序与官方启动器一致：
//! `JVM 参数 → 主类 → 游戏参数`，classpath 里**客户端 jar 在最前**。
//!
//! 游戏输出除了推前端还会**落盘**到 `logs/<版本>/<时间>.log`，
//! 进程退出时对日志尾部做关键词初判，把小白能看懂的中文建议
//! 一起放进 `game://exit` 事件（见 [`analyze_exit`]）。
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::auth;
use crate::engine::install::required_java_major;
use crate::engine::java;
use crate::engine::memory;
use crate::engine::meta::{self, Argument, ArgValue, Platform};
// 头像和启动必须用同一个 UUID，否则游戏里的人和界面上的人对不上
use crate::engine::skin::offline_uuid;
use crate::error::{Error, Result};
use crate::state::{AppState, RunningGame, Settings};

pub const EVENT_LOG: &str = "log://line";
pub const EVENT_GAME_EXIT: &str = "game://exit";

/// 日志文件里保留的行数上限（分析只看尾部，太多反而稀释关键词）。
const TAIL_LINES: usize = 400;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogLine {
    stream: String,
    line: String,
}

/// `game://exit` 事件载荷。崩溃分析的字段是可选的：正常退出时为空。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameExitPayload {
    pub version: String,
    pub code: i64,
    /// 完整日志文件路径（前端"打开日志文件夹"用）
    pub log_path: String,
    /// 崩溃初判：小白能直接照着做的建议
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReport {
    pub pid: u32,
    pub java: String,
    pub java_major: i64,
    pub arg_count: usize,
}

/// 游戏输出的双写目的地：前端事件 + 日志文件。
///
/// `tail` 保留最后 400 行给崩溃分析——关键词（OutOfMemoryError 之类）
/// 通常出现在崩溃瞬间的最后几十行，全量保存没必要。
struct LogSink {
    file: Option<std::sync::Mutex<std::fs::File>>,
    tail: std::sync::Mutex<VecDeque<String>>,
}

impl LogSink {
    fn open(path: &std::path::Path, header: &str) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(|f| {
                use std::io::Write;
                let f = std::sync::Mutex::new(f);
                if let Ok(mut guard) = f.lock() {
                    let _ = guard.write_all(header.as_bytes());
                }
                f
            });
        Self {
            file,
            tail: std::sync::Mutex::new(VecDeque::with_capacity(TAIL_LINES)),
        }
    }

    fn write(&self, line: &str) {
        if let Some(file) = &self.file {
            use std::io::Write;
            if let Ok(mut guard) = file.lock() {
                let _ = guard.write_all(line.as_bytes());
            }
        }
        if let Ok(mut tail) = self.tail.lock() {
            if tail.len() >= TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.to_string());
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

/// 启动指定版本。
///
/// `account` 是已经完成续期的账户（离线或皮肤站）；`injector` 只在皮肤站
/// 模式下有值，带 authlib-injector jar 路径与 yggdrasil 根。
pub fn launch(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    account: auth::Account,
    injector: Option<auth::Injector>,
) -> Result<LaunchReport> {
    // 重复启动防护：连点两次启动，两个游戏抢同一份存档，
    // 是存档损坏的高发原因。同一版本只允许一个实例。
    {
        let running = state
            .running_games
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(game) = running.iter().find(|g| g.version == id) {
            return Err(Error::msg(format!(
                "版本 {id} 已经在运行了（PID {}）。连续开两个会抢同一份存档；要强制结束请用首页的「结束游戏」",
                game.pid
            )));
        }
    }

    let settings = state.settings().normalized();
    let platform = Platform::current();

    // 头像和启动必须用同一个身份：皮肤站 / 微软正版用角色自己的 UUID 和
    // 真实 token，离线账户按名字派生（三套身份的 UUID 不一样，
    // 存档里的玩家数据也不同）
    let real_account =
        account.kind == auth::KIND_AUTHLIB || account.kind == auth::KIND_MICROSOFT;
    let (player_name, player_uuid) = if real_account {
        (account.profile_name.clone(), account.profile_id.clone())
    } else {
        (settings.username.clone(), offline_uuid(&settings.username))
    };

    // 1) 合并继承链，并确定客户端 jar 在哪个版本目录
    let (version, chain) = meta::load_merged_with_chain(&state.paths.versions(), id)?;
    let jar_dir = chain
        .iter()
        .find(|v| state.paths.version_dir(v).join(format!("{v}.jar")).is_file())
        .cloned()
        .unwrap_or_else(|| version.id.clone());
    let client_jar = state
        .paths
        .version_dir(&jar_dir)
        .join(format!("{jar_dir}.jar"));
    if !client_jar.is_file() {
        return Err(Error::msg(format!(
            "版本 {id} 还没有安装完成（缺少客户端 jar），请先在「版本管理」里安装"
        )));
    }

    // 2) Java
    let info = java::detect(&settings.java_path).ok_or_else(|| {
        Error::msg("没有找到 Java。请在「设置 → Java 路径」里手动指定 java 可执行文件")
    })?;
    let needed = required_java_major(&version);
    if info.major > 0 && info.major < needed {
        return Err(Error::msg(format!(
            "版本 {id} 需要 Java {needed}，当前找到的是 Java {}（{}）",
            info.major, info.path
        )));
    }

    // 3) 目录：版本隔离，工作目录就是这个版本自己的 game/
    let game_dir = state.paths.instance_game(&version.id);
    std::fs::create_dir_all(&game_dir)?;
    // natives 也要沿继承链找：Fabric / Quilt / 整合包的版本目录是薄壳，
    // 它们自己没有类库、也没有 natives，`.dll / .so` 是安装原版时解压到
    // `versions/<原版>/natives` 的。只看自己的目录会拿到一个空文件夹，
    // `-Djava.library.path` 指过去就是
    // `UnsatisfiedLinkError: Failed to locate library: lwjgl.dll`。
    let natives = resolve_natives(&state.paths, &chain, &version.id);
    std::fs::create_dir_all(&natives)?;
    let assets_dir = state.paths.assets();

    // 4) classpath：客户端 jar 在最前，纯原生库不进 classpath
    let mut classpath: Vec<PathBuf> = vec![client_jar];
    for library in &version.libraries {
        if !meta::allowed(library.rules.as_deref(), &platform) {
            continue;
        }
        if meta::is_native_only(library, &platform) {
            continue;
        }
        if let Some(rel) = meta::library_path(library) {
            classpath.push(state.paths.libraries().join(meta::rel_path(&rel)));
        }
    }

    // 5) 主类与参数
    let main_class = version
        .main_class
        .clone()
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| Error::msg("version.json 缺少 mainClass，无法启动"))?;
    let assets_index = version
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| version.assets.clone())
        .unwrap_or_else(|| "legacy".to_string());
    // ${game_assets} 不总是 assets 根：1.6 及更早的索引带 virtual /
    // mapToResources，游戏要从展开后的目录读资源，指错的话游戏没声音、
    // 没语言文件。现代版本的索引没有这两个标记，传根目录（与官方一致）。
    let game_assets = legacy_assets_dir(&assets_dir, &assets_index, &game_dir);

    let placeholder = Placeholders {
        username: player_name,
        uuid: player_uuid.clone(),
        access_token: if real_account {
            account.access_token.clone()
        } else {
            "0".to_string()
        },
        // 官方约定：微软账户 msa、皮肤站（yggdrasil）mojang、离线 legacy。
        // 个别服务端插件按 user_type 分流，填错会被当成盗版账号。
        user_type: match account.kind.as_str() {
            auth::KIND_MICROSOFT => "msa",
            auth::KIND_AUTHLIB => "mojang",
            _ => "legacy",
        }
        .to_string(),
        version_id: version.id.clone(),
        version_type: version
            .version_type
            .clone()
            .unwrap_or_else(|| "release".to_string()),
        game_dir: game_dir.to_string_lossy().to_string(),
        assets_dir: assets_dir.to_string_lossy().to_string(),
        game_assets: game_assets.to_string_lossy().to_string(),
        assets_index,
        natives_dir: natives.to_string_lossy().to_string(),
        libraries_dir: state.paths.libraries().to_string_lossy().to_string(),
        classpath_separator: if cfg!(windows) { ";" } else { ":" },
        launcher_name: "MCL".to_string(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    // 4) 内存与 GC
    let memory = resolve_memory(state, &settings, &version);
    let mut args: Vec<String> = Vec::new();
    args.push(format!("-Xmx{}M", memory.max_mb));
    args.push(format!("-Xms{}M", memory.min_mb));
    // GC 参数放在 -Xmx 之后，与 PCL 的顺序一致
    for arg in memory::gc_args(memory::GcMode::parse(&settings.gc_mode), info.major) {
        args.push(arg);
    }
    // natives 目录
    args.push(format!(
        "-Djava.library.path={}",
        placeholder.natives_dir
    ));
    // 版本自带的 JVM 参数：只取 -D / -XX / -javaagent 这类，
    // classpath 由我们自己拼（元数据里的 ${classpath} 不一定包含客户端 jar）
    if let Some(arguments) = &version.arguments {
        for value in expand(&arguments.jvm, &platform) {
            let resolved = placeholder.apply(&value);
            if resolved.starts_with("-cp")
                || resolved.starts_with("-classpath")
                || resolved.starts_with("--class-path")
                || resolved.contains("${classpath}")
            {
                continue;
            }
            // 版本自带的 GC 参数会和上面冲突：`UseG1GC` 与 `UseZGC` 同时存在
            // JVM 会直接拒绝启动，所以这类参数一律丢掉。
            if memory::is_gc_arg(&resolved) {
                continue;
            }
            if resolved.starts_with("-D") || resolved.starts_with("-XX") || resolved.starts_with("-javaagent") {
                args.push(resolved);
            }
        }
    }
    // authlib-injector：必须在 JVM 参数段（-cp 之前）。
    // 等号后面是 yggdrasil 根，agent 自己会去拉元数据并接管游戏内的验证与皮肤。
    if let Some(injector) = &injector {
        args.push(format!(
            "-javaagent:{}={}",
            injector.jar.display(),
            injector.server_url
        ));
    }
    args.push("-cp".to_string());
    args.push(
        classpath
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(placeholder.classpath_separator),
    );
    args.push(main_class);

    // 游戏参数
    let game_args: Vec<String> = match &version.arguments {
        Some(arguments) if !arguments.game.is_empty() => expand(&arguments.game, &platform)
            .into_iter()
            .map(|value| placeholder.apply(&value))
            .collect(),
        _ => version
            .minecraft_arguments
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(|token| placeholder.apply(token))
            .collect(),
    };
    args.extend(game_args);

    // 6) 拉起进程
    let mut command = Command::new(&info.path);
    command
        .args(&args)
        .current_dir(&game_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows: Java 是控制台子系统程序。即使 stdout/stderr 被管道接管，
    // 某些启动环境仍会短暂创建控制台窗口；显式禁用新控制台，同时保留日志管道。
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = command.spawn().map_err(|e| {
        Error::msg(format!(
            "无法启动 Java（{}）：{e}。请在设置里检查 Java 路径",
            info.path
        ))
    })?;
    let pid = child.id();

    // 日志落盘：logs/<版本>/<时间>.log。这次启动的完整命令行也记进去，
    // 排查"参数拼错了"这类问题时不用再猜。
    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let log_dir = state.paths.logs().join(&version.id);
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{}.log", stamp(started_ms)));
    let header = format!(
        "MCL {} · 版本 {} · Java {}（{}）\n命令行：{} {}\n----------\n",
        env!("CARGO_PKG_VERSION"),
        version.id,
        info.major,
        info.path,
        info.path,
        args.join(" "),
    );
    let sink = Arc::new(LogSink::open(&log_path, &header));

    // 输出转发：推前端 + 写日志
    if let Some(stdout) = child.stdout.take() {
        pipe(app, stdout, "out", sink.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        pipe(app, stderr, "err", sink.clone());
    }

    // 注册到"正在运行"列表：「结束游戏」与重复启动防护都靠它
    state
        .running_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(RunningGame {
            pid,
            version: version.id.clone(),
            started_at_ms: started_ms,
        });

    // 8) 进程退出事件
    let handle = app.clone();
    let version_id = version.id.clone();
    std::thread::spawn(move || {
        // 退出码统一按 i64 传：Windows 的原生崩溃码（如 0xC0000005）是负的 i32，
        // 前端与日志里都以 32 位有符号形式出现，直接转 i64 才能对上关键词判断
        let code = child
            .wait()
            .ok()
            .and_then(|s| s.code())
            .map(i64::from)
            .unwrap_or(-1);
        // 管道线程可能还有最后几行没写完（进程退出后输出要再冲一小会儿）
        std::thread::sleep(std::time::Duration::from_millis(300));
        let tail = sink.snapshot();
        let tips = analyze_exit(code, &tail);
        {
            use std::io::Write;
            if let Some(file) = &sink.file {
                if let Ok(mut guard) = file.lock() {
                    let footer = format!(
                        "\n----------\n游戏退出：code={}\n{}",
                        code,
                        if tips.is_empty() {
                            String::new()
                        } else {
                            format!("初步判断：\n{}\n", tips.join("\n"))
                        }
                    );
                    let _ = guard.write_all(footer.as_bytes());
                }
            }
        }
        // 从"正在运行"里摘掉。按 pid 找：版本 ID 理论上唯一，
        // 但玩家删了同名版本再重装的窗口期里 pid 才是可信的。
        if let Some(state) = handle.try_state::<AppState>() {
            state
                .running_games
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|g| g.pid != pid);
        }
        let _ = handle.emit(
            EVENT_GAME_EXIT,
            GameExitPayload {
                version: version_id,
                code,
                log_path: log_path.to_string_lossy().to_string(),
                tips,
            },
        );
    });

    Ok(LaunchReport {
        pid,
        java: info.path,
        java_major: info.major,
        arg_count: args.len(),
    })
}

/// 结束一个游戏进程。Windows 用 taskkill 连子进程一起收，
/// Unix 直接 SIGKILL。返回是否成功。
pub fn kill_pid(pid: u32) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：别给玩家弹黑框
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
    }
}

/// 崩溃初判：对日志尾部做关键词匹配，给出小白能照着做的建议。
///
/// 定位是"挡掉大半求助"，不是完整诊断（PCL 的 1100 行 crash 分析是上限）。
/// 每条都写成"现象 → 该做什么"，一条日志命中几条给几条。
pub fn analyze_exit(code: i64, tail: &[String]) -> Vec<String> {
    let mut tips = Vec::new();
    if code == 0 {
        // 正常退出，不需要"分析"
        return tips;
    }
    let text = tail.join("\n").to_ascii_lowercase();

    if text.contains("outofmemoryerror") || text.contains("out of memory") {
        tips.push(
            "内存不够：到「设置 → 内存」把内存调大一点（如果机器够），或者少装些 mod / 换掉大光影。".to_string(),
        );
    }
    if text.contains("could not reserve enough space") || text.contains("error occurred during initialization of vm") {
        tips.push(
            "物理内存不够给 Java 分：把「设置 → 内存」调小，或者关掉浏览器等吃内存的程序再启动。".to_string(),
        );
    }
    if text.contains("unsupportedclassversionerror") || text.contains("class file version") {
        tips.push(
            "Java 版本太低：这个版本需要更高版本的 Java。首页的 Java 卡片可以一键下载，或者去设置里换个 Java 路径。".to_string(),
        );
    }
    if text.contains("unsatisfiedlinkerror") || text.contains("failed to locate library") {
        tips.push(
            "游戏组件没装完整（缺原生库）：到「版本管理」删掉这个版本重新安装一遍。".to_string(),
        );
    }
    if text.contains("duplicatemodsfoundexception") || text.contains("duplicate mods") {
        tips.push(
            "同一个 mod 装了两份：到「模组管理」把重复的那份停用或删掉（文件名通常很像）。".to_string(),
        );
    }
    if text.contains("missing mods") || text.contains("missing mandatory dependencies") || text.contains("mod resolution") {
        tips.push(
            "缺前置 mod：日志里列了缺哪些（找不到 mod id 的翻译时去 mod 发布页看依赖），装齐再启动。".to_string(),
        );
    }
    if text.contains("mixin apply failed") || text.contains("mixin apply") || text.contains("transient modifier") {
        tips.push(
            "mod 冲突：多半是最近新装的 mod 和现有的打架。把最新装的停用试试，稳定了再逐个加回来。".to_string(),
        );
    }
    if text.contains("mods.toml") || text.contains("fml.modloading") && text.contains("missing") {
        tips.push(
            "mods 目录里混进了不是 mod 的文件（比如解压出来的说明文件）：只保留 .jar 文件。".to_string(),
        );
    }
    if text.contains("glfw error") || text.contains("opengl") || text.contains("glx") {
        tips.push(
            "显卡 / 显卡驱动问题：把显卡驱动更新到最新，笔记本注意别让游戏跑在核显上。".to_string(),
        );
    }
    // Windows 原生崩溃 0xC0000005（访问冲突）经常以十进制 -1073741819 出现
    if code == -1073741819 || code == -1073740791 {
        tips.push(
            "游戏进程原生崩溃：常见原因是 mod 与当前游戏版本不匹配，或显卡驱动过旧。".to_string(),
        );
    }
    if tips.is_empty() {
        tips.push(
            "没有认出的崩溃原因。把下面的日志文件发给帮你装游戏的人（或启动器作者），比截图快得多。".to_string(),
        );
    }
    tips
}

/// 时间戳文件名：`20261024-153012`。不引入 chrono，手写 UTC 时间换算。
fn stamp(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}",
        h = rem / 3600,
        mi = (rem % 3600) / 60,
        s = rem % 60
    )
}

/// Howard Hinnant 的 civil-from-days：天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 本次启动实际采用的堆参数。
struct MemoryChoice {
    max_mb: u32,
    min_mb: u32,
}

/// 决定 `-Xmx` / `-Xms`。
///
/// 自动模式下 `-Xmx` 由 mod 数量和当前可用内存现算（PCL 的四段阶梯），
/// 手动模式用设置里的值。两种模式都会把 `-Xms` 抬起来：
/// PCL 其实只设 `-Xmx`，JVM 默认 `-Xms` 只有物理内存的 1/64，
/// 大整合包启动时堆要一路扩容、每扩一次一次 GC，表现出来就是"卡很久"。
fn resolve_memory(state: &AppState, settings: &Settings, version: &meta::VersionJson) -> MemoryChoice {
    let auto = settings.ram_type != "manual";

    let max_mb = if auto {
        let optifine = version.id.to_ascii_lowercase().contains("optifine");
        // 按这个版本自己的 mods 目录统计，不再看共享目录
        memory::recommend(&state.paths.mods(&version.id), optifine).recommend_mb
    } else {
        settings.max_memory_mb
    };

    let min_mb = if auto {
        let half = (max_mb / 2).max(512);
        // 初始堆不能超过当前可用内存，否则 JVM 直接报 "Could not reserve enough space"。
        // 下限地板必须放在可用性钳制之前：反过来写的话，可用内存不足 640MB 时
        // 会被 min 压下去又被 max 抬回 512，守卫在最需要它的场景正好失效。
        // （可用内存连 320MB 都不到时游戏本来就起不来，256 只是兜底防 0。）
        let available = memory::available_mb() as u32;
        if available > 0 {
            half.max(512).min(available * 8 / 10).min(max_mb).max(256)
        } else {
            half.min(max_mb)
        }
    } else {
        settings.min_memory_mb
    };

    MemoryChoice { max_mb, min_mb }
}

/// 沿继承链找真正解压过 natives 的目录。
///
/// 链的顺序是「子 → 父」，子版本（Fabric / Quilt / 整合包薄壳）的 natives
/// 目录通常是空的，必须往下找到原版那一层。
fn resolve_natives(paths: &crate::state::Paths, chain: &[String], fallback: &str) -> PathBuf {
    chain
        .iter()
        .map(|id| paths.natives_dir(id))
        .find(|dir| has_entries(dir))
        .unwrap_or_else(|| paths.natives_dir(fallback))
}

/// 目录里有没有文件。用来判断某个版本的 natives 是否真的解压过。
fn has_entries(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// 解析 `${game_assets}` 应该指向哪个目录。
///
/// 与官方启动器同规则：索引带 `virtual` → `assets/virtual/<索引>`
/// （install 阶段已把资源铺到那里）；带 `mapToResources` → 游戏目录的
/// `resources/`；都没有（1.7+）→ assets 根。索引文件缺失或解析失败时
/// 保守退回 assets 根，不阻断启动。
fn legacy_assets_dir(assets: &std::path::Path, index_id: &str, game_dir: &std::path::Path) -> PathBuf {
    let index_path = assets.join("indexes").join(format!("{index_id}.json"));
    let index = std::fs::read_to_string(&index_path)
        .ok()
        .and_then(|text| serde_json::from_str::<meta::AssetIndex>(&text).ok());
    match index {
        Some(index) if index.is_virtual.unwrap_or(false) => {
            assets.join("virtual").join(index_id)
        }
        Some(index) if index.map_to_resources.unwrap_or(false) => game_dir.join("resources"),
        _ => assets.to_path_buf(),
    }
}

fn pipe<R: std::io::Read + Send + 'static>(
    app: &AppHandle,
    reader: R,
    stream: &str,
    sink: Arc<LogSink>,
) {
    let handle = app.clone();
    let stream = stream.to_string();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        // 不能用 lines()：它遇到非 UTF-8 字节会返回 Err，map_while 会把整个
        // 读取循环掐断——中文 Windows 上 Java 8 用 GBK 打日志，日志就断流了。
        // 按字节读到换行再 lossy 解码，坏字节顶多显示成 �。
        let mut reader = BufReader::new(reader);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    sink.write(&format!("[{stream}] {line}\n"));
                    let _ = handle.emit(EVENT_LOG, LogLine { stream: stream.clone(), line });
                }
            }
        }
    });
}

/// 参数里的占位符替换。
struct Placeholders {
    username: String,
    uuid: String,
    /// 皮肤站账户是真实 token；离线是 "0"（游戏只做本地校验）
    access_token: String,
    /// 皮肤站 `mojang`，离线 `legacy`
    user_type: String,
    version_id: String,
    version_type: String,
    game_dir: String,
    assets_dir: String,
    /// `${game_assets}`：legacy 索引时是 virtual / resources 目录，其余是 assets 根
    game_assets: String,
    assets_index: String,
    natives_dir: String,
    libraries_dir: String,
    classpath_separator: &'static str,
    launcher_name: String,
    launcher_version: String,
}

impl Placeholders {
    fn apply(&self, raw: &str) -> String {
        raw.replace("${auth_player_name}", &self.username)
            .replace("${auth_uuid}", &self.uuid)
            .replace("${auth_access_token}", &self.access_token)
            .replace(
                "${auth_session}",
                &format!("token:{}:{}", self.access_token, self.uuid),
            )
            .replace("${user_type}", &self.user_type)
            .replace("${user_properties}", "{}")
            .replace("${version_name}", &self.version_id)
            .replace("${version_type}", &self.version_type)
            .replace("${game_directory}", &self.game_dir)
            .replace("${assets_root}", &self.assets_dir)
            .replace("${game_assets}", &self.game_assets)
            .replace("${assets_index_name}", &self.assets_index)
            .replace("${natives_directory}", &self.natives_dir)
            .replace("${library_directory}", &self.libraries_dir)
            .replace("${classpath_separator}", self.classpath_separator)
            .replace("${launcher_name}", &self.launcher_name)
            .replace("${launcher_version}", &self.launcher_version)
            .replace("${resolution_width}", "854")
            .replace("${resolution_height}", "480")
            .replace("${clientid}", "")
            .replace("${auth_xuid}", "")
    }
}

/// 按 rules 展开参数，返回原始（未替换占位符）的字符串列表。
fn expand(args: &[Argument], platform: &Platform) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(value) => out.push(value.clone()),
            Argument::Conditional { rules, value } => {
                if !meta::allowed(rules.as_deref(), platform) {
                    continue;
                }
                match value {
                    ArgValue::One(value) => out.push(value.clone()),
                    ArgValue::Many(values) => out.extend(values.iter().cloned()),
                }
            }
        }
    }
    out
}

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_human_readable() {
        // 2026-10-24 00:00:00 UTC 的 Unix 秒是 1791830400... 用已知对校验
        let (y, m, d) = civil_from_days(0);
        assert_eq!((y, m, d), (1970, 1, 1));
        let (y, m, d) = civil_from_days(19_723); // 2023-12-31 前后校验闰年/月末
        assert_eq!(stamp((19_723 * 86_400) as u64 * 1000), format!("{y:04}{m:02}{d:02}-000000"));
        assert!(stamp(0).starts_with("19700101"));
    }

    #[test]
    fn oom_is_recognized() {
        let tail = vec![
            "java.lang.OutOfMemoryError: Java heap space".to_string(),
            "at net.minecraft.client.main.Main.main(SourceFile)".to_string(),
        ];
        let tips = analyze_exit(1, &tail);
        assert!(tips.iter().any(|t| t.contains("内存不够")));
    }

    #[test]
    fn wrong_java_version_is_recognized() {
        let tail = vec![
            "java.lang.UnsupportedClassVersionError: net/minecraft/client/main/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0)".to_string(),
        ];
        let tips = analyze_exit(1, &tail);
        assert!(tips.iter().any(|t| t.contains("Java 版本太低")));
    }

    #[test]
    fn normal_exit_has_no_tips() {
        assert!(analyze_exit(0, &[]).is_empty());
    }

    #[test]
    fn unknown_crash_gets_a_catch_all() {
        let tips = analyze_exit(1, &["some unknown noise".to_string()]);
        assert_eq!(tips.len(), 1);
        assert!(tips[0].contains("日志"));
    }

    #[test]
    fn access_violation_code_is_recognized() {
        let tips = analyze_exit(-1073741819, &[]);
        assert!(tips.iter().any(|t| t.contains("原生崩溃")));
    }

    fn placeholders() -> Placeholders {
        Placeholders {
            username: "Steve".into(),
            uuid: "00000000-0000-3000-8000-000000000000".into(),
            // 离线模式的默认值：token 是 0
            access_token: "0".into(),
            user_type: "legacy".into(),
            version_id: "1.20.1".into(),
            version_type: "release".into(),
            game_dir: "/mc/game".into(),
            assets_dir: "/mc/assets".into(),
            game_assets: "/mc/assets".into(),
            assets_index: "5".into(),
            natives_dir: "/mc/versions/1.20.1/natives".into(),
            libraries_dir: "/mc/libraries".into(),
            classpath_separator: ":",
            launcher_name: "MCL".into(),
            launcher_version: "0.1.0".into(),
        }
    }

    #[test]
    fn placeholders_are_replaced() {
        let map = placeholders();
        assert_eq!(map.apply("${auth_player_name}"), "Steve");
        assert_eq!(map.apply("${version_name}"), "1.20.1");
        assert_eq!(map.apply("--assetIndex ${assets_index_name}"), "--assetIndex 5");
        // 离线：token 是 0
        assert_eq!(map.apply("${auth_access_token}"), "0");
        assert_eq!(
            map.apply("${auth_session}"),
            "token:0:00000000-0000-3000-8000-000000000000"
        );
        assert_eq!(map.apply("${user_type}"), "legacy");
    }

    #[test]
    fn authlib_placeholders_carry_real_token() {
        let map = Placeholders {
            username: "Notch".into(),
            uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into(),
            access_token: "real-token".into(),
            user_type: "mojang".into(),
            ..placeholders()
        };
        assert_eq!(map.apply("${auth_access_token}"), "real-token");
        assert_eq!(
            map.apply("${auth_session}"),
            "token:real-token:069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
        assert_eq!(map.apply("${user_type}"), "mojang");
        assert_eq!(map.apply("${auth_player_name}"), "Notch");
    }

    #[test]
    fn offline_uuid_is_stable_and_well_formed() {
        let a = offline_uuid("Steve");
        let b = offline_uuid("Steve");
        assert_eq!(a, b, "同一名字必须得到同一个 UUID");
        assert_ne!(a, offline_uuid("Alex"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    /// Fabric / 整合包版本自己没有 natives，必须落到原版那一层的目录，
    /// 否则 `-Djava.library.path` 是空的，启动直接 UnsatisfiedLinkError。
    #[test]
    fn natives_follow_the_inheritance_chain() {
        let root = std::env::temp_dir().join(format!(
            "mcl-natives-chain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let paths = crate::state::Paths { root: root.clone() };
        let child = "fabric-loader-0.16.9-1.20.1".to_string();
        let base = "1.20.1".to_string();
        // 子版本目录建出来但是空的（加载器不产出 natives）
        std::fs::create_dir_all(paths.natives_dir(&child)).unwrap();
        std::fs::create_dir_all(paths.natives_dir(&base)).unwrap();
        std::fs::write(paths.natives_dir(&base).join("lwjgl.dll"), b"x").unwrap();

        let chain = vec![child.clone(), base.clone()];
        assert_eq!(
            resolve_natives(&paths, &chain, &child),
            paths.natives_dir(&base),
            "应当用原版目录里的 natives"
        );

        // 谁都没有文件时退回子版本目录，保持原有行为（launch 会自己 create_dir）
        let empty = vec!["a".to_string(), "b".to_string()];
        assert_eq!(resolve_natives(&paths, &empty, "a"), paths.natives_dir("a"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rules_filter_conditional_arguments() {
        let raw = r#"[
            "--demo",
            {"rules":[{"action":"allow","os":{"name":"windows"}}],"value":["--win-only"]},
            {"rules":[{"action":"allow","os":{"name":"linux"}}],"value":["--linux-only","--x"]}
        ]"#;
        let args: Vec<Argument> = serde_json::from_str(raw).unwrap();
        let on_linux = expand(&args, &Platform::new("linux", "x86_64"));
        assert_eq!(on_linux, vec!["--demo", "--linux-only", "--x"]);
        let on_win = expand(&args, &Platform::new("windows", "x86_64"));
        assert_eq!(on_win, vec!["--demo", "--win-only"]);
    }
}
