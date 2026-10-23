use std::collections::HashMap;
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
#[cfg(test)]
mod test_util;

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
fn set_self_mute(state: tauri::State<'_, AppState>, muted: bool) -> Result<(), String> {
    if let Some(client) = state.client.lock().unwrap().as_ref() {
        client.set_self_mute(muted).map_err(|e| e.to_string())
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

const AUDIO_INPUT_DEVICES_EVENT: &str = "audio-input-devices-changed";
const AUDIO_OUTPUT_DEVICES_EVENT: &str = "audio-output-devices-changed";
/// The device the user prefers (an `AudioDevice`), or null to follow the system default.
const AUDIO_INPUT_SELECTED_EVENT: &str = "audio-input-device-selected";
const AUDIO_OUTPUT_SELECTED_EVENT: &str = "audio-output-device-selected";
/// Sent by the UI once its device listeners are registered. Events emitted
/// earlier would be lost, so this is the cue to publish everything current.
const AUDIO_DEVICES_SUBSCRIBE_EVENT: &str = "audio-devices-subscribe";

const SETTINGS_FILE: &str = "settings.json";
/// Where the preferred audio devices are saved. The UI's own settings share the
/// file (under another key); the store plugin keeps one copy per file.
const AUDIO_SETTINGS_KEY: &str = "audio";

/// Saves the preferred devices each time either one changes.
fn save_audio_preferences(store: Arc<tauri_plugin_store::Store<tauri::Wry>>, audio: &audio::AudioModule) {
    let input = audio.preferred_input_device();
    let output = audio.preferred_output_device();
    for changes in [input.clone(), output.clone()] {
        let (store, input, output) = (store.clone(), input.clone(), output.clone());
        tauri::async_runtime::spawn(forward_watch(changes, move |_| {
            let saved = audio::SavedPreferences::of(input.borrow().as_ref(), output.borrow().as_ref());
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
            if let (Some(muter), Some(muted)) = (universal_muter.as_mut(), confirmed_mute) {
                if let Err(e) = muter.confirm(muted) {
                    log::warn!("Universal mute couldn't update the button: {e}");
                }
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
        }, Mixer::new(audio))
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
}

/// Decodes the packets arriving from the server and mixes everyone talking.
struct Mixer {
    packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>,
    speakers: HashMap<u32, Speaker>,
}

impl Mixer {
    fn new(packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>) -> Self {
        Self { packets, speakers: HashMap::new() }
    }
}

impl audio::SpeakerSource for Mixer {
    fn next_frame(&mut self) -> Result<Option<[i16; MIX_FRAME_SAMPLES]>, Error> {
        loop {
            match self.packets.try_recv() {
                Ok(packet) => {
                    // TODO: volume adjustment
                    let sender = packet.sender_session;
                    let speaker = match self.speakers.entry(sender) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Speaker::new()?),
                    };
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
        for (out, sample) in sum.iter_mut().zip(audio.samples) {
            *out += sample;
        }
    }
    Ok(sum.map(|sample| (soft_clip(sample) * i16::MAX as f32) as i16))
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
                client: Default::default(),
                audio_module,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("mumbo_lib", log::LevelFilter::Trace)
                .level_for("mumble_protocol", log::LevelFilter::Trace)
                .with_colors(ColoredLevelConfig::default())
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            set_audio_input_device,
            set_audio_output_device,
            get_server_stats,
            connect,
            get_users,
            get_chat_messages,
            send_channel_message,
            set_self_mute,
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
        proto::raw_udp::Audio { sender_session: session, opus_data, ..Default::default() }
    }

    #[test]
    fn mixer_plays_the_packets_it_receives() {
        use crate::audio::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let mut mixer = Mixer::new(receiver);
        for packet in tone_packets(440.0, 0.5, 25) {
            sender.try_send(audio_packet(7, packet)).unwrap();
        }

        let total: f32 = (0..50)
            .map(|_| energy(&mixer.next_frame().unwrap().expect("still connected")))
            .sum();

        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    #[test]
    fn mixer_ends_when_the_connection_closes() {
        use crate::audio::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let mut mixer = Mixer::new(receiver);

        let idle = mixer.next_frame().unwrap().expect("connected, just quiet");
        assert_eq!(idle, [0; MIX_FRAME_SAMPLES]);

        drop(sender);
        assert!(mixer.next_frame().unwrap().is_none());
    }

    /// Needs the dev server from dev/mumble-server (`npm run mumble:up`), on
    /// localhost:64738 with password "test". Run with `cargo test -- --ignored`.
    /// Two clients join the root channel; one talks and the other's mixer should
    /// produce the tone, exercising the network, decoding and mixing together.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the dev Mumble server (dev/mumble-server) on localhost:64738"]
    async fn audio_sent_by_one_client_is_mixed_for_another() {
        use crate::audio::SpeakerSource;
        let connect = |role: &str| {
            proto::client::connect(ConnectionOptions {
                server: "localhost".into(),
                port: Some(64738),
                password: Some("test".into()),
                // Unique names: a duplicate name kicks the earlier session.
                username: Some(format!("mumbo-e2e-{role}-{}", std::process::id())),
            })
        };
        let listener = connect("listener").await.expect("connecting the listener");
        let talker = connect("talker").await.expect("connecting the talker");

        let talking = {
            let client = talker.client.clone();
            tokio::spawn(async move {
                let packets = tone_packets(440.0, 0.5, 50); // one second
                for (i, packet) in packets.iter().enumerate() {
                    // Frame numbers count 10 ms; a packet is 20 ms.
                    client.send_audio(packet, i as u64 * 2, i == packets.len() - 1).unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
        };

        let mut mixer = Mixer::new(listener.audio);
        let mut mixed = Vec::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(10));
        for _ in 0..250 {
            tick.tick().await;
            let frame = mixer.next_frame().unwrap().expect("the listener stayed connected");
            mixed.extend(frame.map(|s| s as f32 / i16::MAX as f32));
        }
        talking.await.unwrap();
        listener.client.disconnect();
        talker.client.disconnect();

        // Judge 100 ms at a time. The level is checked, not the pitch: NetEq's time
        // stretching bends a pure tone when audio arrives in real time (see TODO.md).
        let windows: Vec<(f32, f32)> = mixed
            .chunks_exact(MIX_FRAME_SAMPLES * 10)
            .map(|w| {
                let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
                (rms, tone_strength(w, 3000.0))
            })
            .collect();
        // A 0.5-amplitude sine has an RMS of about 0.35.
        let loud = windows.iter().filter(|(rms, _)| *rms > 0.25).count();
        assert!(loud >= 5, "the talker was heard in only {loud} of {} windows: {windows:.3?}", windows.len());
        assert!(windows.iter().all(|(_, other)| *other < 0.02), "unrelated frequency in the mix");
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
