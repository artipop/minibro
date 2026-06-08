use std::sync::Mutex;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};

/// Hide the native traffic-light buttons (close/minimize/zoom).
/// Must be called on the main thread. Safe to call in setup().
#[cfg(target_os = "macos")]
fn remove_traffic_lights(window: &tauri::WebviewWindow<tauri::Cef>) {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let Ok(raw) = window.ns_window() else { return };
    let Some(ns_win) = (unsafe { Retained::<NSWindow>::retain(raw as _) }) else { return };
    for btn in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(b) = ns_win.standardWindowButton(btn) {
            b.setHidden(true);
        }
    }
}

struct TrayShown(Mutex<bool>);

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn show_tray_window(app: tauri::AppHandle<tauri::Cef>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("tray_browser") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn navigate_tray(app: tauri::AppHandle<tauri::Cef>, url: String) -> Result<(), String> {
    let window = app
        .get_webview_window("tray_browser")
        .ok_or("tray_browser not found")?;
    let parsed = url.parse::<tauri::Url>().map_err(|e| e.to_string())?;
    window.navigate(parsed).map_err(|e| e.to_string())
}

#[tauri::command]
fn eval_in_tray(app: tauri::AppHandle<tauri::Cef>, script: String) -> Result<(), String> {
    let window = app
        .get_webview_window("tray_browser")
        .ok_or("tray_browser not found")?;
    window.eval(&script).map_err(|e| e.to_string())
}

#[tauri::command]
fn cdp_eval(script: String) -> Result<String, String> {
    let ws_url = cdp_find_tray_ws()?;
    let (mut ws, _) =
        tungstenite::connect(&ws_url).map_err(|e| format!("WS connect failed: {e}"))?;

    let msg = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": { "expression": script, "returnByValue": true, "awaitPromise": true }
    });
    ws.send(tungstenite::Message::Text(msg.to_string().into()))
        .map_err(|e| format!("WS send: {e}"))?;

    let response = ws.read().map_err(|e| format!("WS read: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&response.to_string()).map_err(|e| e.to_string())?;

    if let Some(exc) = json["result"]["exceptionDetails"].as_object() {
        return Err(format!("JS exception: {exc:?}"));
    }

    let val = &json["result"]["result"]["value"];
    Ok(if val.is_string() {
        val.as_str().unwrap().to_string()
    } else {
        val.to_string()
    })
}

#[tauri::command]
fn cdp_get_html() -> Result<String, String> {
    cdp_eval("document.documentElement.outerHTML".into())
}

#[tauri::command]
fn cdp_list_targets() -> Result<String, String> {
    let targets: serde_json::Value = ureq::get("http://localhost:9229/json")
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))?;
    Ok(serde_json::to_string_pretty(&targets).unwrap())
}

// ── CDP helpers ───────────────────────────────────────────────────────────────

fn cdp_find_tray_ws() -> Result<String, String> {
    let targets: serde_json::Value = ureq::get("http://localhost:9229/json")
        .call()
        .map_err(|e| format!("CDP HTTP failed: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))?;

    let arr = targets.as_array().ok_or("CDP response is not array")?;

    // Main Tauri window is at localhost:1420 — skip it; everything else is tray browser
    let target = arr
        .iter()
        .find(|t| {
            let url = t["url"].as_str().unwrap_or("");
            t["type"].as_str() == Some("page")
                && !url.starts_with("tauri://")
                && !url.starts_with("http://localhost:1420")
        })
        .ok_or_else(|| {
            let urls: Vec<&str> = arr.iter().filter_map(|t| t["url"].as_str()).collect();
            format!("No tray browser target found. Targets: {urls:?}")
        })?;

    target["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No webSocketDebuggerUrl".to_string())
}

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::<tauri::Cef>::new()
        .command_line_args([("remote-debugging-port", Some("9229"))])
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(TrayShown(Mutex::new(false)))
        .setup(|app| {
            let app_handle = app.handle().clone();

            // Create tray browser hidden at startup so CDP target exists immediately.
            let tray_win = WebviewWindowBuilder::new(
                app,
                "tray_browser",
                WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .decorations(false)
            .always_on_top(true)
            .inner_size(800.0, 600.0)
            .visible(false)
            .build()?;

            #[cfg(target_os = "macos")]
            remove_traffic_lights(&tray_win);

            // Main window: hide on close so the app keeps running (dock dot stays).
            if let Some(main_win) = app.get_webview_window("main") {
                let win = main_win.clone();
                main_win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win.hide();
                    }
                });
            }

            // Intercept the close button: hide instead of destroy so the window
            // (and its CDP target) stays alive for the agent.
            let app_for_close = app_handle.clone();
            tray_win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    *app_for_close.state::<TrayShown>().0.lock().unwrap() = false;
                }
            });
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .on_tray_icon_event(move |_tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        let Some(window) = app_handle.get_webview_window("tray_browser") else {
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

                        let x = (icon_x + icon_w / 2.0 - 400.0) as i32;
                        let y = (icon_y + icon_h) as i32;

                        let shown = app_handle.state::<TrayShown>();
                        let mut shown = shown.0.lock().unwrap();
                        let is_visible = window.is_visible().unwrap_or(false);
                        let is_minimized = window.is_minimized().unwrap_or(false);

                        if is_visible && !is_minimized {
                            // Window is on screen — hide it
                            let _ = window.hide();
                            *shown = false;
                        } else {
                            // Hidden or minimized via title bar — restore to tray position
                            if is_minimized {
                                let _ = window.unminimize();
                            }
                            let _ = window.set_position(PhysicalPosition::new(x, y));
                            let _ = window.show();
                            let _ = window.set_focus();
                            *shown = true;
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            show_tray_window,
            navigate_tray,
            eval_in_tray,
            cdp_eval,
            cdp_get_html,
            cdp_list_targets,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Dock icon clicked while app is running — restore main window.
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_minimized().unwrap_or(false) {
                        let _ = w.unminimize();
                    }
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });
}
