//! 联机：三条互不相同的通路，一个页面上说完。
//!
//! | 通道 | 原理 | 适合谁 |
//! |------|------|--------|
//! | [`easytier`] | 双方都装客户端，建一层虚拟局域网 | 朋友都有电脑，想稳定长期联机 |
//! | [`portmap`] | 只在房主这边开洞（UPnP / STUN），朋友直连公网 | 房主是 NAT1 或有公网 IP |
//! | [`frp`] | 流量过服务商中继 | NAT3/NAT4、公司校园网，上面两条都打不通时 |
//!
//! 三条通路都要起一个外部进程（EasyTier 的 core、SakuraFrp 的 frpc），
//! 进程的生命周期、日志、状态全部收在这层，前端只订阅事件。
//!
//! ## 为什么同一时间只允许开一条
//!
//! 三条通路都会改写本机的路由/端口映射：EasyTier 要建虚拟网卡、端口映射要占
//! 一个公网端口、frpc 要占本地连接。同时开两个不会更快，只会让"到底是哪条在
//! 生效"变得没法排查。所以 [`Netplay::start`] 会先把别的通道停掉。
use std::io::{BufRead, BufReader};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub mod easytier;
pub mod frp;
pub mod portmap;

/// 三条通路的标识。前端 Tab 与后端状态用同一个字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    Easytier,
    Portmap,
    Frp,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Easytier => "easytier",
            Self::Portmap => "portmap",
            Self::Frp => "frp",
        }
    }
}

/// 一条通路的当前状态。前端整块渲染这个结构，不需要自己拼逻辑。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    /// `idle` / `starting` / `running` / `error`
    pub state: String,
    /// 一句话主文案
    pub summary: String,
    /// 给朋友填进 MC「添加服务器」里的地址，空串表示还没拿到
    pub address: String,
    /// 本地游戏端口
    pub local_port: u16,
    /// 失败原因（`state == "error"` 时才有意义）
    pub error: String,
    /// 附带信息：虚拟 IP、节点、NAT 类型……前端按「名称 / 值」两列展示
    pub facts: Vec<Fact>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub label: String,
    pub value: String,
}

impl Fact {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            summary: "未启动".to_string(),
            address: String::new(),
            local_port: 0,
            error: String::new(),
            facts: Vec::new(),
        }
    }
}

impl Slot {
    pub fn starting(local_port: u16, summary: impl Into<String>) -> Self {
        Self {
            state: "starting".to_string(),
            summary: summary.into(),
            local_port,
            ..Default::default()
        }
    }

    pub fn fail(local_port: u16, error: impl Into<String>) -> Self {
        Self {
            state: "error".to_string(),
            summary: "启动失败".to_string(),
            local_port,
            error: error.into(),
            ..Default::default()
        }
    }
}

/// 一个通道 = 状态 + 子进程。
#[derive(Default)]
pub struct Runtime {
    pub slot: Slot,
    pub child: Option<Child>,
}

impl Runtime {
    /// 杀掉子进程并把状态清干净。重复调用是安全的。
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // frpc / easytier-core 都会自己处理 SIGKILL，Windows 上等价于 TerminateProcess
            let _ = child.kill();
            let _ = child.wait();
        }
        self.slot = Slot::default();
    }
}

/// 三条通路的运行时容器，挂在 [`crate::state::AppState`] 上。
pub struct Netplay {
    pub easytier: Mutex<Runtime>,
    pub portmap: Mutex<Runtime>,
    pub frp: Mutex<Runtime>,
}

impl Default for Netplay {
    fn default() -> Self {
        Self {
            easytier: Mutex::new(Runtime::default()),
            portmap: Mutex::new(Runtime::default()),
            frp: Mutex::new(Runtime::default()),
        }
    }
}

impl Netplay {
    pub fn slot_of(&self, channel: Channel) -> Slot {
        match channel {
            Channel::Easytier => lock(&self.easytier).slot.clone(),
            Channel::Portmap => lock(&self.portmap).slot.clone(),
            Channel::Frp => lock(&self.frp).slot.clone(),
        }
    }

