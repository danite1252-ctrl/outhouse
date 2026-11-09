//! Automatic updates for the Windows app.
//!
//! Builds are published on GitHub; your Outhouse server copies them to
//! `/updates/` (see deploy/fetch-updates.py). The app checks
//! `<server>/updates/latest.json` (the server is `DEFAULT_SERVER` in lib.rs), and only installs an update signed with
//! the Outhouse signing key, so a tampered download is refused.
//!
//! - At start-up: if there's an update, install it straight away (you're not
//!   in voice yet) and restart.
//! - Every hour after that (and from the tray's "Check for updates"): if
//!   there's an update, remember it and say so. The page shows an orange
//!   arrow at the top right; it asks `http://outhouse.localhost/update-status`
//!   what's out, and `/update-install` downloads it (with progress), installs
//!   it (the NSIS installer in passive mode) and restarts Outhouse. Nothing
//!   restarts by itself mid-session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::update_state::UpdateState;

/// Public half of the signing key, baked in at build time by CI. Without it
/// (local builds) updates are switched off.
const PUBKEY: Option<&str> = option_env!("OUTHOUSE_UPDATER_PUBKEY");
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// What the update button shows.
static STATE: LazyLock<Mutex<UpdateState>> = LazyLock::new(|| Mutex::new(UpdateState::default()));
/// The update the last check found (what "Update now" installs).
static PENDING: Mutex<Option<Update>> = Mutex::new(None);
/// A check is running (the hourly one and the tray's don't overlap).
static CHECKING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
enum Check {
    /// At start-up: install straight away.
    Startup,
    /// The hourly check: quiet unless there's something new.
    Background,
    /// The tray's "Check for updates": always says what it found.
    Manual,
}

pub fn enabled() -> bool {
    PUBKEY.is_some_and(|k| !k.trim().is_empty())
}

pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    let mut builder = tauri_plugin_updater::Builder::new();
    if let Some(key) = PUBKEY.filter(|k| !k.trim().is_empty()) {
        builder = builder.pubkey(key.trim());
    }
    builder.build()
}

pub fn start(app: AppHandle) {
    if !enabled() {
        return;
    }
    std::thread::spawn(move || {
        let mut mode = Check::Startup;
        loop {
            run_check(&app, mode);
            mode = Check::Background;
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// The tray's "Check for updates".
pub fn check_from_tray(app: AppHandle) {
    if !enabled() {
        notify(
            &app,
            "Updates are off",
            "This copy of Outhouse was built without the update key, so it can't update itself.",
        );
        return;
    }
    std::thread::spawn(move || run_check(&app, Check::Manual));
}

fn run_check(app: &AppHandle, mode: Check) {
    if CHECKING.swap(true, Ordering::SeqCst) {
        if mode == Check::Manual {
            notify(
                app,
                "Checking for updates",
                "Already looking, hang on a tick.",
            );
        }
        return;
    }
    let result = tauri::async_runtime::block_on(check(app, mode));
    CHECKING.store(false, Ordering::SeqCst);
    if let Err(e) = result {
        eprintln!("update check failed: {e}");
        if mode == Check::Manual {
            notify(
                app,
                "Couldn't check for updates",
                &format!("Outhouse couldn't reach the server: {e}"),
            );
        }
    }
}

async fn check(app: &AppHandle, mode: Check) -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::server_origin();
    let url = format!("{server}/updates/latest.json").parse()?;
    let updater = app.updater_builder().endpoints(vec![url])?.build()?;
    let Some(update) = updater.check().await? else {
        STATE.lock().unwrap().none_found();
        let downloading = STATE.lock().unwrap().downloading;
        if !downloading {
            *PENDING.lock().unwrap() = None;
        }
        if mode == Check::Manual {
            let current = app.package_info().version.to_string();
            notify(
                app,
                "You're up to date",
                &format!("Outhouse {current} is the latest version."),
            );
        }
        nudge_page(app);
        return Ok(());
    };
    let date = update.date.map(|d| d.to_string());
    let news =
        STATE
            .lock()
            .unwrap()
            .found(&update.version, update.body.as_deref(), date.as_deref());
    let version = update.version.clone();
    let busy = {
        let s = STATE.lock().unwrap();
        s.downloading || s.ready
    };
    if !busy {
        *PENDING.lock().unwrap() = Some(update.clone());
    }

    if mode == Check::Startup {
        // Nothing's going on yet, so install now (as before).
        notify(
            app,
            "Updating Outhouse",
            &format!("Installing version {version}…"),
        );
        if install_inner(app).await.is_ok() {
            return Ok(());
        }
        // Didn't work: the orange button offers it again.
        nudge_page(app);
        return Ok(());
    }

    if news || mode == Check::Manual {
        notify(
            app,
            "Outhouse update ready",
            &format!(
                "Version {version} is out. Click the orange arrow at the top right to update."
            ),
        );
    }
    nudge_page(app);
    Ok(())
}

/// The update button's state, as JSON for the page.
pub fn status_json(app: &AppHandle) -> String {
    let s = STATE.lock().unwrap().clone();
    serde_json::json!({
        "enabled": enabled(),
        "current": app.package_info().version.to_string(),
        "available": s.available(),
        "version": s.version,
        "notes": s.notes,
        "date": s.date,
        "downloading": s.downloading,
        "progress": s.progress(),
        "ready": s.ready,
        "error": s.error,
    })
    .to_string()
}

/// "Update now": download (progress shows on the button), install and
/// restart. False when there's no update to install or it's already going.
pub fn install_now(app: AppHandle) -> bool {
    if PENDING.lock().unwrap().is_none() {
        return false;
    }
    if !STATE.lock().unwrap().start_download() {
        // Already downloading or installing counts as fine.
        let s = STATE.lock().unwrap();
        return s.downloading || s.ready;
    }
    tauri::async_runtime::spawn(async move {
        let _ = download_and_install(&app).await;
    });
    true
}

/// Start-up path: mark it downloading, then the same as "Update now".
async fn install_inner(app: &AppHandle) -> Result<(), String> {
    if !STATE.lock().unwrap().start_download() {
        return Err("busy".into());
    }
    download_and_install(app).await
}

async fn download_and_install(app: &AppHandle) -> Result<(), String> {
    let pending = PENDING.lock().unwrap().clone();
    let Some(update) = pending else {
        STATE
            .lock()
            .unwrap()
            .failed("the update went missing, try again");
        return Err("no update".into());
    };
    let bytes = match update
        .download(|len, total| STATE.lock().unwrap().chunk(len, total), || {})
        .await
    {
        Ok(b) => b,
        Err(e) => {
            let why = format!("download failed: {e}");
            STATE.lock().unwrap().failed(&why);
            return Err(why);
        }
    };
    STATE.lock().unwrap().finished();
    // Give the page a moment to show "Restarting…".
    tokio_sleep(Duration::from_millis(800)).await;
    // On Windows this starts the installer (passive: a small progress
    // window, no questions) and closes Outhouse; it opens again afterwards.
    if let Err(e) = update.install(bytes) {
        let why = format!("install failed: {e}");
        STATE.lock().unwrap().failed(&why);
        return Err(why);
    }
    app.restart();
}

async fn tokio_sleep(d: Duration) {
    // No async timer crate here: sleep on a blocking thread.
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Tells the page to ask for the update state now instead of waiting for
/// its next poll. (It polls the bridge anyway, so this is only a nudge.)
fn nudge_page(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval("window.dispatchEvent(new Event('outhouse-update'))");
    }
}
