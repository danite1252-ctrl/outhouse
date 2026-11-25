package cc.hartspace.outhouse.apkupdater

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Android's PackageInstaller tells us here how the install went (not exported: only via our PendingIntent). */
class InstallReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    if (intent.action != Updater.ACTION_INSTALL_STATUS) return
    Updater.onInstallStatus(context, intent)
  }
}
