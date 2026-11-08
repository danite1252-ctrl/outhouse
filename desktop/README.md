# Outhouse for Windows (and Android)

The Outhouse app for your PC. It shows your Outhouse server in its own window and adds what a browser tab can't:

- **Push to talk that works in game.** Your key or mouse button (middle, back, forward) works while WoW or anything else is focused. The key still reaches the game as normal.
- **Notifications** when someone @mentions you, @everyone, one of your roles, or replies to you while Outhouse isn't in front. The taskbar icon flashes too.
- **Tray icon:** closing the window keeps Outhouse running (so push to talk keeps working). Right-click the tray icon for *Start with Windows* and *Quit*.
- Links to other websites open in your normal browser.

## Get the installer
The easiest way: open https://oh.hartspace.cc/ and use the **Download** section
(the files come straight from your own server's `/updates/` folder). Every push
to GitHub builds the apps and publishes them under the repo's **Releases** →
**Outhouse (latest build)**; `deploy/fetch-updates.py` copies them to the
server within 10 minutes. Or build it yourself:

## Build the installer yourself
You already have Rust and the C++ build tools from building the server.

1. Install the Tauri build tool (one time, takes a few minutes):
   ```powershell
   cargo install tauri-cli --version "^2" --locked
   ```
2. Build:
   ```powershell
   cd "C:\dev\Outhouse Discord\desktop\src-tauri"
   cargo tauri build
   ```
3. The installer is at `desktop\src-tauri\target\release\bundle\nsis\Outhouse_0.1.0_x64-setup.exe`.
   Run it, or send it to your guild and family. It installs just for the current Windows user, no admin needed.

For a quick try without installing: `cargo tauri dev` in the same folder.

## Using it
- The apps always open **https://oh.hartspace.cc** (there's no server picker). If the server can't be reached you get a "Can't reach Outhouse" screen that keeps retrying.
- Set your push-to-talk key in **User settings → Voice & audio** as usual. The app watches that one key only while you're in voice with push to talk on.
- Windows will ask once to let Outhouse use the microphone.

## Notes
- If a game runs **as administrator**, Windows won't let normal apps see keys pressed in it. Run Outhouse as administrator too in that case. WoW doesn't need this.
- Windows SmartScreen may warn about the installer because it isn't code-signed. Click *More info → Run anyway*.
- Settings are kept in `%APPDATA%\cc.hartspace.outhouse\settings.json`.

## Android
There are two ways to have Outhouse on an Android phone:

1. **Install the web app (recommended).** Open your Outhouse address in Chrome,
   tap ⋮ → **Add to Home screen** (or **Install app**). It gets its own icon and
   full screen, and voice works because it's Chrome underneath. Push to talk
   shows a big **Hold to talk** button in the voice panel.
2. **The Android app (APK, experimental).** On your phone, sign in to GitHub in
   the browser, open the repo's **Releases** → **Outhouse (latest build)** and
   tap `Outhouse-Android.apk` (Android asks once to allow installing apps from
   your browser). Updates install over the top. If Android says **"App not
   installed"**, your copy is from the first test builds (version 0.2.14 or
   older, signed with a throwaway key): uninstall Outhouse once, then install
   the new APK; from then on updates install normally. It runs on 64-bit and
   32-bit ARM phones. It's the same app as on Windows, without
   the tray or in-game push to talk. Voice in this version depends on
   Android's built-in web view allowing the microphone; if voice doesn't work
   in it, use option 1.

**Notifications on the phone:** the web app (option 1) gets proper push
notifications, even when it's closed: Settings → Notifications → **Turn on
push notifications**. The APK only notifies you while it's open or recently
in the background, because Android's built-in web view can't receive Web Push
(that would need Google's Firebase inside the app). If you want both, use the
web app for notifications.

## Moving to a different domain
The server address is in **one place**: `DEFAULT_SERVER` at the top of
`desktop/src-tauri/src/lib.rs`. A build can also override it with the
`OUTHOUSE_SERVER_URL` environment variable (CI sets it in
`.github/workflows/ci.yml`, in the top-level `env:`).
Everything else follows from it: the page the window opens, which site may
use the app bridge (push to talk, notifications), which links stay in the
window, and where automatic updates come from (`<server>/updates/latest.json`).

To move:
1. Point the new domain at the server (see `deploy/README.md`).
2. Change `OUTHOUSE_SERVER_URL` at the top of the workflow (and `DEFAULT_SERVER` for local builds), push,
   and let CI build new apps.
3. **Important:** existing Windows installs check the *old* address for
   updates, so keep the old domain working until everyone has updated (or ask
   people to reinstall from the new site). Android installs are updated by
   hand, so they need the new APK either way.

Older versions let you pick a server and saved it in `settings.json`; that
choice is now ignored.

## Where the download links live
The landing page (`web/landing.html`) gets its Windows and Android download
links from the `DOWNLOADS` setting at the top of `web/landing.js`
(`/updates/Outhouse-Windows-Setup.exe` and `/updates/Outhouse-Android.apk`,
with versions from `/updates/latest.json` and `/updates/android.json`). If a
file isn't there yet, its button says *Coming soon*. Change `DOWNLOADS` if
the files ever move.
