//! Outhouse for Windows and Android.
//!
//! A window showing your Outhouse server, plus the things a web page can't do.
//! On Windows: push to talk that works while another app (WoW) is focused,
//! notifications, a tray icon, and starting with Windows. On Android: an app
//! icon and notifications. On both, invite links open in the app:
//! `outhouse://invite/<code>` on Windows, and `https://<server>/invite/<code>`
//! (a verified Android App Link) on Android.

#[cfg(desktop)]
mod ptt;
#[cfg(desktop)]
mod update_state;
#[cfg(desktop)]
mod updates;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

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
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// The Outhouse server these apps show. To move to another domain, change
/// this (or set `OUTHOUSE_SERVER_URL` when building, as CI does) and rebuild.
/// The window only shows pages from here, the updater checks
/// `<server>/updates/latest.json`, and only this site may use the app bridge.
const DEFAULT_SERVER: &str = "https://oh.hartspace.cc";

/// The server's origin, e.g. `https://oh.hartspace.cc` (no trailing slash).
pub(crate) fn server_origin() -> String {
    option_env!("OUTHOUSE_SERVER_URL")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(origin_of)
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// Saved in %APPDATA%\cc.hartspace.outhouse\settings.json. Older versions
/// also saved a chosen `server` there; it's ignored now (and dropped the
/// next time the file is saved).
#[derive(Default, Clone, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    tray_tip_shown: bool,
}

pub(crate) struct AppState {
    settings: Mutex<Settings>,
    path: PathBuf,
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

#[cfg(desktop)]
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

// ---------- invite links ----------

/// The page an invite link should open in the app, e.g. `/invite/AbC123`,
/// from `outhouse://invite/AbC123` (Windows) or
/// `https://<server>/invite/AbC123` (Android App Link). Anything else: None.
fn deep_link_path(link: &str) -> Option<String> {
    let url = url::Url::parse(link.trim().trim_matches('"')).ok()?;
    let code = match url.scheme() {
        // outhouse://invite/CODE (host "invite"), or outhouse:invite/CODE.
        "outhouse" => match url.host_str() {
            Some("invite") => url.path().trim_matches('/').to_string(),
            _ => url
                .path()
                .trim_start_matches('/')
                .strip_prefix("invite/")?
                .trim_end_matches('/')
                .to_string(),
        },
        "https" | "http" if url.origin().ascii_serialization() == server_origin() => url
            .path()
            .strip_prefix("/invite/")?
            .trim_end_matches('/')
            .to_string(),
        _ => return None,
    };
    let ok =
        !code.is_empty() && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| format!("/invite/{code}"))
}

/// Shows an invite link's page in the main window (and brings it forward).
fn open_deep_link(app: &AppHandle, link: &str) -> bool {
    let Some(path) = deep_link_path(link) else {
        return false;
    };
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    if let Ok(url) = url::Url::parse(&format!("{}{path}", server_origin())) {
        let _ = window.navigate(url);
    }
    #[cfg(desktop)]
    show_main(app);
    true
}

// ---------- commands the pages can call ----------

/// The built-in "Connecting to Outhouse…" page asks where to go.
#[tauri::command]
fn get_server() -> String {
    server_origin()
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
/// network). Only the Outhouse server may use it.
fn bridge(
    app: &AppHandle,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let allowed = Some(server_origin());
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
        // The orange update button: what's out, and "Update now".
        #[cfg(desktop)]
        "/update-status" => tauri::http::Response::builder()
            .status(200)
            .header(
                "Access-Control-Allow-Origin",
                origin.clone().unwrap_or_default(),
            )
            .header("Content-Type", "application/json")
            .header("Cache-Control", "no-store")
            .body(updates::status_json(app).into_bytes())
            .unwrap(),
        #[cfg(desktop)]
        "/update-install" => {
            if updates::install_now(app.clone()) {
                reply(200, "ok")
            } else {
                reply(409, "there's no update to install")
            }
        }
        _ => reply(404, "unknown"),
    }
}

/// Tells the website it's inside the app, which kind, which version, and
/// (for the built-in "Connecting…" page) which server and page to open:
/// `start` is an invite link's page when an invite link launched the app.
fn init_script(version: &str, start: Option<&str>) -> String {
    let platform = if cfg!(mobile) { "android" } else { "desktop" };
    let server = serde_json::to_string(&server_origin()).unwrap_or_else(|_| "\"\"".into());
    let start = serde_json::to_string(start.unwrap_or("/")).unwrap_or_else(|_| "\"/\"".into());
    format!(
        "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = '{platform}'; \
         window.__OUTHOUSE_APP_VERSION__ = '{version}'; window.__OUTHOUSE_SERVER__ = {server}; \
         window.__OUTHOUSE_START__ = {start};"
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
            // A second launch just brings the existing window forward; one
            // started by an invite link (outhouse://invite/…) opens the invite.
            .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
                let opened = args.iter().skip(1).any(|a| open_deep_link(app, a));
                if !opened {
                    show_main(app);
                }
            }))
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
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("outhouse", |ctx, request| {
            bridge(ctx.app_handle(), &request)
        })
        .invoke_handler(tauri::generate_handler![
            get_server,
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
            app.manage(AppState {
                settings: Mutex::new(settings),
                path,
            });
            let origin = server_origin();

            // Started by an invite link? Open that invite once connected.
            #[cfg(desktop)]
            let launch_link = std::env::args().skip(1).find_map(|a| deep_link_path(&a));
            #[cfg(mobile)]
            let launch_link: Option<String> = None;
            let start = launch_link.or_else(|| {
                app.deep_link()
                    .get_current()
                    .ok()
                    .flatten()
                    .and_then(|urls| urls.iter().find_map(|u| deep_link_path(u.as_str())))
            });
            // Invite links while the app is running (Android hands them over
            // here; on Windows a second launch does, see single-instance).
            #[cfg(mobile)]
            {
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        if open_deep_link(&handle, url.as_str()) {
                            break;
                        }
                    }
                });
            }
            // Make sure outhouse:// links reach this copy of the app (the
            // installer registers it too; this also covers older installs).
            #[cfg(windows)]
            let _ = app.deep_link().register("outhouse");

            // The window starts on the built-in "Connecting…" page, which moves
            // on to the server. Links anywhere else open in your browser.
            let handle = app.handle().clone();
            let version = app.package_info().version.to_string();
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .initialization_script(init_script(&version, start.as_deref()))
                    .on_navigation(move |url| {
                        let local = url.scheme() == "tauri"
                            || url.host_str() == Some("tauri.localhost")
                            || matches!(url.scheme(), "about" | "data" | "blob");
                        if local {
                            return true;
                        }
                        let here = url.origin().ascii_serialization();
                        if here == origin {
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
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let check_updates = MenuItem::with_id(
        app,
        "check_updates",
        "Check for updates",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Outhouse", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &check_updates,
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
            "check_updates" => updates::check_from_tray(app.clone()),
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
