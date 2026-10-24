//! 目录布局与用户设置。
//!
//! 所有落盘位置集中在这里：前端只传版本 ID 之类的逻辑标识，
//! 具体路径一律由后端决定（这也是"文件 IO 全部交给 Rust"的前提）。
use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::engine::java;
use crate::engine::net;
use crate::engine::queue::Queue;
use crate::error::Result;

/// `.minecraft` 风格的目录树。
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// 默认放在系统的应用数据目录下，避免污染用户已有的 `.minecraft`。
    pub fn detect() -> Self {
        let base = dirs::data_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Self {
            root: base.join("MCL"),
        }
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    /// 某个版本的 mods 目录。
    ///
    /// **按版本隔离**：`versions/<id>/game/mods`。启动时 `--gameDir` 指向
    /// `versions/<id>/game`，所以游戏只会看到这个版本自己的 mod。
    /// 之前的共享 `root/game/mods` 会让 1.20.1 的 mod 在 1.21 里照样加载。
    pub fn mods(&self, id: &str) -> PathBuf {
        self.instance_game(id).join("mods")
    }

    /// 材质包目录（按版本隔离）。
    pub fn resourcepacks(&self, id: &str) -> PathBuf {
        self.instance_game(id).join("resourcepacks")
    }

    /// 光影包目录（按版本隔离）。
    pub fn shaderpacks(&self, id: &str) -> PathBuf {
        self.instance_game(id).join("shaderpacks")
    }

    /// 存档目录（按版本隔离）。
    pub fn saves(&self, id: &str) -> PathBuf {
        self.instance_game(id).join("saves")
    }

    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    /// 可以随时删掉的临时缓存（版本清单、镜像测速结果）。
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// 玩家自己导入的皮肤。
    ///
    /// 放 `root/skins` 而不是 `cache/skins`：缓存目录的语义是"删了会自动重建"，
    /// 而玩家手动挑的皮肤删了就找不回来了。
    pub fn skins(&self) -> PathBuf {
        self.root.join("skins")
    }

    /// 联机用到的落盘文件：SakuraFrp 的访问密钥、生成出来的 frpc 配置。
    ///
    /// 放 `root/netplay`：访问密钥是凭据，跟着应用数据目录走，
    /// 删版本、清缓存都不该把它一起带走。
    pub fn netplay(&self) -> PathBuf {
        self.root.join("netplay")
    }

    /// 存档备份。`backups/<版本id>/<时间戳>.zip`。
    ///
    /// 删版本前自动备一份、玩家也能手动备份/还原——存档是唯一
    /// 删了就找不回的东西，值得一个专门的目录。
    pub fn backups(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// 游戏日志。`logs/<版本>/<启动时间>.log`。
    ///
    /// 之前游戏输出只推给前端，关窗口就没了——崩溃后没有任何现场，
    /// 每次都变成"请把刚才的控制台截图发我"。落盘之后才有崩溃分析可言。
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 自动下载的 Java 运行时：`jre/<主版本>/`。
    pub fn jre_dir(&self) -> PathBuf {
        self.root.join("jre")
    }

    /// JRE 下载中的临时压缩包，解压完即删。
    pub fn jre_downloads(&self) -> PathBuf {
        self.jre_dir().join("_pkg")
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }

    pub fn natives_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }

    /// 某个版本的私有游戏目录（版本隔离）。
    ///
    /// 放在 `versions/<id>/game` 下：mods、config、saves 都是这个版本独有的，
    /// 换版本不会互相污染。删除版本时整块跟着删掉。
    pub fn instance_game(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("game")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            self.root.clone(),
            self.versions(),
            self.libraries(),
            self.assets(),
            self.instances(),
            self.skins(),
            self.netplay(),
            self.logs(),
            self.jre_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// 前端"设置页"能改的东西。
///
/// `#[serde(default)]` 是必需的：以后再加字段时，旧的 settings.json
/// 缺字段会逐字段回落默认值，而不是整个反序列化失败、把用户设置清空。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `bmclapi`（国内镜像）或 `official`（Mojang 官方）。
    pub source: String,
    /// 手动模式下的堆上限；自动模式下这个值不生效，由 `memory::recommend` 现算。
    pub max_memory_mb: u32,
    /// JVM 初始堆（`-Xms`）。自动模式下会被抬到接近 `-Xmx`。
    pub min_memory_mb: u32,
    /// `auto`（按 mod 数量与可用内存推荐）或 `manual`。
    pub ram_type: String,
    /// GC 策略：`auto` / `g1` / `g1-optimized` / `custom`。
    pub gc_mode: String,
    /// 留空表示自动探测。
    pub java_path: String,
    pub username: String,
    pub theme: String,
    /// 玩家导入的离线皮肤（PNG 绝对路径）。空串表示用内置的 Steve。
    ///
    /// 只影响头像显示，不会改写 `game/` 里的任何东西：0.01 只有离线账号，
    /// 游戏内皮肤需要服务端或 `CustomSkinLoader` 之类的 mod 才生效。
    pub skin_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: "bmclapi".to_string(),
            max_memory_mb: 4096,
            min_memory_mb: 512,
            ram_type: "auto".to_string(),
            gc_mode: "auto".to_string(),
            java_path: String::new(),
            username: "Player".to_string(),
            theme: "dark".to_string(),
            skin_path: String::new(),
        }
    }
}

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        let file = paths.settings_file();
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure()?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(paths.settings_file(), text)?;
        Ok(())
    }

    /// 内存值兜底：滑块传进来的值可能是越界的，别直接交给 JVM。
    pub fn normalized(&self) -> Self {
        let mut out = self.clone();
        out.max_memory_mb = self.max_memory_mb.clamp(1024, 32768);
        out.min_memory_mb = self.min_memory_mb.min(out.max_memory_mb);
        if out.username.trim().is_empty() {
            out.username = "Player".to_string();
        }
        out.skin_path = out.skin_path.trim().to_string();
        out
    }
}

