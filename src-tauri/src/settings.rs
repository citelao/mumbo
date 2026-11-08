//! Settings saved across launches. One store file holds both what the UI owns
//! (`settings.ts`, under `settings`) and what Rust owns (under `audio`); the
//! store plugin keeps one copy per file, so each side sees the other's writes.

use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Wry};
use tauri_plugin_store::{Store, StoreExt};

use crate::audio::SavedPreferences;

const FILE: &str = "settings.json";
const UI_KEY: &str = "settings";
const AUDIO_KEY: &str = "audio";

#[derive(Clone)]
pub struct Settings {
    store: Arc<Store<Wry>>,
}

impl Settings {
    pub fn load(app: &AppHandle) -> tauri_plugin_store::Result<Self> {
        Ok(Self { store: app.store(FILE)? })
    }

    /// Read fresh each time, since the UI can change these at any moment.
    pub fn ui(&self) -> UiSettings {
        UiSettings::from_stored(self.store.get(UI_KEY))
    }

    pub fn audio(&self) -> SavedPreferences {
        SavedPreferences::from_stored(self.store.get(AUDIO_KEY))
    }

    pub fn save_audio(&self, saved: &SavedPreferences) -> tauri_plugin_store::Result<()> {
        self.store.set(AUDIO_KEY, serde_json::json!(saved));
        self.store.save()
    }
}

/// The UI's settings (see `SettingsSchema` in `settings.ts`); only the fields
/// Rust reads. The UI writes them, so Rust never does.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    auto_update: Option<bool>,
}

impl UiSettings {
    /// Anything missing or unreadable counts as nothing saved, so a bad
    /// settings file never stops startup.
    fn from_stored(value: Option<serde_json::Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        serde_json::from_value(value).unwrap_or_else(|e| {
            log::warn!("Ignoring unreadable UI settings: {e}");
            Self::default()
        })
    }

    /// On unless the user turned it off.
    pub fn auto_update(&self) -> bool {
        self.auto_update.unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_update_is_on_when_nothing_is_saved() {
        assert!(UiSettings::from_stored(None).auto_update());
        assert!(UiSettings::from_stored(Some(json!({ "theme": "dark" }))).auto_update());
    }

    #[test]
    fn auto_update_follows_the_saved_choice() {
        assert!(UiSettings::from_stored(Some(json!({ "autoUpdate": true }))).auto_update());
        assert!(!UiSettings::from_stored(Some(json!({ "autoUpdate": false }))).auto_update());
    }

    #[test]
    fn auto_update_is_on_when_the_settings_are_unreadable() {
        assert!(UiSettings::from_stored(Some(json!({ "autoUpdate": "nope" }))).auto_update());
    }
}
