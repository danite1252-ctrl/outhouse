//! Automatic updates for the Windows app.
//!
//! Builds are published on GitHub; your Outhouse server copies them to
//! `/updates/` (see deploy/fetch-updates.py). The app checks
//! `<server>/updates/latest.json` (the server is `DEFAULT_SERVER` in lib.rs), and only installs an update signed with
//! the Outhouse signing key, so a tampered download is refused.
//!
//! - At start-up: if there's an update, install it straight away (you're not
//!   in voice yet) and restart.
//! - Every 6 hours after that: if there's an update, say so; it installs the
//!   next time Outhouse starts.

use std::time::Duration;

use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Public half of the signing key, baked in at build time by CI. Without it
/// (local builds) updates are switched off.
const PUBKEY: Option<&str> = option_env!("OUTHOUSE_UPDATER_PUBKEY");
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

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
        let mut first = true;
        loop {
            let result = tauri::async_runtime::block_on(check(&app, first));
            if let Err(e) = result {
                eprintln!("update check failed: {e}");
            }
            first = false;
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

async fn check(app: &AppHandle, install_now: bool) -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::server_origin();
    let url = format!("{server}/updates/latest.json").parse()?;
    let updater = app.updater_builder().endpoints(vec![url])?.build()?;
    let Some(update) = updater.check().await? else {
        return Ok(());
    };
    if install_now {
        let _ = app
            .notification()
            .builder()
            .title("Updating Outhouse")
            .body(format!("Installing version {}…", update.version))
            .show();
        update.download_and_install(|_, _| {}, || {}).await?;
        app.restart();
    } else {
        let _ = app
            .notification()
            .builder()
            .title("Outhouse update ready")
            .body(format!(
                "Version {} installs the next time you start Outhouse.",
                update.version
            ))
            .show();
    }
    Ok(())
}
