package cc.hartspace.outhouse.apkupdater

import java.net.URI

/**
 * The decisions behind an in-app update, kept free of Android classes so the
 * JVM unit tests (src/test) can check them.
 */
object UpdateRules {
  /** Largest APK we'll download (today's is ~15 MB). */
  const val MAX_BYTES: Long = 300L * 1024 * 1024

  private fun port(u: URI): Int = if (u.port == -1) 443 else u.port

  /** Same https origin (scheme, host, port) as the app's Outhouse server? */
  fun sameOrigin(url: String?, origin: String?): Boolean {
    if (url.isNullOrBlank() || origin.isNullOrBlank()) return false
    return try {
      val u = URI(url)
      val o = URI(origin)
      u.scheme.equals("https", ignoreCase = true) &&
        o.scheme.equals("https", ignoreCase = true) &&
        u.rawUserInfo == null &&
        !u.host.isNullOrEmpty() &&
        u.host.equals(o.host, ignoreCase = true) &&
        port(u) == port(o)
    } catch (e: Exception) {
      false
    }
  }

  /** An APK under /updates/ on the app's own server, over https. */
  fun isAllowedUrl(url: String?, origin: String?): Boolean {
    if (!sameOrigin(url, origin)) return false
    return try {
      val path = URI(url).path ?: return false
      path.startsWith("/updates/") && path.endsWith(".apk") && !path.contains("..")
    } catch (e: Exception) {
      false
    }
  }

  /** Where a redirect points, if it stays on the same server (else null). */
  fun redirectTarget(from: String, location: String?, origin: String): String? {
    if (location.isNullOrBlank()) return null
    return try {
      val next = URI(from).resolve(location.trim()).toString()
      if (sameOrigin(next, origin)) next else null
    } catch (e: Exception) {
      null
    }
  }

  /** Android only installs over the top when the version code goes up. */
  fun isNewer(candidateCode: Long, installedCode: Long): Boolean = candidateCode > installedCode

  /** The APK is the version the server offered ("0.2.512" == "v0.2.512"); no offer means anything. */
  fun versionMatches(expected: String?, actual: String?): Boolean {
    if (expected.isNullOrBlank()) return true
    if (actual.isNullOrBlank()) return false
    return expected.trim().removePrefix("v") == actual.trim().removePrefix("v")
  }

  /**
   * Signed with the installed app's key? Both are sets of SHA-256 digests of
   * the signing certificates. The download must carry every certificate the
   * installed app has (with key rotation, its history includes the old one).
   */
  fun signersMatch(installed: Set<String>, archive: Set<String>): Boolean =
    installed.isNotEmpty() && archive.isNotEmpty() && archive.containsAll(installed)

  /** A size we can trust: the server said how big it is, and it's sane. */
  fun sizeOk(contentLength: Long): Boolean = contentLength in 1..MAX_BYTES

  /** "0.2.512" -> "0.2.512"; anything odd becomes "_" (it's only a file name). */
  fun safeName(version: String?): String {
    val v = (version ?: "").take(40).map { if (it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '.' || it == '-') it else '_' }.joinToString("")
    return if (v.isEmpty() || v.all { it == '.' }) "update" else v
  }
}