    pub fn all(&self) -> [Slot; 3] {
        [
            self.slot_of(Channel::Easytier),
            self.slot_of(Channel::Portmap),
            self.slot_of(Channel::Frp),
        ]
    }

    pub fn runtime(&self, channel: Channel) -> &Mutex<Runtime> {
        match channel {
            Channel::Easytier => &self.easytier,
            Channel::Portmap => &self.portmap,
            Channel::Frp => &self.frp,
        }
    }

    pub fn stop(&self, channel: Channel) {
        lock(self.runtime(channel)).stop();
    }

    pub fn stop_all(&self) {
        for channel in [Channel::Easytier, Channel::Portmap, Channel::Frp] {
            self.stop(channel);
        }
    }
}

/// 取锁时忽略「上一次持锁线程 panic 了」这件事：状态本身是自洽的，
/// 为了一个毒锁把整个联机页打死不值得。
pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 随程序分发的联机组件目录。
///
/// 打包后是 `<安装目录>/resources/netplay/`，开发时是 `src-tauri/resources/netplay/`。
/// 两处都找不到就返回开发路径，让随后的「找不到 xxx.exe」报错直接指出位置。
pub fn tool_dir(app: &AppHandle) -> PathBuf {
    // 打包后：<安装目录>/resources/netplay（bundle.resources 会保留相对结构）
    // 有些打包器会把 resources 的内容直接铺到资源根，两个位置都认一下
    for rel in ["resources/netplay", "netplay"] {
        if let Ok(dir) = app
            .path()
            .resolve(rel, tauri::path::BaseDirectory::Resource)
        {
            if dir.is_dir() && dir.join("frpc.exe").is_file() {
                return dir;
            }
        }
    }
    // 开发时：src-tauri/resources/netplay
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/netplay")
}

/// 定位一个随包分发的可执行文件。
pub fn tool(app: &AppHandle, name: &str) -> crate::error::Result<PathBuf> {
    let path = tool_dir(app).join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(crate::error::Error::msg(format!(
            "缺少联机组件 {}，请重新解压完整的 MCL 压缩包（应位于 {}）",
            name,
            path.display()
        )))
    }
}

/// 起一个后台进程，把它的 stdout/stderr 按行转成 `netplay://log` 事件。
///
/// 返回值给调用方塞进 [`Runtime::child`]。
pub fn spawn(
    app: &AppHandle,
    channel: Channel,
    program: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
) -> crate::error::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // 别让子进程弹控制台黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| {
        crate::error::Error::msg(format!(
            "启动 {} 失败：{e}（路径 {}）",
            program.file_name().and_then(|s| s.to_str()).unwrap_or("组件"),
            program.display()
        ))
    })?;

    if let Some(stdout) = child.stdout.take() {
        pump(app.clone(), channel, "out", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        pump(app.clone(), channel, "err", stderr);
    }

    Ok(child)
}

/// 把一个子进程的输出管道按行转成事件。
fn pump<R: std::io::Read + Send + 'static>(
    app: AppHandle,
    channel: Channel,
    stream: &'static str,
    pipe: R,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            let _ = app.emit(
                "netplay://log",
                LogLine {
                    channel: channel.as_str().to_string(),
                    stream: stream.to_string(),
                    line,
                },
            );
        }
    });
}

/// `netplay://log` 的负载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub channel: String,
    /// `out` / `err`
    pub stream: String,
    pub line: String,
}

/// `netplay://status` 的负载：一条通路变化就整份推，前端少写分支。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEvent {
    pub channel: String,
    pub slot: Slot,
}

/// 状态写回 + 主动推事件。所有通道的"我变了"都走这里，避免漏推。
pub fn publish(app: &AppHandle, channel: Channel, slot: Slot) {
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        lock(state.netplay.runtime(channel)).slot = slot.clone();
    }
    let _ = app.emit(
        "netplay://status",
        StatusEvent {
            channel: channel.as_str().to_string(),
            slot,
        },
    );
}

/// 本机在局域网里的地址，给"同一个 WiFi 下的朋友"用。
pub fn lan_ip() -> Option<String> {
    // 连一个不可达地址不会真的发包，只让系统选出默认出口网卡
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("223.5.5.5:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// 端口是否有人监听——"开联机"之前先确认游戏真的在跑。
///
/// 只做 TCP 连接尝试，不写任何数据；连上就立刻断开。
pub fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(400),
    )
    .is_ok()
}

