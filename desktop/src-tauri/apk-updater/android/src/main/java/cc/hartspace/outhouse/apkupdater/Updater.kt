package cc.hartspace.outhouse.apkupdater

import android.app.Activity
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import app.tauri.plugin.JSObject
import java.io.File
import java.io.FileOutputStream
import java.lang.ref.WeakReference
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * One update at a time: download into the app's private cache
 * (cache/apk-updates, never shared Downloads), check it, then hand it to
 * Android's PackageInstaller. The web page polls [status] for progress.
 */
object Updater {
  private const val TAG = "OuthouseUpdater"
  const val ACTION_INSTALL_STATUS = "cc.hartspace.outhouse.apkupdater.INSTALL_STATUS"

  // What the page sees. "idle", "downloading", "verifying", "needs_permission",
  // "installing", "confirm" (Android's install prompt is up), "error".
  @Volatile private var phase = "idle"
  @Volatile private var received = 0L
  @Volatile private var total = 0L
  @Volatile private var version: String? = null
  @Volatile private var error: String? = null
  @Volatile private var errorCode: String? = null
  @Volatile private var note: String? = null
  // Each start (and each cancel) bumps this; a download from an older one stops.
  @Volatile private var gen = 0
  @Volatile private var ready: File? = null // checked and waiting for the permission
  private var worker: Thread? = null
  private var activityRef: WeakReference<Activity>? = null
  private lateinit var app: Context

  fun attach(activity: Activity) {
    app = activity.applicationContext
    activityRef = WeakReference(activity)
  }

  private fun dir(): File = File(app.cacheDir, "apk-updates")

  /** Every start: nothing from an earlier try stays on the phone. */
  fun cleanupOnLaunch() {
    synchronized(this) {
      if (phase == "downloading" || phase == "verifying" || phase == "installing") return
      wipe()
    }
  }

  private fun wipe() {
    try {
      dir().listFiles()?.forEach { it.delete() }
    } catch (e: Exception) {
      Log.w(TAG, "couldn't clear the update folder", e)
    }
    ready = null
  }

  private fun fail(code: String, message: String, my: Int = gen) {
    synchronized(this) {
      if (my != gen) return
      wipe()
      phase = "error"
      errorCode = code
      error = message
    }
  }

  fun canInstall(): Boolean =
    Build.VERSION.SDK_INT < 26 || app.packageManager.canRequestPackageInstalls()

  fun status(): JSObject {
    val o = JSObject()
    o.put("state", phase)
    o.put("received", received)
    o.put("total", total)
    o.put("version", version ?: "")
    o.put("canInstall", canInstall())
    if (error != null) o.put("error", error)
    if (errorCode != null) o.put("code", errorCode)
    if (note != null) o.put("note", note)
    return o
  }

  @Synchronized
  fun start(url: String?, wanted: String?, origin: String?): JSObject {
    if (phase in setOf("downloading", "verifying", "installing", "confirm")) return status()
    if (phase == "needs_permission" && ready?.exists() == true && wanted == version) return status()
    if (!UpdateRules.isAllowedUrl(url, origin)) {
      phase = "error"; errorCode = "refused"; error = "That download isn't from your Outhouse server."
      return status()
    }
    wipe()
    phase = "downloading"; received = 0; total = 0; version = wanted
    error = null; errorCode = null; note = null
    val my = ++gen
    val t = Thread({ run(url!!, wanted, origin!!, my) }, "outhouse-apk-update")
    worker = t
    t.start()
    return status()
  }

  @Synchronized
  fun cancel(): JSObject {
    if (phase == "downloading" || phase == "verifying" || phase == "needs_permission" || phase == "error") {
      gen++
      wipe()
      phase = "idle"; error = null; errorCode = null; note = "cancelled"; received = 0; total = 0
    }
    return status()
  }

  /** Back from Settings, or "Continue": install if we're now allowed to. */
  fun resume() {
    val file = ready
    if (phase == "needs_permission" && file != null && canInstall()) {
      Thread({ install(file) }, "outhouse-apk-install").start()
    }
  }

