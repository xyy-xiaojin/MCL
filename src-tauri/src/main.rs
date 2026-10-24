// Windows 下不弹控制台黑窗口（debug/release 都不弹；panic 走日志/对话框）
#![windows_subsystem = "windows"]

fn main() {
    // WebKitGTK 在部分显卡驱动上启动即堆损坏（glibc "corrupted double-linked list"），
    // 关掉 DMABUF 渲染器可稳定复现地避免。必须在 Tauri 初始化前设置。
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    mcl_lib::run()
}
