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
#[cfg(test)]
mod test_util;
mod universal_mute;

use universal_mute::UniversalMuter;

struct MumbleConnection {
    client: proto::client::MumbleClient,

    /// Volume adjustments keyed by user session ID.
    ///
    /// TODO: store these one time in the Mixer?
    /// TODO: key by an immutable user ID, for durable adjustments between connections.
    volume_adjustments: watch::Sender<HashMap<u32, f32>>
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
        mut events,
    } = proto::client::connect(ConnectionOptions {
        server,
        port: Some(port),
        password: Some(password),
        username: Some(name),
    })
    .await
    .map_err(|e| Error::other(format!("Failed to connect: {}", describe_connect_error(&e))))?;

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

    // Send state events to the app.
    let status_client = client.clone();
    let event_volumes = volumes_tx.clone();
    tauri::async_runtime::spawn(async move {
        let mut shown_channel: Option<String> = None;
        while let Some(event) = events.recv().await {
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
        }
        // `recv` only returns `None` once the driver has exited, so the
        // connection is over, however it ended.
        // TODO: clear AppState and tell the UI. Careful: by now a newer
        // connection may be in AppState, so only clear it if it's still ours.
        log::info!("Connection ended");
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
            Mixer::new(audio, volumes_rx),
        )
        .map_err(|e| Error::other(format!("Failed to start audio module: {}", e)))?;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = audio_task.await {
            // TODO: better reporting.
            log::error!("Audio ended with error: {e}");
        }
    });

    *state.connection.lock().unwrap() = Some(MumbleConnection {
        client,
        volume_adjustments: volumes_tx
    });

    log::info!("Connected to server");
    Ok(())
}

const OUTPUT_SAMPLE_RATE: u32 = audio::SPEAKER_SAMPLE_RATE;

/// 10 ms at 48 kHz: the frame size `NetEq::get_audio` produces.
const MIX_FRAME_SAMPLES: usize = audio::SPEAKER_FRAME_SAMPLES;

/// NetEq routes packets to decoders by RTP payload type; we only have Opus.
const OPUS_PAYLOAD_TYPE: u8 = 0;

/// Adapts libopus to NetEq's decoder interface.
struct OpusDecoder(opus::Decoder);

impl neteq::codec::AudioDecoder for OpusDecoder {
    fn sample_rate(&self) -> u32 {
        OUTPUT_SAMPLE_RATE
    }

    fn channels(&self) -> u8 {
        1
    }

    fn decode(&mut self, encoded: &[u8]) -> neteq::Result<Vec<f32>> {
        // 120 ms, Opus's largest frame.
        let mut pcm = vec![0.0; OUTPUT_SAMPLE_RATE as usize * 120 / 1000];
        let count = self
            .0
            .decode_float(encoded, &mut pcm, false)
            .map_err(|e| neteq::NetEqError::DecoderError(e.to_string()))?;
        pcm.truncate(count);
        Ok(pcm)
    }
}

/// One remote talker: a jitter buffer that decodes their Opus packets.
struct Speaker {
    neteq: neteq::NetEq,
    // Over the TCP tunnel packets arrive complete and in order, so a running
    // count is an exact sequence/timestamp. UDP will need Mumble's frame_number.
    next_sequence: u16,
    next_timestamp: u32,

    /// Volume adjustment in decibels. 0db is no change. Negative is quieter.
    volume_adjust_db: f32,
}

impl Speaker {
    fn new() -> Result<Self, Error> {
        let mut neteq = neteq::NetEq::new(neteq::NetEqConfig {
            sample_rate: OUTPUT_SAMPLE_RATE,
            channels: 1,
            ..Default::default()
        })
        .map_err(Error::other)?;
        let decoder =
            opus::Decoder::new(OUTPUT_SAMPLE_RATE, opus::Channels::Mono).map_err(Error::other)?;
        neteq.register_decoder(OPUS_PAYLOAD_TYPE, Box::new(OpusDecoder(decoder)));
        Ok(Self {
            neteq,
            next_sequence: 0,
            next_timestamp: 0,
            volume_adjust_db: 0.0,
        })
    }

