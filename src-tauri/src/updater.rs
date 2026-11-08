//! Background auto-update from GitHub Releases (see `plugins.updater` in
//! `tauri.conf.json` and the release workflow, which publishes `latest.json`).

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// Checks for, downloads, and installs an update without blocking startup.
///
/// On macOS the new bundle replaces the old one in place and takes effect on
/// next launch. On Windows the NSIS installer runs and closes the app.
pub fn spawn_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = check_and_install(&app).await {
            // Offline or GitHub hiccups shouldn't stop the app from working.
            log::warn!("Update check failed: {e}");
        }
    });
}

async fn check_and_install(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        log::info!("No update available");
        return Ok(());
    };
    log::info!("Updating {} -> {}", update.current_version, update.version);
    update.download_and_install(|_, _| {}, || log::info!("Update downloaded")).await?;
    log::info!("Update installed; it applies on next launch");
    Ok(())
}
