pub mod notifications;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::<tauri::Cef>::new()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            notifications::ax_check_permission,
            notifications::get_notifications,
            notifications::click_notification,
            notifications::start_notification_watcher,
            notifications::get_notification_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