    fn insert_packet(&mut self, session: u32, opus_data: &[u8]) -> Result<(), Error> {
        let samples =
            opus::packet::get_nb_samples(opus_data, OUTPUT_SAMPLE_RATE).map_err(Error::other)?;
        let header = neteq::RtpHeader::new(
            self.next_sequence,
            self.next_timestamp,
            session,
            OPUS_PAYLOAD_TYPE,
            false,
        );
        let duration_ms = (samples * 1000 / OUTPUT_SAMPLE_RATE as usize) as u32;
        let packet = neteq::AudioPacket::new(
            header,
            opus_data.to_vec(),
            OUTPUT_SAMPLE_RATE,
            1,
            duration_ms,
        );
        self.neteq.insert_packet(packet).map_err(Error::other)?;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.next_timestamp = self.next_timestamp.wrapping_add(samples as u32);
        Ok(())
    }

    fn set_volume_adjust_db(&mut self, volume_db: f32) {
        self.volume_adjust_db = volume_db;
    }
}

/// Decodes the packets arriving from the server and mixes everyone talking.
struct Mixer {
    packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>,

    /// Receiver for volume adjustments (session ID, volume in decibels)
    volume_adjustments: watch::Receiver<HashMap<u32, f32>>,

    speakers: HashMap<u32, Speaker>,
}

impl Mixer {
    /// Creates a new Mixer with the given packet receiver and volume adjustment receiver.
    fn new(packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>,
           volume_adjustments: watch::Receiver<HashMap<u32, f32>>) -> Self {
        Self {
            packets,
            volume_adjustments,
            speakers: HashMap::new(),
        }
    }

    fn get_or_create_speaker(map: &mut HashMap<u32, Speaker>, session: u32) -> Result<&mut Speaker, Error> {
        Ok(match map.entry(session) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Speaker::new()?),
        })
    }
}

