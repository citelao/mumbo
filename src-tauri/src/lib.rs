use std::collections::HashMap;
use std::io::Error;
use std::sync::Arc;

use mumbo_sdk::state::MumbleEvent;
use mumbo_sdk as proto;
use mumbo_sdk::client::{Connection, ConnectionOptions, SelfMuteStatus};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tauri_plugin_store::StoreExt;
use tokio::sync::{broadcast, watch};

pub mod audio;
mod universal_mute;

use universal_mute::UniversalMuter;

struct MumbleConnection {
    /// Tells this connection apart from later ones, which may replace it in
    /// `AppState` before it finishes ending.
    id: u64,

    client: proto::client::MumbleClient,

    /// Volume adjustments keyed by user session ID.
    ///
    /// TODO: store these one time in the Mixer?
    /// TODO: key by an immutable user ID, for durable adjustments between connections.
    volume_adjustments: watch::Sender<HashMap<u32, f32>>
}

/// Global, the next connection ID to assign to a new `MumbleConnection`, to tell them apart.
static NEXT_CONNECTION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tells the UI the connection it was showing has ended.
#[derive(Clone, serde::Serialize)]
struct Disconnected {
    reason: DisconnectReason,
}

/// Why a connection ended, for the UI to explain however it likes.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DisconnectReason {
    /// The server removed us: an admin kicked or banned us, or we connected
    /// again from elsewhere with the same name.
    Removed {
        reason: Option<String>,
        /// The admin who did it, if anyone.
        by: Option<String>,
        banned: bool,
    },
    /// The server ended the connection cleanly without saying why.
    ServerClosed,
    /// The connection broke, e.g. because the server went away.
    ConnectionLost { detail: String },
}