  fun openSettings(activity: Activity) {
    if (Build.VERSION.SDK_INT < 26) return
    val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:" + app.packageName))
    try {
      activity.startActivity(intent)
    } catch (e: Exception) {
      activity.startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS))
    }
  }

  // ---------- download ----------

  private fun run(start: String, wanted: String?, origin: String, my: Int) {
    fun cancelled() = my != gen
    val folder = dir()
    folder.mkdirs()
    val name = "Outhouse-Android-" + UpdateRules.safeName(wanted)
    val part = File(folder, "$name.apk.part")
    val apk = File(folder, "$name.apk")
    var conn: HttpURLConnection? = null
    try {
      var url = start
      var hops = 0
      while (true) {
        conn = (URL(url).openConnection() as HttpURLConnection).apply {
          instanceFollowRedirects = false
          connectTimeout = 20_000
          readTimeout = 30_000
          useCaches = false
          setRequestProperty("Accept-Encoding", "identity")
          setRequestProperty("Cache-Control", "no-cache")
        }
        val code = conn.responseCode
        if (code in 300..399) {
          val next = UpdateRules.redirectTarget(url, conn.getHeaderField("Location"), origin)
          conn.disconnect()
          if (next == null || ++hops > 3) return fail("refused", "The download was sent somewhere other than your Outhouse server.", my)
          url = next
          continue
        }
        if (code != 200) return fail("network", "The server answered $code.", my)
        break
      }
      val c = conn!!
      val length = c.getHeaderField("Content-Length")?.trim()?.toLongOrNull() ?: -1L
      if (!UpdateRules.sizeOk(length)) return fail("size", "The server didn't say how big the update is.", my)
      total = length
      c.inputStream.use { input ->
        FileOutputStream(part).use { out ->
          val buf = ByteArray(64 * 1024)
          while (true) {
            if (cancelled()) { part.delete(); return }
            val n = input.read(buf)
            if (n < 0) break
            out.write(buf, 0, n)
            received += n
            if (received > length) return fail("size", "The download was bigger than the server said.", my)
          }
          out.fd.sync()
        }
      }
      if (cancelled()) { part.delete(); return }
      if (part.length() != length) return fail("size", "The download stopped early (${part.length()} of $length bytes). Try again.", my)
      if (!part.renameTo(apk)) return fail("io", "Couldn't save the update.", my)
      synchronized(this) { if (cancelled()) return; phase = "verifying" }
      verify(apk, wanted)?.let { return fail("mismatch", it, my) }
      if (cancelled()) { apk.delete(); return }
      synchronized(this) {
        if (cancelled()) { apk.delete(); return }
        ready = apk
        if (!canInstall()) {
          phase = "needs_permission"
          return
        }
      }
      install(apk)
    } catch (e: Exception) {
      Log.w(TAG, "update download failed", e)
      if (!cancelled()) fail("network", "The download didn't finish (${e.javaClass.simpleName}). Check your connection and try again.", my)
      else part.delete()
    } finally {
      conn?.disconnect()
    }
  }

  // ---------- checks ----------

  @Suppress("DEPRECATION")
  private fun packageInfo(archive: File?): PackageInfo? {
    val pm = app.packageManager
    val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES or PackageManager.GET_SIGNATURES
      else PackageManager.GET_SIGNATURES
    return if (archive == null) pm.getPackageInfo(app.packageName, flags)
    else pm.getPackageArchiveInfo(archive.absolutePath, flags)
  }

  @Suppress("DEPRECATION")
  private fun certs(info: PackageInfo): Set<String> {
    val s = if (Build.VERSION.SDK_INT >= 28) info.signingInfo else null
    val sigs = when {
      s == null -> info.signatures
      s.hasMultipleSigners() -> s.apkContentsSigners
      else -> s.signingCertificateHistory
    }
    val sha = MessageDigest.getInstance("SHA-256")
    return (sigs ?: emptyArray()).map { sig -> sha.digest(sig.toByteArray()).joinToString("") { "%02x".format(it) } }.toSet()
  }

  @Suppress("DEPRECATION")
  private fun code(info: PackageInfo): Long =
    if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()

  /** Null when the APK is fine, else why not. */
  private fun verify(apk: File, wanted: String?): String? {
    val mine = packageInfo(null) ?: return "Couldn't read this app's own details."
    val theirs = packageInfo(apk) ?: return "The download isn't a valid Android app."
    if (theirs.packageName != app.packageName) return "The download is a different app (${theirs.packageName})."
    if (!UpdateRules.isNewer(code(theirs), code(mine))) return "The download isn't newer than the Outhouse you have."
    if (!UpdateRules.versionMatches(wanted, theirs.versionName)) return "The download is version ${theirs.versionName}, not $wanted."
    if (!UpdateRules.signersMatch(certs(mine), certs(theirs))) return "The download isn't signed with Outhouse's key."
    return null
  }

  // ---------- install ----------

  private fun install(apk: File) {
    synchronized(this) {
      if (phase == "installing" || phase == "confirm") return
      phase = "installing"
    }
    try {
      val installer = app.packageManager.packageInstaller
      val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
      params.setAppPackageName(app.packageName)
      params.setSize(apk.length())
      if (Build.VERSION.SDK_INT >= 31) params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
      val id = installer.createSession(params)
      installer.openSession(id).use { session ->
        apk.inputStream().use { input ->
          session.openWrite("base.apk", 0, apk.length()).use { out ->
            input.copyTo(out, 64 * 1024)
            session.fsync(out)
          }
        }
        val intent = Intent(app, InstallReceiver::class.java).setAction(ACTION_INSTALL_STATUS).setPackage(app.packageName)
        var flags = PendingIntent.FLAG_UPDATE_CURRENT
        if (Build.VERSION.SDK_INT >= 31) flags = flags or PendingIntent.FLAG_MUTABLE
        val pending = PendingIntent.getBroadcast(app, id, intent, flags)
        session.commit(pending.intentSender)
      }
    } catch (e: Exception) {
      Log.w(TAG, "install hand-off failed", e)
      return fail("install", "Android wouldn't take the update (${e.message ?: e.javaClass.simpleName}).")
    } finally {
      // The installer has its own copy now (or it failed): ours goes.
      apk.delete()
      ready = null
    }
  }

  /** From [InstallReceiver]: what Android's installer said. */
  @Suppress("DEPRECATION")
  fun onInstallStatus(context: Context, intent: Intent) {
    val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
    val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
    when (status) {
      PackageInstaller.STATUS_PENDING_USER_ACTION -> {
        val confirm: Intent? = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
          else intent.getParcelableExtra(Intent.EXTRA_INTENT)
        if (confirm == null) return fail("install", "Android didn't show its install prompt.")
        phase = "confirm"
        val activity = activityRef?.get()
        try {
          if (activity != null && !activity.isFinishing) activity.startActivity(confirm)
          else context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: Exception) {
          fail("install", "Couldn't show Android's install prompt (${e.message}).")
        }
      }
      PackageInstaller.STATUS_SUCCESS -> {
        // Usually Android has already restarted us into the new version.
        synchronized(this) { wipe(); phase = "idle"; note = "installed" }
      }
      PackageInstaller.STATUS_FAILURE_ABORTED -> {
        synchronized(this) { wipe(); phase = "idle"; note = "cancelled" }
      }
      else -> {
        val why = when (status) {
          PackageInstaller.STATUS_FAILURE_CONFLICT -> "it conflicts with the installed app"
          PackageInstaller.STATUS_FAILURE_INCOMPATIBLE -> "it doesn't fit this phone"
          PackageInstaller.STATUS_FAILURE_INVALID -> "the file isn't valid"
          PackageInstaller.STATUS_FAILURE_STORAGE -> "the phone is short of space"
          PackageInstaller.STATUS_FAILURE_BLOCKED -> "it was blocked"
          else -> message ?: "unknown reason"
        }
        fail("install", "Android didn't install the update: $why.")
      }
    }
  }
}
