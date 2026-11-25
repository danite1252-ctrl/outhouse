// Adds android/ (the Kotlin side) to the generated Android project:
// `tauri android build` includes it as the Gradle module :tauri-plugin-apk-updater.
fn main() {
    tauri_plugin::Builder::new(&[]).android_path("android").build();
}
