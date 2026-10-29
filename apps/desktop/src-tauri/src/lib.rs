mod agent;
mod commands;

use agent::AgentHandle;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // Auto-update is only compiled into release builds that carry an
            // updater public key (see tauri.updater.conf.json and the release workflow).
            #[cfg(feature = "updater")]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app.handle().plugin(tauri_plugin_process::init())?;
            }
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            let handle = AgentHandle::spawn(app.handle().clone());
            app.manage(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::rpc_call, commands::agent_status])
        .run(tauri::generate_context!())
        .expect("error while running the OurShell desktop app");
}
