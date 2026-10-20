use std::collections::HashMap;
use std::io::Error;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use mumble_protocol as proto;
use mumble_protocol::client::{Connection, ConnectionOptions};
use opus::Channels::Mono;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use tauri::Emitter;
use tauri::ipc::Channel;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tokio::sync::broadcast;

#[derive(Default)]
struct AppState {
    client: std::sync::Mutex<Option<proto::client::MumbleClient>>,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
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
        mut audio,
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

    // Send state events to the app.
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            // TODO: a useful payload.
            let _ = app.emit("session-changed", event);
        }
        // `recv` only returns `None` once the driver has exited, so the
        // connection is over, however it ended.
        // TODO: clear AppState and tell the UI. Careful: by now a newer
        // connection may be in AppState, so only clear it if it's still ours.
        log::info!("Connection ended");
    });

    let mut speakers: HashMap<u32, Speaker> = HashMap::new();

    // Match Opus sample rate for output.
    const HEADROOM_MS: u32 = 200;
    const HEADROOM_SAMPLES: u32 = (OUTPUT_SAMPLE_RATE * HEADROOM_MS) / 1000;

    let (mut output_producer, mut output_consumer) = HeapRb::<i16>::new(HEADROOM_SAMPLES as usize).split();
    let (mut input_producer, mut input_consumer) = HeapRb::<f32>::new(HEADROOM_SAMPLES as usize).split();

    // TODO: allow user to select output device and configure sample rate.
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| Error::other("No audio output device"))?;
    let preferred = device
        .supported_output_configs()
        .map_err(Error::other)?
        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
        .find_map(|c| c.try_with_sample_rate(OUTPUT_SAMPLE_RATE));
    let config = match preferred {
        Some(supported) => supported.config(),
        None => device
            .default_output_config()
            .map_err(Error::other)?
            .config(),
    };

    let input_device = host
        .default_input_device()
        .ok_or_else(|| Error::other("No audio input device"))?;
    let input_config = input_device
        .supported_input_configs()
        .map_err(Error::other)?
        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
        .find_map(|c| c.try_with_sample_rate(OUTPUT_SAMPLE_RATE));
    let input_config = match input_config {
        Some(supported) => supported.config(),
        None => input_device
            .default_input_config()
            .map_err(Error::other)?
            .config(),
    };

    let channels = config.channels as usize;

    // Start the audio loop!
    let cloned_client = client.clone();
    tauri::async_runtime::spawn(async move {
        // NetEq absorbs network jitter; this only needs to cover timer slop.
        const MIX_TARGET_SAMPLES: usize = (OUTPUT_SAMPLE_RATE as usize / 1000) * 40;
        let mut mix_interval = tokio::time::interval(std::time::Duration::from_millis(5));

        let result = async {
            // Playback stops when `stream` is dropped, so it lives for the whole loop.
            let stream = device.build_output_stream(
                config,
                move |output: &mut [f32], _| {
                    // same value in left and right
                    for frame in output.chunks_mut(channels) {
                        let sample = output_consumer.try_pop().unwrap_or(0) as f32 / i16::MAX as f32;
                        frame.fill(sample);
                    }
                },
                |err| log::error!("audio stream error: {err}"),
                None,
            ).map_err(Error::other)?;
            stream.play().map_err(Error::other)?;

            let mic_stream = input_device.build_input_stream(
                input_config,
                move |input: &[f32], _| {
                    for frame in input.chunks(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32;
                        let _ = input_producer.try_push(mono);
                    }
                },
                |err| log::error!("mic stream error: {err}"),
                None,
            ).map_err(Error::other)?;
            mic_stream.play().map_err(Error::other)?;

            let mut encoder = opus::Encoder::new(
                OUTPUT_SAMPLE_RATE,
                Mono,
                opus::Application::Voip
            ).map_err(Error::other)?;

            // Pre-allocate buffers (I think these are on the heap so not huge prob).
            let mut mic_frame = vec![0.0; MIC_FRAME_SAMPLES];
            let mut output = vec![0u8; 4000];
            let mut frame_counter: u64 = 0;

            loop {
                tokio::select! {
                    audio = audio.recv() => {
                        // If we disconnect, stop the loop!
                        let audio = match audio {
                            Some(audio) => audio,
                            None => break,
                        };

                        // TODO: volume adjustment
                        let sender = audio.sender_session;

                        let speaker = match speakers.entry(sender) {
                            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Speaker::new()?),
                        };
                        if let Err(e) = speaker.insert_packet(sender, &audio.opus_data) {
                            log::warn!("Dropping audio from session {sender}: {e}");
                        }
                    }
                    _ = mix_interval.tick() => {
                        // Top the ring buffer up to a fixed level rather than mixing once per
                        // tick: the card drains it at its own rate, so this follows the card's
                        // clock instead of drifting against this timer.
                        while output_producer.occupied_len() < MIX_TARGET_SAMPLES {
                            let frame = mix_frame(&mut speakers)?;
                            output_producer.push_slice(&frame);
                        }

                        // Send complete mic frames
                        while input_consumer.occupied_len() >= MIC_FRAME_SAMPLES {
                            input_consumer.pop_slice(&mut mic_frame);
                            let len = encoder.encode_float(&mic_frame, &mut output).map_err(Error::other)?;
                            cloned_client.send_audio(&output[..len], frame_counter).map_err(Error::other)?;
                            // Opus expects 10ms chunks, we send 20ms.
                            frame_counter += 2;
                        }
                    }
                }
            }
            Ok::<(), Error>(())
        }.await;
        if let Err(e) = result {
            // TODO: better reporting.
            log::error!("Audio loop ended with error: {e}");
        }
    });

    *state.client.lock().unwrap() = Some(client);

    log::info!("Connected to server");
    Ok(())
}

const OUTPUT_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size `NetEq::get_audio` produces.
const MIX_FRAME_SAMPLES: usize = OUTPUT_SAMPLE_RATE as usize / 100;

/// 20 ms at 48 kHz: the frame size for microphone input.
const MIC_FRAME_SAMPLES: usize = OUTPUT_SAMPLE_RATE as usize / 50;

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
        .manage(AppState::default())
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
            connect,
            get_users,
            get_chat_messages,
            send_channel_message,
            disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Amplitude of the `frequency` component of `samples` (a single DFT bin).
    fn tone_strength(samples: &[f32], frequency: f32) -> f32 {
        let (re, im) = samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (n, &x)| {
                let phase =
                    std::f32::consts::TAU * frequency * n as f32 / OUTPUT_SAMPLE_RATE as f32;
                (re + x * phase.cos(), im - x * phase.sin())
            });
        2.0 * (re * re + im * im).sqrt() / samples.len() as f32
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
