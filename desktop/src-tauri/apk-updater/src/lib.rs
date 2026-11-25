//! Outhouse for Android updates itself: the Kotlin side (android/) downloads
//! the new APK into the app's private cache, checks it (complete, our package,
//! a newer version, signed with the same key as the installed app) and hands
//! it to Android's PackageInstaller. The app's bridge (src/lib.rs in the app)
//! checks the address first and calls these.

use serde_json::Value;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

pub struct ApkUpdater<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> ApkUpdater<R> {
    /// Runs a Kotlin command: `start` {url, version, origin}, `status`,
    /// `cancel`, `openSettings` or `install`. Each answers with the status.
    pub fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<Value>(command, args)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (command, args);
            Err("only the Android app updates itself this way".into())
        }
    }
}

pub trait ApkUpdaterExt<R: Runtime> {
    fn apk_updater(&self) -> &ApkUpdater<R>;
}

impl<R: Runtime, T: Manager<R>> ApkUpdaterExt<R> for T {
    fn apk_updater(&self) -> &ApkUpdater<R> {
        self.state::<ApkUpdater<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("apk-updater")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let handle = api
                .register_android_plugin("cc.hartspace.outhouse.apkupdater", "ApkUpdaterPlugin")?;
            #[cfg(not(target_os = "android"))]
            let _ = api;
            app.manage(ApkUpdater::<R> {
                #[cfg(target_os = "android")]
                handle,
                _runtime: std::marker::PhantomData,
            });
            Ok(())
        })
        .build()
}
