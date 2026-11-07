//! Outhouse for Windows and Android.
//!
//! A window showing your Outhouse server, plus the things a web page can't do.
//! On Windows: push to talk that works while another app (WoW) is focused,
//! notifications, a tray icon, and starting with Windows. On Android: an app
//! icon and notifications.

#[cfg(desktop)]
mod ptt;
#[cfg(desktop)]
mod updates;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
#[cfg(desktop)]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
#[cfg(desktop)]
use tauri::{UserAttentionType, WindowEvent};
#[cfg(desktop)]
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// Saved in %APPDATA%\cc.hartspace.outhouse\settings.json
#[derive(Default, Clone, Serialize, Deserialize)]
struct Settings {
    server: Option<String>,
    #[serde(default)]
    tray_tip_shown: bool,
}

pub(crate) struct AppState {
    settings: Mutex<Settings>,
    path: PathBuf,
    /// Origin of the chosen server (e.g. https://oh.hartspace.cc). The window
    /// only shows pages from here; any other link opens in your browser.
    pub(crate) origin: Arc<Mutex<Option<String>>>,
}

impl AppState {
    fn save(&self) {
        let settings = self.settings.lock().unwrap().clone();
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&settings) {
            let _ = fs::write(&self.path, json);
        }
    }
}

fn origin_of(address: &str) -> Option<String> {
    let url = url::Url::parse(address).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

/// The app's own "choose a server" page.
#[cfg(desktop)]
fn local_page(query: &str) -> String {
    if cfg!(windows) {
        format!("http://tauri.localhost/index.html{query}")
    } else {
        format!("tauri://localhost/index.html{query}")
    }
}

#[cfg(desktop)]
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

// ---------- commands the pages can call ----------

#[tauri::command]
fn get_server(state: State<AppState>) -> Option<String> {
    state.settings.lock().unwrap().server.clone()
}

#[tauri::command]
fn set_server(address: String, state: State<AppState>) -> Result<String, String> {
    let address = address.trim().trim_end_matches('/');
    let full = if address.contains("://") {
        address.to_string()
    } else {
        format!("https://{address}")
    };
    let origin = origin_of(&full).ok_or("That doesn't look like a web address.")?;
    state.settings.lock().unwrap().server = Some(origin.clone());
    *state.origin.lock().unwrap() = Some(origin.clone());
    state.save();
    Ok(origin)
}

/// Which key or mouse button is push to talk right now, or none (not in
/// voice, or using voice activity). Only that one key is watched.
#[tauri::command]
fn set_ptt(code: Option<String>) -> bool {
    #[cfg(desktop)]
    return ptt::set_binding(code.as_deref());
    #[cfg(mobile)]
    return code.is_none();
}

#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) {
    let _ = app.notification().builder().title(title).body(body).show();
    #[cfg(desktop)]
    if let Some(w) = app.get_webview_window("main") {
        if !w.is_focused().unwrap_or(false) {
            let _ = w.request_user_attention(Some(UserAttentionType::Informational));
        }
    }
}

#[tauri::command]
fn open_external(app: AppHandle, url: String) {
    if url.starts_with("https://") || url.starts_with("http://") {
        let _ = app.opener().open_url(url, None::<&str>);
    }
}

/// The website talks to the app by fetching http://outhouse.localhost/...
/// (Windows routes that straight to this handler, nothing goes on the
/// network). Only the server you chose may use it.
fn bridge(
    app: &AppHandle,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let allowed = app.state::<AppState>().origin.lock().unwrap().clone();
    let origin = request
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let reply = |status: u16, text: &str| {
        tauri::http::Response::builder()
            .status(status)
            .header(
                "Access-Control-Allow-Origin",
                origin.clone().unwrap_or_default(),
            )
            .header("Content-Type", "text/plain")
            .body(text.as_bytes().to_vec())
            .unwrap()
    };
    if origin.is_none() || origin != allowed {
        return reply(403, "only your Outhouse server can do that");
    }
    let query: HashMap<String, String> = url::Url::parse(&request.uri().to_string())
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default();
    let get = |k: &str| query.get(k).cloned().unwrap_or_default();
    match request.uri().path() {
        "/ping" => reply(200, "ok"),
        "/ptt" => {
            let code = get("code");
            if set_ptt((!code.is_empty()).then_some(code)) {
                reply(200, "ok")
            } else {
                reply(400, "that key isn't supported")
            }
        }
        "/notify" => {
            notify(app.clone(), get("title"), get("body"));
            reply(200, "ok")
        }
        "/open" => {
            open_external(app.clone(), get("url"));
            reply(200, "ok")
        }
        _ => reply(404, "unknown"),
    }
}

