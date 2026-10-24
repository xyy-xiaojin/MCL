//! MCL · Minecraft 启动器（0.01）
//!
//! 前端（Vue3）只做渲染与交互，所有网络、文件、哈希、进程操作都在这里。
//! 事件约定：
//! - `queue://progress` 单文件下载进度
//! - `queue://changed`  队列结构变化（增删/状态迁移）
//! - `log://line`       游戏进程输出
//! - `game://exit`      游戏退出
mod commands;
mod engine;
mod error;
mod state;

use std::sync::RwLock;

use tauri::Manager;

pub use error::{Error, Result};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = engine::net::client().expect("创建 HTTP 客户端失败");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let paths = state::Paths::detect();
            paths.ensure()?;
            let settings = state::Settings::load(&paths);
            let queue = engine::queue::Queue::new(
                app.handle().clone(),
                client.clone(),
                settings.source.clone(),
            );
            app.manage(state::AppState {
                paths,
                settings: RwLock::new(settings),
                client,
                queue,
                java_cache: Default::default(),
                netplay: Default::default(),
                running_games: Default::default(),
                app: app.handle().clone(),
                account_lock: Default::default(),
            });
            Ok(())
        })
        // 关窗口时把联机子进程收掉：EasyTier / frpc 被 kill 之前不会自己退出，
        // 留着就是两个看不见的后台进程一直占着端口。
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.try_state::<state::AppState>() {
                    state.netplay.stop_all();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_remote_versions,
            commands::list_local_versions,
            commands::delete_version,
            commands::install_version,
            commands::launch_version,
            commands::detect_java,
            commands::download_java,
            commands::game_status,
            commands::game_kill,
            commands::get_settings,
            commands::save_settings,
            commands::queue_snapshot,
            commands::queue_pause,
            commands::queue_resume,
            commands::queue_cancel,
            commands::queue_cancel_group,
            commands::queue_retry_failed,
            commands::queue_clear,
            commands::search_modpacks,
            commands::browse_search,
            commands::browse_install,
            commands::import_modpack,
            commands::install_loader,
            commands::list_loader_versions,
            commands::test_sources,
            commands::memory_plan,
            commands::export_instance,
            commands::import_files,
            commands::list_mods,
            commands::toggle_mod,
            commands::skin_avatar,
            commands::import_skin,
            commands::reset_skin,
            commands::app_paths,
            commands::check_data_directory,
            commands::reveal_in_explorer,
            commands::netplay_status,
            commands::netplay_stop,
            commands::netplay_lan_info,
            commands::netplay_relaunch_admin,
            commands::netplay_easytier_start,
            commands::netplay_portmap_start,
            commands::netplay_frp_login,
            commands::netplay_frp_save_token,
            commands::netplay_frp_start,
            commands::auth_status,
            commands::auth_set_active,
            commands::auth_server_info,
            commands::auth_login,
            commands::auth_select_profile,
            commands::auth_logout,
            commands::auth_ms_begin,
            commands::auth_ms_poll,
            commands::open_in_browser,
            commands::backup_list,
            commands::backup_create,
            commands::backup_restore,
            commands::backup_remove,
            commands::mod_diagnostics,
            commands::copy_version,
            commands::rename_version,
            commands::repair_version,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 启动失败");
}
