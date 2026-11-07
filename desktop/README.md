# Outhouse for Windows (and Android)

The Outhouse app for your PC. It shows your Outhouse server in its own window and adds what a browser tab can't:

- **Push to talk that works in game.** Your key or mouse button (middle, back, forward) works while WoW or anything else is focused. The key still reaches the game as normal.
- **Notifications** when someone @mentions you, @everyone, one of your roles, or replies to you while Outhouse isn't in front. The taskbar icon flashes too.
- **Tray icon:** closing the window keeps Outhouse running (so push to talk keeps working). Right-click the tray icon for *Change server…*, *Start with Windows* and *Quit*.
- Links to other websites open in your normal browser.

## Get the installer
Every push to GitHub builds it: open the repo's **Actions** tab → the latest
**CI** run → **Artifacts** → `Outhouse-Windows-installer`. Or build it yourself:

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
- First launch asks for the server: `oh.hartspace.cc` is filled in already.
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
2. **The Android app (APK, experimental).** Every push to GitHub builds
   `Outhouse-Android-APK` (Actions → the latest CI run → Artifacts). Unzip it,
   copy the `.apk` to your phone and open it (Android asks once to allow
   installing apps from that source). It's the same app as on Windows, without
   the tray or in-game push to talk. Voice in this version depends on
   Android's built-in web view allowing the microphone; if voice doesn't work
   in it, use option 1.
