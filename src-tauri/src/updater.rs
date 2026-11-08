//! Auto-update from GitHub Releases (see `plugins.updater` in
//! `tauri.conf.json` and the release workflow, which publishes `latest.json`).
//!
//! Updates download in the background at startup and install when the app
//! quits, so nobody is pulled out of a call. Windows can't replace a running
//! .exe, and on macOS this keeps the bundle from changing under a running app.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// A verified-on-install update, downloaded and waiting for the app to quit.
#[derive(Default)]
struct PendingUpdate(Mutex<Option<(Update, Vec<u8>)>>);

/// Starts downloading any available update.
pub fn spawn_download(app: &AppHandle) {
    app.manage(PendingUpdate::default());
    // Dev builds aren't installed bundles, so there's nothing to replace.
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match download(&app).await {
            Ok(Some(pending)) => *app.state::<PendingUpdate>().0.lock().unwrap() = Some(pending),
            Ok(None) => log::info!("No update available"),
            // Offline or GitHub hiccups shouldn't stop the app from working.
            Err(e) => log::warn!("Update check failed: {e}"),
        }
    });
}

async fn download(app: &AppHandle) -> tauri_plugin_updater::Result<Option<(Update, Vec<u8>)>> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(None);
    };
    log::info!("Downloading update {} -> {}", update.current_version, update.version);
    let bytes = update.download(|_, _| {}, || {}).await?;
    log::info!("Update downloaded; it installs when the app quits");
    // The user is quitting, not restarting, so don't relaunch on Windows.
    Ok(Some((update.restart_after_install(false), bytes)))
}

/// Installs the downloaded update, if any. Call as the app exits; on Windows
/// this launches the installer and ends the process.
pub fn install_pending(app: &AppHandle) {
    let Some(state) = app.try_state::<PendingUpdate>() else {
        return;
    };
    let Some((update, bytes)) = state.0.lock().unwrap().take() else {
        return;
    };
    log::info!("Installing update {}", update.version);
    if let Err(e) = update.install(bytes) {
        log::error!("Couldn't install update {}: {e}", update.version);
    }
}
