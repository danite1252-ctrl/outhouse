package cc.hartspace.outhouse.apkupdater

import android.app.Activity
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
  var url: String? = null
  var version: String? = null
  var origin: String? = null
}

/**
 * The app's bridge (src/lib.rs, /apk-update-*) calls these. Each answers
 * with [Updater.status].
 */
@TauriPlugin
class ApkUpdaterPlugin(private val activity: Activity) : Plugin(activity) {
  init {
    Updater.attach(activity)
    // A fresh start: no update file from before stays behind.
    Updater.cleanupOnLaunch()
  }

  override fun load(webView: WebView) {
    Updater.attach(activity)
  }

  override fun onResume(activity: AppCompatActivity) {
    Updater.attach(activity)
    // Back from "Install unknown apps": carry on with the install.
    Updater.resume()
  }

  @Command
  fun start(invoke: Invoke) {
    val args = invoke.parseArgs(StartArgs::class.java)
    invoke.resolve(Updater.start(args.url, args.version, args.origin))
  }

  @Command
  fun status(invoke: Invoke) {
    invoke.resolve(Updater.status())
  }

  @Command
  fun cancel(invoke: Invoke) {
    invoke.resolve(Updater.cancel())
  }

  @Command
  fun openSettings(invoke: Invoke) {
    Updater.openSettings(activity)
    invoke.resolve(Updater.status())
  }

  @Command
  fun install(invoke: Invoke) {
    Updater.resume()
    invoke.resolve(Updater.status())
  }
}