/// Tells the website it's inside the app, which kind, and which version.
fn init_script(version: &str) -> String {
    let platform = if cfg!(mobile) { "android" } else { "desktop" };
    format!(
        "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = '{platform}'; \
         window.__OUTHOUSE_APP_VERSION__ = '{version}';"
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    let start_hidden = std::env::args().any(|a| a == "--minimized");

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder
            // A second launch just brings the existing window forward.
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
            .plugin(tauri_plugin_autostart::init(
                MacosLauncher::LaunchAgent,
                Some(vec!["--minimized"]),
            ))
            // Closing the window keeps Outhouse running in the tray, like Discord.
            .on_window_event(|window, event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    if window.label() == "main" {
                        api.prevent_close();
                        let _ = window.hide();
                        let app = window.app_handle();
                        let state = app.state::<AppState>();
                        let first_time = !state.settings.lock().unwrap().tray_tip_shown;
                        if first_time {
                            state.settings.lock().unwrap().tray_tip_shown = true;
                            state.save();
                            let _ = app
                                .notification()
                                .builder()
                                .title("Outhouse is still running")
                                .body("It's in the tray, so push to talk and notifications keep working. Right-click the tray icon to quit.")
                                .show();
                        }
                    }
                }
            });
    }

    #[cfg(desktop)]
    if updates::enabled() {
        builder = builder.plugin(updates::plugin());
    }

    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("outhouse", |ctx, request| {
            bridge(ctx.app_handle(), &request)
        })
        .invoke_handler(tauri::generate_handler![
            get_server,
            set_server,
            set_ptt,
            notify,
            open_external
        ])
        .setup(move |app| {
            let path = app.path().app_config_dir()?.join("settings.json");
            let settings: Settings = fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let origin = Arc::new(Mutex::new(settings.server.as_deref().and_then(origin_of)));
            app.manage(AppState {
                settings: Mutex::new(settings),
                path,
                origin: origin.clone(),
            });

            // The window. Links to anywhere but your server open in your browser.
            let handle = app.handle().clone();
            let version = app.package_info().version.to_string();
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .initialization_script(init_script(&version))
                    .on_navigation(move |url| {
                        let local = url.scheme() == "tauri"
                            || url.host_str() == Some("tauri.localhost")
                            || matches!(url.scheme(), "about" | "data" | "blob");
                        if local {
                            return true;
                        }
                        let here = url.origin().ascii_serialization();
                        if origin.lock().unwrap().as_deref() == Some(here.as_str()) {
                            return true;
                        }
                        if matches!(url.scheme(), "http" | "https") {
                            let _ = handle.opener().open_url(url.as_str(), None::<&str>);
                        }
                        false
                    });
            #[cfg(desktop)]
            let window = window
                .title("Outhouse")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                .visible(!start_hidden);
            window.build()?;

            #[cfg(desktop)]
            {
                setup_tray(app)?;
                ptt::start(app.handle().clone());
                updates::start(app.handle().clone());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Outhouse failed to start");
}

#[cfg(desktop)]
fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    // Tray icon and menu.
    let open = MenuItem::with_id(app, "open", "Open Outhouse", true, None::<&str>)?;
    let server = MenuItem::with_id(app, "server", "Change server…", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Outhouse", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &server,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let autostart_item = autostart.clone();
    TrayIconBuilder::with_id("tray")
        .tooltip("Outhouse")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "server" => {
                show_main(app);
                if let Some(w) = app.get_webview_window("main") {
                    if let Ok(url) = local_page("?change=1").parse() {
                        let _ = w.navigate(url);
                    }
                }
            }
            "autostart" => {
                let launcher = app.autolaunch();
                let on = launcher.is_enabled().unwrap_or(false);
                let _ = if on {
                    launcher.disable()
                } else {
                    launcher.enable()
                };
                let _ = autostart_item.set_checked(!on);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