/// Passes each of a connection's events to `on_event` until the connection
/// ends, then says why it ended. `removals` is a subscription to the
/// connection's debug feed: the only place the server's notice that it
/// removed us, with its reason, shows up.
async fn follow_until_disconnected(
    mut events: tokio::sync::mpsc::UnboundedReceiver<MumbleEvent>,
    mut removals: broadcast::Receiver<proto::ServerToClientMessage>,
    client: &proto::client::MumbleClient,
    mut on_event: impl FnMut(MumbleEvent),
) -> DisconnectReason {
    let my_session = client.state().lock().unwrap().current_user().map(|user| user.session);
    let mut sdk_reason = None;
    // `recv` only returns `None` once the driver has exited, so the
    // connection is over, however it ended.
    while let Some(event) = events.recv().await {
        if let MumbleEvent::Disconnected { reason } = &event {
            sdk_reason = reason.clone();
        }
        on_event(event);
    }

    // The server sends its notice before closing the connection, so by now
    // it's queued.
    let mut removal = None;
    loop {
        match removals.try_recv() {
            Ok(message) => {
                if let Some(my_session) = my_session {
                    let state = client.state();
                    let state = state.lock().unwrap();
                    removal = removal_of(&message, my_session, &state).or(removal);
                }
            }
            // Audio shares the feed and can overflow it, but lagging only
            // drops the oldest messages; the notice is among the newest.
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    disconnect_reason(removal, sdk_reason)
}

/// The removal `message` announces, if it's the server removing `my_session`.
fn removal_of(
    message: &proto::ServerToClientMessage,
    my_session: u32,
    state: &proto::state::MumbleState,
) -> Option<DisconnectReason> {
    let proto::ServerToClientMessage::UserRemove(remove) = message else {
        return None;
    };
    if remove.session != my_session {
        return None;
    }
    Some(DisconnectReason::Removed {
        // Murmur sends an empty reason when there isn't one.
        reason: remove.reason.clone().filter(|reason| !reason.is_empty()),
        by: remove
            .actor
            .and_then(|actor| state.users.get(&actor))
            .map(|admin| admin.name.clone()),
        banned: remove.ban.unwrap_or(false),
    })
}

/// Why the connection ended: the server's notice that it removed us if one
/// came, otherwise how the SDK says the connection ended.
fn disconnect_reason(removal: Option<DisconnectReason>, sdk_reason: Option<String>) -> DisconnectReason {
    match (removal, sdk_reason) {
        (Some(removal), _) => removal,
        (None, None) => DisconnectReason::ServerClosed,
        (None, Some(detail)) => DisconnectReason::ConnectionLost { detail },
    }
}

struct AppState {
    connection: std::sync::Mutex<Option<MumbleConnection>>,
    audio_module: audio::AudioModule,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
// `device_id` is the frontend's IPC arg name. `None` means the system default.
#[tauri::command]
fn set_audio_input_device(
    state: tauri::State<'_, AppState>,
    device_id: Option<String>,
) -> Result<(), String> {
    state
        .audio_module
        .select_input_device(device_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_audio_output_device(
    state: tauri::State<'_, AppState>,
    device_id: Option<String>,
) -> Result<(), String> {
    state
        .audio_module
        .select_output_device(device_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_noise_suppression(state: tauri::State<'_, AppState>, enabled: bool) {
    state.audio_module.set_noise_suppression(enabled);
}

#[tauri::command]
async fn get_server_stats(server: String, port: u16) -> Result<proto::client::ServerStats, String> {
    // Stats are a nice-to-have UDP ping, so failure is a warning. On Windows an
    // unreachable host (e.g. no local server running) surfaces as a bare
    // "connection forcibly closed" (os error 10054), so name the target.
    proto::client::get_server_stats(server.clone(), Some(port))
        .await
        .inspect_err(|e| log::warn!("Couldn't fetch stats from {server}:{port}: {e}"))
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
    let target = format!("{server}:{port}");
    connect_internal(app, state, server, port, password, name, channel)
        .await
        .inspect_err(|e| log::error!("Couldn't connect to {target}: {e}"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, AppState>) -> Result<(), String> {
    // `take` empties the slot under a single lock, so no other command can
    // observe the connection between "found it" and "removed it".
    if let Some(connection) = state.connection.lock().unwrap().take() {
        log::info!("Disconnecting from server");
        connection.client.disconnect();
    } else {
        log::info!("No active connection");
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct UiUser {
    #[serde(flatten)]
    user: proto::state::User,

    volume_adjustment_db: Option<f32>,
}

#[tauri::command]
fn get_users(state: tauri::State<'_, AppState>) -> Vec<UiUser> {
    state
        .connection
        .lock()
        .unwrap()
        .as_ref()
        .map(|conn| {
            let binding = conn.client.state();
            let state = binding.lock().unwrap();
            let volume_adjustments = conn.volume_adjustments.borrow();
            state.users().iter().map(|user| UiUser {
                user: user.clone(),
                volume_adjustment_db: volume_adjustments.get(&user.session).copied(),
            }).collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[tauri::command]
fn get_channels(state: tauri::State<'_, AppState>) -> Vec<proto::state::Channel> {
    state
        .connection
        .lock()
        .unwrap()
        .as_ref()
        .map(|conn| conn.client.state().lock().unwrap().channels())
        .unwrap_or_default()
}

#[tauri::command]
fn get_chat_messages(state: tauri::State<'_, AppState>) -> Vec<proto::state::TextMessage> {
    state
        .connection
        .lock()
        .unwrap()
        .as_ref()
        .map(|conn| conn.client.state().lock().unwrap().chat_messages().to_vec())
        .unwrap_or_default()
}

#[tauri::command]
fn send_channel_message(state: tauri::State<'_, AppState>, message: String) -> Result<(), String> {
    let client = state.connection.lock().unwrap();
    let client = &client.as_ref().ok_or("No active connection")?.client;
    let limits = mumble_messages::MessageLimits::from_state(&client.state().lock().unwrap());
    let message = mumble_messages::prepare_text_message(&limits, &message).map_err(|e| e.to_string())?;
    client
        .send_channel_message(message, None)
        .map_err(|e| e.to_string())
}

/// Sends `image`, a data URL, to the current channel as a message of its own.
///
/// TODO: a `send_user_image` for DMs, once there's UI to write them.
///
/// Async, and re-encodes on a blocking-task thread: that takes long enough to
/// visibly freeze the UI if done on the main thread, where Tauri runs non-async
/// commands.
#[tauri::command]
async fn send_channel_image(state: tauri::State<'_, AppState>, image: String) -> Result<(), String> {
    let limits = {
        let client = state.connection.lock().unwrap();
        let client = &client.as_ref().ok_or("No active connection")?.client;
        mumble_messages::MessageLimits::from_state(&client.state().lock().unwrap())
    };
    let message = tauri::async_runtime::spawn_blocking(move || {
        mumble_messages::prepare_image_message(&limits, &image).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    let client = state.connection.lock().unwrap();
    let client = &client.as_ref().ok_or("No active connection")?.client;
    client
        .send_channel_message(message, None)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn send_user_message(state: tauri::State<'_, AppState>, message: String, session: Vec<u32>) -> Result<(), String> {
    let client = state.connection.lock().unwrap();
    let client = &client.as_ref().ok_or("No active connection")?.client;
    let limits = mumble_messages::MessageLimits::from_state(&client.state().lock().unwrap());
    let message = mumble_messages::prepare_text_message(&limits, &message).map_err(|e| e.to_string())?;
    client
        .send_user_message(message, session)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_self_mute(state: tauri::State<'_, AppState>, muted: bool) -> Result<(), String> {
    if let Some(connection) = state.connection.lock().unwrap().as_ref() {
        connection.client.set_self_mute(muted).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

#[tauri::command]
fn set_channel(state: tauri::State<'_, AppState>, channel_id: u32) -> Result<(), String> {
    if let Some(connection) = state.connection.lock().unwrap().as_ref() {
        connection.client.set_channel(channel_id).map_err(|e| e.to_string())
    } else {
        Err("No active connection".into())
    }
}

#[tauri::command]
fn set_volume_adjustment(app: AppHandle, state: tauri::State<'_, AppState>, session: u32, volume_db: f32) -> Result<(), String> {
    if let Some(connection) = state.connection.lock().unwrap().as_ref() {
        log::info!("Setting volume adjustment for session {} to {} dB", session, volume_db);
        connection.volume_adjustments.send_modify(|adjustments| {
            adjustments.insert(session, volume_db);
        });

        // Manually emit a session-changed event to notify listeners of the volume adjustment change.
        let _ = app.emit("session-changed", MumbleEvent::UsersChanged);
    } else {
        return Err("No active connection".into());
    }
    Ok(())
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
    fn next_change<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
}

impl<T: Send + Sync> ChangeSource for watch::Receiver<T> {
    fn next_change<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
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
fn save_audio_preferences(
    store: Arc<tauri_plugin_store::Store<tauri::Wry>>,
    audio: &audio::AudioModule,
) {
    let input = audio.preferred_input_device();
    let output = audio.preferred_output_device();
    let noise_suppression = audio.noise_suppression();
    let changes = [
        input.clone().map_changes(),
        output.clone().map_changes(),
        noise_suppression.clone().map_changes(),
    ];
    for changes in changes {
        let (store, input, output, noise_suppression) = (
            store.clone(),
            input.clone(),
            output.clone(),
            noise_suppression.clone(),
        );
        tauri::async_runtime::spawn(forward_changes(changes, move || {
            let saved = audio::SavedPreferences::of(
                input.borrow().as_ref(),
                output.borrow().as_ref(),
                *noise_suppression.borrow(),
            );
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
fn publish<T>(
    app: &tauri::AppHandle,
    event: &'static str,
    changes: watch::Receiver<T>,
) -> Box<dyn Fn() + Send>
where
    T: Clone + serde::Serialize + Send + Sync + 'static,
{
    let current = changes.clone();
    let on_change = app.clone();
    tauri::async_runtime::spawn(forward_watch(changes, move |value| {
        emit_to_ui(&on_change, event, value)
    }));
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
        publish(
            app,
            AUDIO_INPUT_SELECTED_EVENT,
            audio.preferred_input_device(),
        ),
        publish(
            app,
            AUDIO_OUTPUT_SELECTED_EVENT,
            audio.preferred_output_device(),
        ),
        publish(
            app,
            AUDIO_NOISE_SUPPRESSION_EVENT,
            audio.noise_suppression(),
        ),
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
    if let Some(previous) = state.connection.lock().unwrap().take() {
        log::info!("Disconnecting from existing connection");
        previous.client.disconnect();
    }

    let Connection {
        client,
        debug,
        audio,
        events,
    } = proto::client::connect(ConnectionOptions {
        server,
        port: Some(port),
        password: Some(password),
        username: Some(name),
    })
    .await
    .map_err(|e| Error::other(format!("Failed to connect: {}", describe_connect_error(&e))))?;

    // Subscribed now, so it sees everything from here on; see `follow_until_disconnected`.
    let removals = debug.resubscribe();

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

    let (volumes_tx, volumes_rx) = tokio::sync::watch::channel(HashMap::<u32, f32>::new());

    let connection_id = NEXT_CONNECTION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Send state events to the app.
    let status_client = client.clone();
    let event_volumes = volumes_tx.clone();
    tauri::async_runtime::spawn(async move {
        let mut shown_channel: Option<String> = None;
        let reason = follow_until_disconnected(events, removals, &status_client, |event| {
            if matches!(event, MumbleEvent::UsersChanged) {
                let present = status_client.state().lock().unwrap().users();
                forget_departed_users(&event_volumes, present.iter().map(|user| user.session));
            }

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
        })
        .await;
        log::info!("Connection ended: {reason:?}");
        // Only if it's still ours: if the user disconnected or connected
        // elsewhere, it's already gone and the UI already moved on.
        let ours = app
            .state::<AppState>()
            .connection
            .lock()
            .unwrap()
            .take_if(|connection| connection.id == connection_id);
        if ours.is_some() {
            let _ = app.emit("disconnected", Disconnected { reason });
        }
    });

    let cloned_client = client.clone();
    let audio_task = state
        .audio_module
        .start(
            move |audio: &[u8], frame_counter: u64, is_terminator: bool| {
                if let Err(e) = cloned_client.send_audio(audio, frame_counter, is_terminator) {
                    log::warn!("Failed to send mic audio: {e}");
                }
            },
            mumbo_audio::speaker::Mixer::new(audio, volumes_rx),
        )
        .map_err(|e| Error::other(format!("Failed to start audio module: {}", e)))?;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = audio_task.await {
            // TODO: better reporting.
            log::error!("Audio ended with error: {e}");
        }
    });

    *state.connection.lock().unwrap() = Some(MumbleConnection {
        id: connection_id,
        client,
        volume_adjustments: volumes_tx
    });

    log::info!("Connected to server");
    Ok(())
}

/// Drops the volume adjustments of sessions no longer on the server. The
/// server reuses session IDs, so a newcomer would otherwise inherit a
/// departed user's volume.
fn forget_departed_users(
    volumes: &watch::Sender<HashMap<u32, f32>>,
    present: impl IntoIterator<Item = u32>,
) {
    let present: std::collections::HashSet<u32> = present.into_iter().collect();
    volumes.send_if_modified(|volumes| {
        let before = volumes.len();
        volumes.retain(|session, _| present.contains(session));
        volumes.len() != before
    });
}

/// Explains a failed connect in terms the user can act on, keeping the
/// underlying error for debugging.
fn describe_connect_error(error: &proto::client::MumbleConnectionError) -> String {
    use proto::client::MumbleConnectionError;
    use std::io::ErrorKind;
    match error {
        // Murmur hangs up on an address it has autobanned, before even
        // finishing TLS; a server that's shutting down looks the same.
        MumbleConnectionError::IoError(e)
            if matches!(e.kind(), ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset) =>
        {
            format!(
                "The server hung up before the connection was set up. It may be restarting, \
                 or blocking this address for a few minutes after too many connection \
                 attempts. ({error})"
            )
        }
        _ => error.to_string(),
    }
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
                connection: Default::default(),
                audio_module,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("mumbo_lib", log::LevelFilter::Debug)
                .level_for("mumbo_sdk", log::LevelFilter::Debug)
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
            send_channel_image,
            send_user_message,
            set_self_mute,
            set_channel,
            set_volume_adjustment,
            disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn departed_users_lose_their_volume_adjustment() {
        let (volumes, _volumes_rx) = watch::channel(HashMap::from([(3, -6.0), (5, -12.0)]));

        forget_departed_users(&volumes, [3, 4]);

        assert_eq!(*volumes.borrow(), HashMap::from([(3, -6.0)]));
    }

    #[test]
    fn volume_adjustments_are_untouched_when_nobody_left() {
        let (volumes, mut volumes_rx) = watch::channel(HashMap::from([(3, -6.0)]));
        volumes_rx.mark_unchanged();

        forget_departed_users(&volumes, [3, 4]);

        // Users change constantly (e.g. whenever someone talks), so only a real
        // departure should wake the mixer.
        assert!(!volumes_rx.has_changed().unwrap());
    }

    /// Everyone lands in the dev server's root channel, so tests running at
    /// once would kick each other. Tests using it take turns.
    static DEV_SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Needs the dev server from dev/mumble-server (`npm run mumble:up`), on
    /// localhost:64738 with password "test". Run with `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the dev Mumble server (dev/mumble-server) on localhost:64738"]
    async fn connecting_again_under_our_name_removes_us_and_says_why() {
        let _turn = DEV_SERVER.lock().await;
        let connect = || {
            proto::client::connect(ConnectionOptions {
                server: "localhost".into(),
                port: Some(64738),
                password: Some("test".into()),
                username: Some(format!("mumbo-e2e-removed-{}", std::process::id())),
            })
        };
        let Connection { client, debug, audio, events } = connect().await.expect("connecting the first time");
        let removals = debug.resubscribe();
        let followed = tokio::spawn(async move {
            let _audio = audio; // Keep receiving audio, as the app does.
            follow_until_disconnected(events, removals, &client, |_| {}).await
        });

        // Murmur lets only one session use a name, so it removes the first.
        let second = connect().await.expect("connecting the second time");
        let reason = tokio::time::timeout(std::time::Duration::from_secs(5), followed)
            .await
            .expect("the first connection ended")
            .unwrap();
        second.client.disconnect();

        assert_eq!(
            reason,
            DisconnectReason::Removed {
                reason: Some("You connected to the server from another device".into()),
                by: None,
                banned: false,
            }
        );
    }

    /// Connects to a server that accepts each connection and hangs up at once,
    /// as Murmur does to an address it has autobanned. If `read_hello`, it
    /// reads the client's TLS hello first, so it closes cleanly instead of
    /// resetting the connection.
    async fn connect_to_a_server_that_hangs_up(
        read_hello: bool,
    ) -> proto::client::MumbleConnectionError {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                if read_hello {
                    let _ = socket.read(&mut [0; 4096]).await;
                }
                drop(socket);
            }
        });

        proto::client::connect(ConnectionOptions {
            server: "localhost".into(),
            port: Some(port),
            password: None,
            username: Some("mumbo-test".into()),
        })
        .await
        .err()
        .expect("the server hung up")
    }

    #[tokio::test]
    async fn explains_a_server_that_hangs_up_while_connecting() {
        for read_hello in [true, false] {
            let error = connect_to_a_server_that_hangs_up(read_hello).await;

            let description = describe_connect_error(&error);
            assert!(description.contains("hung up"), "unclear: {description}");
            // The original error stays, for anyone debugging.
            assert!(description.contains(&error.to_string()), "lost the cause: {description}");
        }
    }

    /// A server state with an admin in it, as session 1.
    fn state_with_admin() -> proto::state::MumbleState {
        let mut state = proto::state::MumbleState::default();
        state.users.insert(
            1,
            proto::state::User {
                session: 1,
                name: "admin".into(),
                ..Default::default()
            },
        );
        state
    }

    fn user_remove(session: u32, actor: Option<u32>, reason: Option<&str>, ban: Option<bool>) -> proto::ServerToClientMessage {
        proto::ServerToClientMessage::UserRemove(proto::raw::UserRemove {
            session,
            actor,
            reason: reason.map(Into::into),
            ban,
        })
    }

    #[test]
    fn notices_an_admin_kicking_us() {
        let message = user_remove(7, Some(1), Some("go away"), None);
        assert_eq!(
            removal_of(&message, 7, &state_with_admin()),
            Some(DisconnectReason::Removed {
                reason: Some("go away".into()),
                by: Some("admin".into()),
                banned: false,
            })
        );
    }

    #[test]
    fn notices_an_admin_banning_us() {
        let message = user_remove(7, Some(1), Some("go away"), Some(true));
        assert!(matches!(
            removal_of(&message, 7, &state_with_admin()),
            Some(DisconnectReason::Removed { banned: true, .. })
        ));
    }

    #[test]
    fn notices_the_server_removing_us_with_no_admin_or_reason() {
        // Murmur sends an empty reason when there isn't one.
        let message = user_remove(7, None, Some(""), None);
        assert_eq!(
            removal_of(&message, 7, &state_with_admin()),
            Some(DisconnectReason::Removed { reason: None, by: None, banned: false })
        );
    }

    #[test]
    fn ignores_other_users_leaving() {
        let message = user_remove(8, Some(1), Some("go away"), None);
        assert_eq!(removal_of(&message, 7, &state_with_admin()), None);
    }

    #[test]
    fn a_removal_explains_a_disconnect_best() {
        let removed = DisconnectReason::Removed { reason: None, by: None, banned: false };
        assert_eq!(
            disconnect_reason(Some(removed.clone()), Some("IO error".into())),
            removed
        );
    }

    #[test]
    fn without_a_removal_the_sdk_says_how_the_connection_ended() {
        assert_eq!(disconnect_reason(None, None), DisconnectReason::ServerClosed);
        assert_eq!(
            disconnect_reason(None, Some("IO error: peer closed connection".into())),
            DisconnectReason::ConnectionLost { detail: "IO error: peer closed connection".into() }
        );
    }

    /// The shape `src/invoke/connection.ts` parses.
    #[test]
    fn disconnect_reasons_serialize_as_tagged_objects() {
        let json = |reason| serde_json::to_value(Disconnected { reason }).unwrap();
        assert_eq!(
            json(DisconnectReason::Removed { reason: Some("go away".into()), by: None, banned: true }),
            serde_json::json!({ "reason": { "kind": "removed", "reason": "go away", "by": null, "banned": true } })
        );
        assert_eq!(
            json(DisconnectReason::ServerClosed),
            serde_json::json!({ "reason": { "kind": "server_closed" } })
        );
        assert_eq!(
            json(DisconnectReason::ConnectionLost { detail: "IO error".into() }),
            serde_json::json!({ "reason": { "kind": "connection_lost", "detail": "IO error" } })
        );
    }

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
