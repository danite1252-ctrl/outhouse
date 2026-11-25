package cc.hartspace.outhouse.apkupdater

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdateRulesTest {
  private val origin = "https://oh.hartspace.cc"

  @Test
  fun onlyOurServerOverHttps() {
    assertTrue(UpdateRules.isAllowedUrl("https://oh.hartspace.cc/updates/Outhouse-Android-0.2.9.apk", origin))
    assertTrue(UpdateRules.isAllowedUrl("https://OH.hartspace.cc:443/updates/Outhouse-Android.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("http://oh.hartspace.cc/updates/Outhouse-Android.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://evil.example/updates/Outhouse-Android.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc.evil.example/updates/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc@evil.example/updates/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://user@oh.hartspace.cc/updates/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc:8443/updates/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc/uploads/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc/updates/x.exe", origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc/updates/../api/x.apk", origin))
    assertFalse(UpdateRules.isAllowedUrl("not a url", origin))
    assertFalse(UpdateRules.isAllowedUrl(null, origin))
    assertFalse(UpdateRules.isAllowedUrl("https://oh.hartspace.cc/updates/x.apk", "http://oh.hartspace.cc"))
  }

  @Test
  fun redirectsStayOnTheServer() {
    val from = "https://oh.hartspace.cc/updates/Outhouse-Android.apk"
    assertEquals("https://oh.hartspace.cc/updates/Outhouse-Android-0.2.9.apk",
      UpdateRules.redirectTarget(from, "/updates/Outhouse-Android-0.2.9.apk", origin))
    assertNull(UpdateRules.redirectTarget(from, "https://objects.githubusercontent.com/x.apk", origin))
    assertNull(UpdateRules.redirectTarget(from, "http://oh.hartspace.cc/updates/x.apk", origin))
    assertNull(UpdateRules.redirectTarget(from, null, origin))
  }

  @Test
  fun versions() {
    assertTrue(UpdateRules.isNewer(100513, 100512))
    assertFalse(UpdateRules.isNewer(100512, 100512))
    assertFalse(UpdateRules.isNewer(2005, 100512))
    assertTrue(UpdateRules.versionMatches("0.2.513", "0.2.513"))
    assertTrue(UpdateRules.versionMatches("v0.2.513", "0.2.513"))
    assertTrue(UpdateRules.versionMatches(null, "0.2.513"))
    assertFalse(UpdateRules.versionMatches("0.2.513", "0.2.512"))
    assertFalse(UpdateRules.versionMatches("0.2.513", null))
  }

  @Test
  fun signers() {
    val ours = setOf("d6314591e5fa44c082282792c52831883bfe0cfd85632b518be504fe830f8ae3")
    assertTrue(UpdateRules.signersMatch(ours, ours))
    // Key rotation: the new APK's history still includes our key.
    assertTrue(UpdateRules.signersMatch(ours, ours + "aaaa"))
    assertFalse(UpdateRules.signersMatch(ours, setOf("aaaa")))
    assertFalse(UpdateRules.signersMatch(ours, emptySet()))
    assertFalse(UpdateRules.signersMatch(emptySet(), emptySet()))
  }

  @Test
  fun sizesAndNames() {
    assertTrue(UpdateRules.sizeOk(15_000_000))
    assertFalse(UpdateRules.sizeOk(-1))
    assertFalse(UpdateRules.sizeOk(0))
    assertFalse(UpdateRules.sizeOk(UpdateRules.MAX_BYTES + 1))
    assertEquals("0.2.513", UpdateRules.safeName("0.2.513"))
    assertEquals("_.._x", UpdateRules.safeName("/../x"))
    assertEquals("update", UpdateRules.safeName(""))
    assertEquals("update", UpdateRules.safeName(".."))
  }
}
