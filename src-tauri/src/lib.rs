
use std::sync::Arc;
use std::io::Error;

use mumble_protocol as proto;
use mumble_protocol::client::{Connection, ConnectionOptions, SelfMuteStatus};
use tauri::{Emitter, Listener, Manager};
use tauri::ipc::Channel;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tauri_plugin_store::StoreExt;
use tokio::sync::{broadcast, watch};

pub mod audio;
mod universal_mute;

use universal_mute::UniversalMuter;

struct AppState {
    client: std::sync::Mutex<Option<proto::client::MumbleClient>>,
    audio_module: audio::AudioModule,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
// `device_id` is the frontend's IPC arg name. `None` means the system default.
#[tauri::command]
fn set_audio_input_device(state: tauri::State<'_, AppState>, device_id: Option<String>) -> Result<(), String> {
    state.audio_module.select_input_device(device_id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_audio_output_device(state: tauri::State<'_, AppState>, device_id: Option<String>) -> Result<(), String> {
    state.audio_module.select_output_device(device_id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_noise_suppression(state: tauri::State<'_, AppState>, enabled: bool) {
    state.audio_module.set_noise_suppression(enabled);
}

#[tauri::command]
async fn get_server_stats(server: String, port: u16) -> Result<proto::client::ServerStats, String> {
    proto::client::get_server_stats(server, Some(port))
        .await
        .inspect_err(|e| log::error!("{e}"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    server: String,
    port: u16,
    password: String,
    name: String,
    channel: Channel<proto::ServerToClientMessage>,
) -> Result<(), String> {
    connect_internal(app, state, server, port, password, name, channel)
        .await
        .inspect_err(|e| log::error!("{e}"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, AppState>) -> Result<(), String> {
    // `take` empties the slot under a single lock, so no other command can
    // observe the connection between "found it" and "removed it".
    if let Some(client) = state.client.lock().unwrap().take() {
        log::info!("Disconnecting from server");
        client.disconnect();
    } else {
        log::info!("No active connection");
    }
    Ok(())
}

#[tauri::command]
fn get_users(state: tauri::State<'_, AppState>) -> Vec<proto::state::User> {
    state
        .client
        .lock()
        .unwrap()
        .as_ref()
        .map(|client| client.state().lock().unwrap().users())
        .unwrap_or_default()
}

#[tauri::command]
fn get_channels(state: tauri::State<'_, AppState>) -> Vec<proto::state::Channel> {
    state
        .client
        .lock()
        .unwrap()
        .as_ref()
        .map(|client| client.state().lock().unwrap().channels())
        .unwrap_or_default()
}

#[tauri::command]
fn get_chat_messages(state: tauri::State<'_, AppState>) -> Vec<proto::state::TextMessage> {
    state
        .client
        .lock()
        .unwrap()
        .as_ref()
        .map(|client| client.state().lock().unwrap().chat_messages().to_vec())
        .unwrap_or_default()
}

#[tauri::command]
fn send_channel_message(state: tauri::State<'_, AppState>, message: String) -> Result<(), String> {
    if let Some(client) = state.client.lock().unwrap().as_ref() {
        client.send_channel_message(message, None).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

#[tauri::command]
fn send_user_message(state: tauri::State<'_, AppState>, message: String, session: Vec<u32>) -> Result<(), String> {
    if let Some(client) = state.client.lock().unwrap().as_ref() {
        client.send_user_message(message, session).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

#[tauri::command]
fn set_self_mute(state: tauri::State<'_, AppState>, muted: bool) -> Result<(), String> {
    if let Some(client) = state.client.lock().unwrap().as_ref() {
        client.set_self_mute(muted).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

#[tauri::command]
fn set_channel(state: tauri::State<'_, AppState>, channel_id: u32) -> Result<(), String> {
    if let Some(client) = state.client.lock().unwrap().as_ref() {
        client.set_channel(channel_id).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

/// Forwards debug messages until the connection closes. A consumer that falls
/// behind skips messages instead of stopping the stream.
async fn forward_debug<T: Clone>(mut debug: broadcast::Receiver<T>, mut forward: impl FnMut(T)) {
    loop {
        match debug.recv().await {
            Ok(message) => forward(message),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                log::warn!("Debug stream skipped {skipped} messages")
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Calls `forward` with the latest value each time `changes` is updated, until
/// the sender is dropped. Like any `watch` consumer, it sees only the newest
/// value if several updates land before it runs.
async fn forward_watch<T: Clone>(mut changes: watch::Receiver<T>, mut forward: impl FnMut(T)) {
    while changes.changed().await.is_ok() {
        forward(changes.borrow_and_update().clone());
    }
}

/// A `watch` channel's changes with the value's type erased, so receivers of
/// different types can share a list.
struct Changes(Box<dyn ChangeSource + Send>);

trait ChangeSource {
    /// Resolves to `false` once the sender is gone.
    fn next_change<'a>(&'a mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
}

impl<T: Send + Sync> ChangeSource for watch::Receiver<T> {
    fn next_change<'a>(&'a mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { self.changed().await.is_ok() })
    }
}

trait MapChanges {
    fn map_changes(self) -> Changes;
}

impl<T: Send + Sync + 'static> MapChanges for watch::Receiver<T> {
    fn map_changes(self) -> Changes {
        Changes(Box::new(self))
    }
}

/// Calls `forward` each time `changes` changes, until the sender is dropped.
/// The value itself is not passed on: use this when `forward` re-reads state.
async fn forward_changes(mut changes: Changes, mut forward: impl FnMut()) {
    while changes.0.next_change().await {
        forward();
    }
}

const AUDIO_INPUT_DEVICES_EVENT: &str = "audio-input-devices-changed";
const AUDIO_OUTPUT_DEVICES_EVENT: &str = "audio-output-devices-changed";
/// The device the user prefers (an `AudioDevice`), or null to follow the system default.
const AUDIO_INPUT_SELECTED_EVENT: &str = "audio-input-device-selected";
const AUDIO_OUTPUT_SELECTED_EVENT: &str = "audio-output-device-selected";
/// Whether mic noise suppression is on (a bool).
const AUDIO_NOISE_SUPPRESSION_EVENT: &str = "audio-noise-suppression-changed";
/// Sent by the UI once its device listeners are registered. Events emitted
/// earlier would be lost, so this is the cue to publish everything current.
const AUDIO_DEVICES_SUBSCRIBE_EVENT: &str = "audio-devices-subscribe";

const SETTINGS_FILE: &str = "settings.json";
/// Where the preferred audio devices are saved. The UI's own settings share the
/// file (under another key); the store plugin keeps one copy per file.
const AUDIO_SETTINGS_KEY: &str = "audio";

/// Saves the audio preferences each time one changes.
fn save_audio_preferences(store: Arc<tauri_plugin_store::Store<tauri::Wry>>, audio: &audio::AudioModule) {
    let input = audio.preferred_input_device();
    let output = audio.preferred_output_device();
    let noise_suppression = audio.noise_suppression();
    let changes = [
        input.clone().map_changes(),
        output.clone().map_changes(),
        noise_suppression.clone().map_changes(),
    ];
    for changes in changes {
        let (store, input, output, noise_suppression) = (store.clone(), input.clone(), output.clone(), noise_suppression.clone());
        tauri::async_runtime::spawn(forward_changes(changes, move || {
            let saved = audio::SavedPreferences::of(input.borrow().as_ref(), output.borrow().as_ref(), *noise_suppression.borrow());
            store.set(AUDIO_SETTINGS_KEY, serde_json::json!(saved));
            if let Err(e) = store.save() {
                log::warn!("Failed to save the preferred audio devices: {e}");
            }
        }));
    }
}

/// Emits `value` as `event`; the UI may simply not be listening yet.
fn emit_to_ui<T: Clone + serde::Serialize>(app: &tauri::AppHandle, event: &str, value: T) {
    if let Err(e) = app.emit(event, value) {
        log::warn!("Failed to emit {event}: {e}");
    }
}

/// Emits each change of `changes` as `event`, and returns a function that emits
/// the current value on demand.
fn publish<T>(app: &tauri::AppHandle, event: &'static str, changes: watch::Receiver<T>) -> Box<dyn Fn() + Send>
where
    T: Clone + serde::Serialize + Send + Sync + 'static,
{
    let current = changes.clone();
    let on_change = app.clone();
    tauri::async_runtime::spawn(forward_watch(changes, move |value| emit_to_ui(&on_change, event, value)));
    let on_demand = app.clone();
    Box::new(move || emit_to_ui(&on_demand, event, current.borrow().clone()))
}

/// Publishes the audio module's device lists and the user's selections to the
/// UI as events: whenever one changes, and again (all of them) whenever a
/// listener announces itself.
fn publish_audio_devices(app: &tauri::AppHandle, audio: &audio::AudioModule) {
    let republish = [
        publish(app, AUDIO_INPUT_DEVICES_EVENT, audio.input_devices()),
        publish(app, AUDIO_OUTPUT_DEVICES_EVENT, audio.output_devices()),
        publish(app, AUDIO_INPUT_SELECTED_EVENT, audio.preferred_input_device()),
        publish(app, AUDIO_OUTPUT_SELECTED_EVENT, audio.preferred_output_device()),
        publish(app, AUDIO_NOISE_SUPPRESSION_EVENT, audio.noise_suppression()),
    ];
    app.listen(AUDIO_DEVICES_SUBSCRIBE_EVENT, move |_| {
        for emit_current in &republish {
            emit_current();
        }
    });
}

async fn connect_internal(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    server: String,
    port: u16,
    password: String,
    name: String,
    channel: Channel<proto::ServerToClientMessage>,
) -> Result<(), Error> {
    if let Some(previous) = state.client.lock().unwrap().take() {
        log::info!("Disconnecting from existing connection");
        previous.disconnect();
    }

    let Connection {
        client,
        debug,
        audio,
        mut events,
    } = proto::client::connect(ConnectionOptions {
        server,
        port: Some(port),
        password: Some(password),
        username: Some(name),
    })
    .await
    .map_err(|e| Error::other(format!("Failed to connect: {}", e)))?;

    // Send debug events to the channel.
    tauri::async_runtime::spawn(forward_debug(debug, move |message| {
        let _ = channel.send(message);
    }));

    // Owned by the events task below, so the Windows "call" ends with the connection.
    let mute_client = client.clone();
    let mut universal_muter = match UniversalMuter::new(move |muted| {
        // Windows may report a press that matches what we already asked for
        // (e.g. echoing a confirmation), so only act on real changes.
        let already_requested = matches!(
            mute_client.self_mute_status(),
            SelfMuteStatus::Muting | SelfMuteStatus::Muted
        );
        if already_requested == muted {
            return;
        }
        if let Err(e) = mute_client.set_self_mute(muted) {
            log::warn!("Universal mute couldn't change mute: {e}");
        }
    }) {
        Ok(muter) => Some(muter),
        Err(e) => {
            log::warn!("Universal mute unavailable: {e}");
            None
        }
    };

    // Send state events to the app.
    let status_client = client.clone();
    tauri::async_runtime::spawn(async move {
        let mut shown_channel: Option<String> = None;
        while let Some(event) = events.recv().await {
            // TODO: a useful payload.
            let _ = app.emit("session-changed", event);

            // Audio stops the moment a mute is requested, but the system mute
            // button only follows once the server agrees.
            let confirmed_mute = match status_client.self_mute_status() {
                SelfMuteStatus::Muted => Some(true),
                SelfMuteStatus::Unmuted => Some(false),
                SelfMuteStatus::Muting | SelfMuteStatus::Unmuting => None,
            };
            if let (Some(muter), Some(muted)) = (universal_muter.as_mut(), confirmed_mute)
                && let Err(e) = muter.confirm(muted)
            {
                log::warn!("Universal mute couldn't update the button: {e}");
            }

            // Windows shows the call's contact name next to the mute button, so
            // keep it on the channel we're in.
            let channel_name = {
                let state = status_client.state();
                let state = state.lock().unwrap();
                state
                    .current_user()
                    .and_then(|user| state.channels.get(&user.channel_id))
                    .map(|channel| channel.name.clone())
            };
            if let (Some(muter), Some(name)) = (universal_muter.as_ref(), channel_name)
                && shown_channel.as_deref() != Some(name.as_str())
            {
                if let Err(e) = muter.set_contact_name(&name) {
                    log::warn!("Universal mute couldn't rename the call: {e}");
                }
                shown_channel = Some(name);
            }
        }
        // `recv` only returns `None` once the driver has exited, so the
        // connection is over, however it ended.
        // TODO: clear AppState and tell the UI. Careful: by now a newer
        // connection may be in AppState, so only clear it if it's still ours.
        log::info!("Connection ended");
    });

    let cloned_client = client.clone();
    let audio_task = state.audio_module
        .start(move |audio: &[u8], frame_counter: u64, is_terminator: bool| {
            if let Err(e) = cloned_client.send_audio(audio, frame_counter, is_terminator) {
                log::warn!("Failed to send mic audio: {e}");
            }
        }, mumbo_audio::speaker::Mixer::new(audio))
        .map_err(|e|
            Error::other(format!("Failed to start audio module: {}", e))
        )?;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = audio_task.await {
            // TODO: better reporting.
            log::error!("Audio ended with error: {e}");
        }
    });

    *state.client.lock().unwrap() = Some(client);

    log::info!("Connected to server");
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let store = app.store(SETTINGS_FILE)?;
            let saved = audio::SavedPreferences::from_stored(store.get(AUDIO_SETTINGS_KEY));
            let audio_module = audio::AudioModule::new(saved)?;
            publish_audio_devices(app.handle(), &audio_module);
            save_audio_preferences(store, &audio_module);
            app.manage(AppState {
                client: Default::default(),
                audio_module,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("mumbo_lib", log::LevelFilter::Debug)
                .level_for("mumble_protocol", log::LevelFilter::Debug)
                .with_colors(ColoredLevelConfig::default())
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            set_audio_input_device,
            set_audio_output_device,
            set_noise_suppression,
            get_server_stats,
            connect,
            get_users,
            get_channels,
            get_chat_messages,
            send_channel_message,
            send_user_message,
            set_self_mute,
            set_channel,
            disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn change_forwarding_runs_once_per_observed_change_for_any_value_type() {
        let (numbers, number_changes) = tokio::sync::watch::channel(0);
        let (words, word_changes) = tokio::sync::watch::channel(String::new());
        numbers.send(1).unwrap();
        // Two updates before the consumer runs count as one change.
        words.send("a".into()).unwrap();
        words.send("b".into()).unwrap();
        drop((numbers, words));

        let mut calls = 0;
        // Different value types sit in one list once their type is erased.
        for changes in [number_changes.map_changes(), word_changes.map_changes()] {
            forward_changes(changes, || calls += 1).await;
        }

        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn change_forwarding_ignores_the_starting_value_and_ends_when_the_sender_drops() {
        let (sender, receiver) = tokio::sync::watch::channel(7);
        drop(sender);

        let mut calls = 0;
        forward_changes(receiver.map_changes(), || calls += 1).await;

        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn watch_forwarding_sends_the_latest_value_and_ends_when_the_sender_drops() {
        let (sender, receiver) = tokio::sync::watch::channel(0);
        // Two updates before the consumer runs: only the newest is seen.
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        drop(sender);

        let mut forwarded = Vec::new();
        forward_watch(receiver, |value| forwarded.push(value)).await;

        assert_eq!(forwarded, [2]);
    }

    #[tokio::test]
    async fn watch_forwarding_ignores_the_value_it_started_with() {
        let (sender, receiver) = tokio::sync::watch::channel(7);
        drop(sender);

        let mut forwarded = Vec::new();
        forward_watch(receiver, |value| forwarded.push(value)).await;

        assert!(forwarded.is_empty());
    }

    #[tokio::test]
    async fn debug_forwarding_survives_lag_and_ends_when_the_connection_closes() {
        let (sender, receiver) = tokio::sync::broadcast::channel(2);
        // Five messages into a 2-slot channel: the reader lags and skips 0..=2.
        for message in 0..5 {
            sender.send(message).unwrap();
        }
        drop(sender);

        let mut forwarded = Vec::new();
        forward_debug(receiver, |message| forwarded.push(message)).await;

        assert_eq!(forwarded, [3, 4]);
    }
}
