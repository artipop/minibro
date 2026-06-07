use std::sync::Mutex;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};

struct TrayUrl(Mutex<String>);

#[tauri::command]
fn set_tray_url(state: tauri::State<TrayUrl>, url: String) {
    *state.0.lock().unwrap() = url;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::<tauri::Cef>::new()
        .command_line_args([("remote-debugging-port", Some("9229"))])
        .plugin(tauri_plugin_opener::init())
        .manage(TrayUrl(Mutex::new(String::new())))
        .setup(|app| {
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        let url_str = app.state::<TrayUrl>().0.lock().unwrap().clone();
                        if url_str.is_empty() {
                            return;
                        }
                        let Ok(parsed) = url_str.parse::<tauri::Url>() else {
                            return;
                        };

                        let icon_x = match &rect.position {
                            tauri::Position::Physical(p) => p.x as f64,
                            tauri::Position::Logical(p) => p.x,
                        };
                        let icon_y = match &rect.position {
                            tauri::Position::Physical(p) => p.y as f64,
                            tauri::Position::Logical(p) => p.y,
                        };
                        let icon_w = match &rect.size {
                            tauri::Size::Physical(s) => s.width as f64,
                            tauri::Size::Logical(s) => s.width,
                        };
                        let icon_h = match &rect.size {
                            tauri::Size::Physical(s) => s.height as f64,
                            tauri::Size::Logical(s) => s.height,
                        };

                        // Center window horizontally under tray icon
                        let x_phys = icon_x + icon_w / 2.0 - 400.0;
                        let y_phys = icon_y + icon_h;

                        if let Some(window) = app.get_webview_window("tray_browser") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.navigate(parsed);
                                let _ = window.set_position(PhysicalPosition::new(
                                    x_phys as i32,
                                    y_phys as i32,
                                ));
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        } else {
                            let scale = app
                                .primary_monitor()
                                .ok()
                                .flatten()
                                .map(|m| m.scale_factor())
                                .unwrap_or(1.0);
                            let _ = WebviewWindowBuilder::new(
                                app,
                                "tray_browser",
                                WebviewUrl::External(parsed),
                            )
                            .decorations(false)
                            .always_on_top(true)
                            .inner_size(800.0, 600.0)
                            .position(x_phys / scale, y_phys / scale)
                            .build();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![set_tray_url])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
