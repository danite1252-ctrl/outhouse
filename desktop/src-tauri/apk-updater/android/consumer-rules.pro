# The install-result receiver is named in the manifest (kept by aapt's rules);
# the plugin class is kept by Tauri's @TauriPlugin rule. Keep both by name too.
-keep class cc.hartspace.outhouse.apkupdater.** { *; }
