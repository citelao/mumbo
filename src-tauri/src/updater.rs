//! Auto-update from GitHub Releases (see `plugins.updater` in
//! `tauri.conf.json` and the release workflow, which publishes `latest.json`).
//!
//! Updates download in the background at startup and install when the app
//! quits, so nobody is pulled out of a call. Windows can't replace a running
//! .exe, and on macOS this keeps the bundle from changing under a running app.

use std::sync::Mutex;

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// The frontend's settings, under the store's `settings` key (see
/// `settings.ts`). Only the fields the updater needs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    auto_update: Option<bool>,
}

/// On unless the user turned it off. The setting can change mid-session from
/// the menu, so read it fresh each time.
fn auto_update_enabled(settings: Option<serde_json::Value>) -> bool {
    let Some(settings) = settings else {
        return true;
    };
    match serde_json::from_value::<Settings>(settings) {
        Ok(settings) => settings.auto_update.unwrap_or(true),
        Err(e) => {
            log::warn!("Ignoring unreadable update setting: {e}");
            true
        }
    }
}

fn auto_update_enabled_in_store(app: &AppHandle) -> bool {
    match app.store(crate::SETTINGS_FILE) {
        Ok(store) => auto_update_enabled(store.get("settings")),
        Err(e) => {
            log::warn!("Couldn't read update setting, assuming on: {e}");
            true
        }
    }
}

/// A verified-on-install update, downloaded and waiting for the app to quit.
#[derive(Default)]
struct PendingUpdate(Mutex<Option<(Update, Vec<u8>)>>);

/// Starts downloading any available update, unless auto-update is off.
pub fn spawn_download(app: &AppHandle) {
    app.manage(PendingUpdate::default());
    // Dev builds aren't installed bundles, so there's nothing to replace.
    if cfg!(debug_assertions) {
        return;
    }
    if !auto_update_enabled_in_store(app) {
        log::info!("Auto-update is off; not checking");
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
    if !auto_update_enabled_in_store(app) {
        log::info!("Auto-update was turned off; not installing {}", update.version);
        return;
    }
    log::info!("Installing update {}", update.version);
    if let Err(e) = update.install(bytes) {
        log::error!("Couldn't install update {}: {e}", update.version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn on_when_nothing_is_saved() {
        assert!(auto_update_enabled(None));
        assert!(auto_update_enabled(Some(json!({ "theme": "dark" }))));
    }

    #[test]
    fn follows_the_saved_choice() {
        assert!(auto_update_enabled(Some(json!({ "autoUpdate": true }))));
        assert!(!auto_update_enabled(Some(json!({ "autoUpdate": false }))));
    }

    #[test]
    fn on_when_the_setting_is_unreadable() {
        assert!(auto_update_enabled(Some(json!({ "autoUpdate": "nope" }))));
    }
}