/// 探测一个远端 `host:port` 是否可连。域名解析放 blocking 线程。
///
/// 樱花节点只在该隧道注册成功后才会接受这个端口上的 TCP 连接，
/// 所以"连得上"就等于"隧道真的通了"——比只看 frpc 进程活着可靠得多。
pub async fn remote_open(remote: &str) -> bool {
    let Some((host, port)) = remote.rsplit_once(':') else {
        return false;
    };
    let Ok(port) = port.parse::<u16>() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_string();
    let addr = match tokio::task::spawn_blocking(move || {
        (host.as_str(), port).to_socket_addrs().ok().and_then(|mut it| it.next())
    })
    .await
    {
        Ok(Some(addr)) => addr,
        _ => return false,
    };
    tokio::task::spawn_blocking(move || {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(1200)).is_ok()
    })
    .await
    .unwrap_or(false)
}

/// 进程意外退出时把状态改成 error。
///
/// 没有这一层的话，EasyTier 因为没管理员权限在启动两秒后自杀，
/// 界面上却一直显示"运行中"——比直接报错更让人摸不着头脑。
pub fn watch(app: AppHandle, channel: Channel) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let Some(state) = app.try_state::<crate::state::AppState>() else {
            return;
        };
        let exited = {
            let mut rt = lock(state.netplay.runtime(channel));
            // 用户主动断开后 slot 会被清成 idle，这时不该报"意外退出"
            if rt.slot.state == "idle" {
                return;
            }
            let Some(child) = rt.child.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => Some(status.to_string()),
                Ok(None) => None,
                Err(_) => return,
            }
        };
        if let Some(status) = exited {
            let mut slot = state.netplay.slot_of(channel);
            slot.state = "error".to_string();
            slot.summary = "进程已退出".to_string();
            slot.error = format!("{}{}", status, exit_hint(channel));
            publish(&app, channel, slot);
            return;
        }
    });
}

/// 子进程退出时最可能的原因，直接写在错误里，省得玩家猜。
fn exit_hint(channel: Channel) -> String {
    match channel {
        Channel::Easytier if !is_admin() => {
            "：建虚拟网卡需要管理员权限，请用管理员身份重新打开 MCL".to_string()
        }
        Channel::Easytier => "：看下面的日志，多半是房间名/密码和对面不一致，或种子节点连不上".to_string(),
        Channel::Frp => "：看下面的日志，常见是节点不通、流量用完、密钥失效".to_string(),
        Channel::Portmap => "：端口被占用或被系统拦了".to_string(),
    }
}

/// 当前进程是不是管理员。
///
/// EasyTier 要装虚拟网卡（wintun 驱动），这一步没有管理员权限会直接失败，
/// 而且是**静默**失败——它连日志都来不及写就退了，所以必须提前拦。
#[cfg(windows)]
pub fn is_admin() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    // SAFETY: 无参数、无副作用地查当前进程的令牌
    unsafe { IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    // 非 Windows 平台没有这个限制（Linux 上 TUN 走 /dev/net/tun 的权限位）
    true
}

/// 用管理员权限重新拉起自己。
///
/// 走 `ShellExecuteW` + `runas`，也就是右键「以管理员身份运行」等价的那条路；
/// 成功返回后调用方应该立刻退出当前实例，避免两个 MCL 同时存在。
#[cfg(windows)]
pub fn relaunch_as_admin() -> bool {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut core::ffi::c_void;
    }

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let to_wide = |s: &std::ffi::OsStr| -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    };
    let file = to_wide(exe.as_os_str());
    let operation: Vec<u16> = "runas\0".encode_utf16().collect();

    const SW_SHOWNORMAL: i32 = 1;
    // SAFETY: 四个指针都指向以 NUL 结尾的宽字符串，生命周期覆盖整个调用
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 返回值 > 32 才算成功
    ret as isize > 32
}

#[cfg(not(windows))]
pub fn relaunch_as_admin() -> bool {
    false
}