/// 全局共享状态。Tauri 的 `manage` 只接受 `Send + Sync + 'static`。
pub struct AppState {
    pub paths: Paths,
    pub settings: RwLock<Settings>,
    pub client: reqwest::Client,
    pub queue: std::sync::Arc<Queue>,
    /// Java 探测缓存：`(设置里填的路径, 结果)`。
    ///
    /// `java -version` 要起一个进程，实测 130~170 ms，装了杀软的机器上首次更久。
    /// 之前每次算内存方案（切版本、存设置、导入完回来）都重跑一遍，
    /// 这 100 多毫秒是直接在界面线程上等的，表现就是"点了没反应"。
    /// Java 路径一变缓存自然失效，另外用户点"重新检测"可以强制刷新。
    pub java_cache: std::sync::Mutex<Option<(String, Option<java::JavaInfo>)>>,
    /// 联机三条通路的运行时（状态 + 子进程句柄）。
    ///
    /// 放进 AppState 而不是各命令里临时持有：进程要能跨命令被停掉，
    /// 应用退出时也要能一次性清干净，不能留下孤儿 EasyTier/frpc 进程。
    pub netplay: crate::engine::netplay::Netplay,
    /// 正在运行的游戏进程。
    ///
    /// 记 pid 是为了两件事：**重复启动防护**（连点两次启动，两个游戏抢
    /// 同一份存档是存档损坏的高发原因）和**「结束游戏」**（游戏卡死时
    /// 玩家需要一个按钮，而不是去任务管理器里找 javaw）。
    pub running_games: std::sync::Mutex<Vec<RunningGame>>,
    /// 应用句柄。forge / jdk 这类不经过命令层的引擎模块发事件用
    /// （安装器的实时日志就是从这里推出去的）。
    pub app: tauri::AppHandle,
    /// 账户文件的读改写锁。
    ///
    /// `launch_version` 的 token 续期是「读 → 网络（可达 10~25 秒）→ 写回」
    /// 的长事务；期间玩家在首页登录/退出另一个账户，旧事务写回时会把
    /// 新账户整个覆盖掉。所有动 account.json 的路径都在这把锁下走。
    pub account_lock: tokio::sync::Mutex<()>,
}

/// 一个正在运行的游戏进程。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningGame {
    pub pid: u32,
    pub version: String,
    /// 启动时刻（Unix 毫秒），前端显示"已运行 xx 分钟"用
    pub started_at_ms: u64,
}

impl AppState {
    /// 取 Java 信息。`force` 为真时忽略缓存重新探测。
    pub fn java_info(&self, force: bool) -> Option<java::JavaInfo> {
        let key = self.settings().java_path.trim().to_string();
        let mut cache = self
            .java_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !force {
            if let Some((cached_key, info)) = cache.as_ref() {
                if cached_key == &key {
                    return info.clone();
                }
            }
        }
        let info = java::detect(&key);
        *cache = Some((key, info.clone()));
        info
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// 当前下载源，决定 URL 是否重写到镜像。
    pub fn source(&self) -> String {
        self.settings().source
    }

    pub fn base_url(&self) -> &'static str {
        net::base_url_for(&self.source())
    }
}