impl audio::SpeakerSource for Mixer {
    fn next_frame(&mut self) -> Result<Option<[i16; MIX_FRAME_SAMPLES]>, Error> {
        // Apply any pending volume adjustments before processing new packets.
        if self.volume_adjustments.has_changed().unwrap_or(false) {
            // Copy out so the guard drops at once: holding it while speakers are
            // created below would block the UI's writes for that long.
            let adjustments = self.volume_adjustments.borrow_and_update().clone();
            // Speakers whose entry was removed go back to full volume.
            for (session, speaker) in self.speakers.iter_mut() {
                speaker.set_volume_adjust_db(adjustments.get(session).copied().unwrap_or(0.0));
            }
            for (session, volume_db) in adjustments {
                Self::get_or_create_speaker(&mut self.speakers, session)?
                    .set_volume_adjust_db(volume_db);
            }
        }

        loop {
            match self.packets.try_recv() {
                Ok(packet) => {
                    let sender = packet.sender_session;
                    let speaker = Self::get_or_create_speaker(&mut self.speakers, sender)?;
                    if let Err(e) = speaker.insert_packet(sender, &packet.opus_data) {
                        log::warn!("Dropping audio from session {sender}: {e}");
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                // Only once every queued packet has been taken: we disconnected.
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(None),
            }
        }
        mix_frame(&mut self.speakers).map(Some)
    }
}

/// Mixes 10 ms from every speaker who has audio buffered.
fn mix_frame(speakers: &mut HashMap<u32, Speaker>) -> Result<[i16; MIX_FRAME_SAMPLES], Error> {
    let mut sum = [0.0f32; MIX_FRAME_SAMPLES];
    for speaker in speakers.values_mut() {
        // Skip idle speakers so NetEq doesn't synthesize concealment for silence.
        if speaker.neteq.current_buffer_size_samples() == 0 {
            continue;
        }
        let audio = speaker.neteq.get_audio().map_err(Error::other)?;
        let scale_factor = db_to_linear(speaker.volume_adjust_db);
        for (out, sample) in sum.iter_mut().zip(audio.samples) {
            *out += sample * scale_factor;
        }
    }
    Ok(sum.map(|sample| (soft_clip(sample) * i16::MAX as f32) as i16))
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

/// Converts a volume in decibels to a linear scale factor.
/// https://stackoverflow.com/a/46153568/788168
fn db_to_linear(volume_db: f32) -> f32 {
    10f32.powf(volume_db / 20.0)
}

/// Passes normal levels through untouched and smoothly bends anything louder
/// (e.g. overlapping speakers) toward, but never past, full scale.
fn soft_clip(sample: f32) -> f32 {
    const THRESHOLD: f32 = 0.8;
    let magnitude = sample.abs();
    if magnitude <= THRESHOLD {
        return sample;
    }
    // tanh's slope is 1 at 0, so the curve continues the straight line at
    // THRESHOLD without a kink, and approaches 1.0 asymptotically.
    let headroom = 1.0 - THRESHOLD;
    let bent = THRESHOLD + headroom * ((magnitude - THRESHOLD) / headroom).tanh();
    bent.copysign(sample)
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
    use crate::test_util::tone_strength;

    /// Opus-encodes `packets` × 20 ms of a sine tone, as a sender would.
    fn tone_packets(frequency: f32, amplitude: f32, packets: usize) -> Vec<Vec<u8>> {
        const PACKET_SAMPLES: usize = 960;
        let mut encoder = opus::Encoder::new(
            OUTPUT_SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .unwrap();
        (0..packets)
            .map(|packet| {
                let pcm: Vec<f32> = (0..PACKET_SAMPLES)
                    .map(|i| {
                        let t = (packet * PACKET_SAMPLES + i) as f32 / OUTPUT_SAMPLE_RATE as f32;
                        (t * frequency * std::f32::consts::TAU).sin() * amplitude
                    })
                    .collect();
                let mut encoded = [0u8; 4000];
                let len = encoder.encode_float(&pcm, &mut encoded).unwrap();
                encoded[..len].to_vec()
            })
            .collect()
    }

    fn energy(frame: &[i16]) -> f32 {
        frame
            .iter()
            .map(|&s| (s as f32 / i16::MAX as f32).powi(2))
            .sum()
    }

    #[test]
    fn speaker_audio_comes_out_of_the_mix() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);
        for packet in tone_packets(440.0, 0.5, 25) {
            speakers
                .get_mut(&7)
                .unwrap()
                .insert_packet(7, &packet)
                .unwrap();
        }

        let total: f32 = (0..50)
            .map(|_| energy(&mix_frame(&mut speakers).unwrap()))
            .sum();

        // A 0.5-amplitude tone has mean power 0.125 per sample; 500 ms of it is
        // ~3000. Require a healthy fraction to allow for NetEq's startup delay.
        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    #[test]
    fn two_speakers_are_heard_together() {
        let mut speakers =
            HashMap::from([(1, Speaker::new().unwrap()), (2, Speaker::new().unwrap())]);
        // Quiet enough that the sum stays below soft_clip's threshold.
        let alice = tone_packets(440.0, 0.25, 25);
        let bob = tone_packets(1000.0, 0.25, 25);
        for (alice_packet, bob_packet) in alice.iter().zip(&bob) {
            speakers
                .get_mut(&1)
                .unwrap()
                .insert_packet(1, alice_packet)
                .unwrap();
            speakers
                .get_mut(&2)
                .unwrap()
                .insert_packet(2, bob_packet)
                .unwrap();
        }

        let mixed: Vec<f32> = (0..50)
            .flat_map(|_| mix_frame(&mut speakers).unwrap())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect();

        let (alice_level, bob_level) =
            (tone_strength(&mixed, 440.0), tone_strength(&mixed, 1000.0));
        assert!(alice_level > 0.1, "440 Hz missing from mix: {alice_level}");
        assert!(bob_level > 0.1, "1000 Hz missing from mix: {bob_level}");
        // An unrelated frequency stays near zero, so the levels above are the tones, not noise.
        assert!(tone_strength(&mixed, 3000.0) < 0.02);
    }

    fn audio_packet(session: u32, opus_data: Vec<u8>) -> proto::raw_udp::Audio {
        proto::raw_udp::Audio {
            sender_session: session,
            opus_data,
            ..Default::default()
        }
    }

    /// Volumes that never change: the sender is dropped right away.
    fn no_volume_adjustments() -> watch::Receiver<HashMap<u32, f32>> {
        watch::channel(HashMap::new()).1
    }

    #[test]
    fn mixer_plays_the_packets_it_receives() {
        use crate::audio::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let mut mixer = Mixer::new(receiver, no_volume_adjustments());
        for packet in tone_packets(440.0, 0.5, 25) {
            sender.try_send(audio_packet(7, packet)).unwrap();
        }

        let total: f32 = (0..50)
            .map(|_| energy(&mixer.next_frame().unwrap().expect("still connected")))
            .sum();

        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    #[test]
    fn mixer_applies_a_speakers_volume_adjustment() {
        use crate::audio::SpeakerSource;
        let mixed_energy = |volume_db: Option<f32>| {
            let (sender, receiver) = tokio::sync::mpsc::channel(64);
            let (volumes, volumes_rx) = watch::channel(HashMap::new());
            let mut mixer = Mixer::new(receiver, volumes_rx);
            if let Some(volume_db) = volume_db {
                volumes.send_modify(|volumes| {
                    volumes.insert(7, volume_db);
                });
            }
            for packet in tone_packets(440.0, 0.5, 25) {
                sender.try_send(audio_packet(7, packet)).unwrap();
            }
            (0..50)
                .map(|_| energy(&mixer.next_frame().unwrap().expect("still connected")))
                .sum::<f32>()
        };

        // -6 dB halves the amplitude, which quarters the energy.
        let ratio = mixed_energy(Some(-6.0)) / mixed_energy(None);
        assert!((ratio - 0.25).abs() < 0.02, "energy ratio was {ratio}");
    }

    #[test]
    fn mixer_plays_at_full_volume_once_an_adjustment_is_removed() {
        use crate::audio::SpeakerSource;
        let tone_energy = |mixer: &mut Mixer| {
            let (sender, receiver) = tokio::sync::mpsc::channel(64);
            mixer.packets = receiver;
            for packet in tone_packets(440.0, 0.5, 25) {
                sender.try_send(audio_packet(7, packet)).unwrap();
            }
            (0..50)
                .map(|_| energy(&mixer.next_frame().unwrap().expect("still connected")))
                .sum::<f32>()
        };
        // Both mixers play a tone before the one measured, so their jitter
        // buffers are equally warmed up.
        let mut unadjusted = Mixer::new(tokio::sync::mpsc::channel(1).1, no_volume_adjustments());
        tone_energy(&mut unadjusted);
        let (volumes, volumes_rx) = watch::channel(HashMap::new());
        let mut mixer = Mixer::new(tokio::sync::mpsc::channel(1).1, volumes_rx);
        volumes.send_modify(|volumes| {
            volumes.insert(7, -6.0);
        });
        tone_energy(&mut mixer);

        // As when session 7 leaves and the server hands the session to someone new.
        volumes.send_modify(|volumes| {
            volumes.remove(&7);
        });

        let ratio = tone_energy(&mut mixer) / tone_energy(&mut unadjusted);
        assert!((ratio - 1.0).abs() < 0.05, "energy ratio was {ratio}");
    }

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

    #[test]
    fn mixer_ends_when_the_connection_closes() {
        use crate::audio::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let mut mixer = Mixer::new(receiver, no_volume_adjustments());

        let idle = mixer.next_frame().unwrap().expect("connected, just quiet");
        assert_eq!(idle, [0; MIX_FRAME_SAMPLES]);

        drop(sender);
        assert!(mixer.next_frame().unwrap().is_none());
    }

    /// Needs the dev server from dev/mumble-server (`npm run mumble:up`), on
    /// localhost:64738 with password "test". Two clients join the root channel;
    /// one talks for a second and the other's mixer mixes it, with the talker's
    /// volume adjusted to `volume_db` if given. Returns each 100 ms of the mix as
    /// (RMS, strength of an unrelated frequency).
    async fn mix_a_talker(test: &str, volume_db: Option<f32>) -> Vec<(f32, f32)> {
        use crate::audio::SpeakerSource;
        // Everyone lands in the root channel, so tests running at once would
        // hear each other's talkers. Take turns.
        static SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _turn = SERVER.lock().await;
        let connect = |role: &str| {
            proto::client::connect(ConnectionOptions {
                server: "localhost".into(),
                port: Some(64738),
                password: Some("test".into()),
                // Unique names, even across tests running at once: a duplicate
                // name kicks the earlier session.
                username: Some(format!("mumbo-e2e-{test}-{role}-{}", std::process::id())),
            })
        };
        let listener = connect("listener").await.expect("connecting the listener");
        let talker = connect("talker").await.expect("connecting the talker");
        let talker_session = talker
            .client
            .state()
            .lock()
            .unwrap()
            .current_user()
            .expect("the talker knows who it is")
            .session;

        let talking = {
            let client = talker.client.clone();
            tokio::spawn(async move {
                let packets = tone_packets(440.0, 0.5, 50); // one second
                for (i, packet) in packets.iter().enumerate() {
                    // Frame numbers count 10 ms; a packet is 20 ms.
                    client
                        .send_audio(packet, i as u64 * 2, i == packets.len() - 1)
                        .unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
        };

        let (volumes, volumes_rx) = watch::channel(HashMap::new());
        let mut mixer = Mixer::new(listener.audio, volumes_rx);
        if let Some(volume_db) = volume_db {
            volumes.send_modify(|volumes| {
                volumes.insert(talker_session, volume_db);
            });
        }
        let mut mixed = Vec::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(10));
        for _ in 0..250 {
            tick.tick().await;
            let frame = mixer
                .next_frame()
                .unwrap()
                .expect("the listener stayed connected");
            mixed.extend(frame.map(|s| s as f32 / i16::MAX as f32));
        }
        talking.await.unwrap();
        listener.client.disconnect();
        talker.client.disconnect();

        // Judge 100 ms at a time. The level is checked, not the pitch: NetEq's time
        // stretching bends a pure tone when audio arrives in real time (see TODO.md).
        mixed
            .as_chunks::<{ MIX_FRAME_SAMPLES * 10 }>()
            .0
            .iter()
            .map(|w| {
                let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
                (rms, tone_strength(w, 3000.0))
            })
            .collect()
    }

    /// Exercises the network, decoding and mixing together. Run with
    /// `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the dev Mumble server (dev/mumble-server) on localhost:64738"]
    async fn audio_sent_by_one_client_is_mixed_for_another() {
        let windows = mix_a_talker("plain", None).await;
        // A 0.5-amplitude sine has an RMS of about 0.35.
        let loud = windows.iter().filter(|(rms, _)| *rms > 0.25).count();
        assert!(
            loud >= 5,
            "the talker was heard in only {loud} of {} windows: {windows:.3?}",
            windows.len()
        );
        assert!(
            windows.iter().all(|(_, other)| *other < 0.02),
            "unrelated frequency in the mix"
        );
    }

    /// Adds volume adjustment to the above. Run with `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the dev Mumble server (dev/mumble-server) on localhost:64738"]
    async fn a_talkers_volume_adjustment_applies_to_their_mixed_audio() {
        let windows = mix_a_talker("volume", Some(-12.0)).await;
        // -12 dB is a quarter of the amplitude: an RMS of about 0.09, not 0.35.
        let heard = windows.iter().filter(|(rms, _)| *rms > 0.06).count();
        assert!(
            heard >= 5,
            "the talker was heard in only {heard} of {} windows: {windows:.3?}",
            windows.len()
        );
        assert!(
            windows.iter().all(|(rms, _)| *rms < 0.15),
            "the talker wasn't turned down: {windows:.3?}"
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

    #[test]
    fn soft_clip_leaves_normal_levels_untouched() {
        for sample in [0.0, 0.1, -0.5, 0.79, -0.79] {
            assert_eq!(soft_clip(sample), sample);
        }
    }

    #[test]
    fn soft_clip_keeps_overlaps_below_full_scale() {
        // Mathematically the curve stays strictly below 1.0, but for large inputs
        // tanh rounds to exactly 1.0 in f32. Full scale itself is still safe.
        for sample in [0.9, 1.0, 1.5, 3.0, 100.0] {
            let clipped = soft_clip(sample);
            assert!(clipped <= 1.0, "soft_clip({sample}) = {clipped}");
            assert_eq!(soft_clip(-sample), -clipped);
        }
        assert!(
            soft_clip(1.0) < 1.0,
            "a moderate overlap should keep some headroom"
        );
    }

    #[test]
    fn soft_clip_is_smooth_and_increasing() {
        // No jump at the threshold, and louder input never comes out quieter.
        let samples: Vec<f32> = (0..=400).map(|i| i as f32 / 100.0).collect();
        for pair in samples.windows(2) {
            let (a, b) = (soft_clip(pair[0]), soft_clip(pair[1]));
            assert!(
                b >= a,
                "soft_clip decreased between {} and {}",
                pair[0],
                pair[1]
            );
            assert!(
                b - a <= 0.01 + f32::EPSILON,
                "soft_clip jumped between {} and {}",
                pair[0],
                pair[1]
            );
        }
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

    #[test]
    fn idle_speaker_mixes_to_silence() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);

        let frame = mix_frame(&mut speakers).unwrap();

        assert_eq!(frame, [0; MIX_FRAME_SAMPLES]);
    }
}
