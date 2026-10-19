use std::collections::HashMap;
use std::io::Error;

use futures_util::SinkExt;
use futures_util::StreamExt;
use mumble_protocol::codec::MumbleCodec;
use mumble_protocol as proto;
use ringbuf::HeapRb;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tauri::{AppHandle, ipc::Channel};
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::net::TcpStream;
use tokio::{self, io::AsyncReadExt, io::AsyncWriteExt};
use tokio_rustls::client::TlsStream;
use tokio_util::codec::FramedRead;
use futures_util::TryStreamExt;
use tokio_util::codec::FramedWrite;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};

mod AcceptAnyCert;

// HACK! Global state.
static CANCELLATION_TOKEN: std::sync::Mutex<Option<tokio_util::sync::CancellationToken>> = std::sync::Mutex::new(None);
static CONNECTION: std::sync::Mutex<Option<proto::state::MumbleState>> = std::sync::Mutex::new(None);

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
async fn connect_and_listen(app: AppHandle, server: String, port: u16, password: String, name: String, channel: Channel<proto::ServerToClientMessage>) -> Result<(), String> {
    let _ = connect(server, port, password, name, channel)
        .await
        .inspect_err(|e| log::error!("{e}"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn disconnect(app: AppHandle) -> Result<(), String> {
    if let Some(token) = CANCELLATION_TOKEN.lock().unwrap().as_ref() {
        log::info!("Disconnecting from server");
        token.cancel();
    } else {
        log::info!("No active connection");
    }
    Ok(())
}

#[tauri::command]
fn get_users() -> Vec<proto::state::User> {
    if let Some(connection) = CONNECTION.lock().unwrap().as_ref() {
        return connection.users();
    } else {
        Vec::new()
    }
}

// struct RawMumbleConnection {
//     // tcp_read: ReadHalf<TlsStream<TcpStream>>,
//     // reader: FramedRead<ReadHalf<TlsStream<TcpStream>>, proto::codec::MumbleCodec>,
//     // tcp_write: WriteHalf<TlsStream<TcpStream>>,
//     // CANCELLATION_TOKEN: Option<tokio_util::sync::CancellationToken>,
// }

async fn connect(server: String, port: u16, password: String, name: String, channel: Channel<proto::ServerToClientMessage>) -> Result<(), Error> {
    // HACK! Global state?
    let token = tokio_util::sync::CancellationToken::new();
    *CANCELLATION_TOKEN.lock().unwrap() = Some(token.clone());

    *CONNECTION.lock().unwrap() = Some(proto::state::MumbleState::default());

    // Connection process:
    //
    // https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/establishing_connection.md
    log::info!("Connecting to server {} on port {}", server, port);
    let server_name = rustls::pki_types::ServerName::try_from(server.clone()).map_err(Error::other)?;
    let tcp = tokio::net::TcpStream::connect((server.clone(), port)).await?;
    let tls = AcceptAnyCert::make_connector().map_err(Error::other)?
        .connect(server_name, tcp)
        .await?;
    let (mut tcp_read, mut tcp_write) = tokio::io::split(tls);
    let mut reader = FramedRead::new(tcp_read, proto::codec::MumbleCodec::default())
        .inspect(|result| {
            if let Ok(message) = result {
                if let Some(ref mut connection) = *CONNECTION.lock().unwrap() {
                    connection.apply(message.clone()).unwrap();
                }
                log::trace!("Received message from server: {:?}", message);
                channel.send(message.clone()).unwrap();
            }
        });
    let mut writer = FramedWrite::new(tcp_write, proto::codec::MumbleCodec::default());
    log::trace!("TCP and TLS connection established with server {} on port {}", server, port);

    // Send a version message!
    let (major, minor, patch) = (1u32, 5u32, 0u32);
    let our_version: u32 = (major << 16) | (minor << 8) | patch;
    let our_version_v2: u64 = (u64::from(major) << 48) | (u64::from(minor) << 32) | (u64::from(patch) << 16);

    let version = proto::raw::Version {
        // not necessary, but nice
        os: Some(std::env::consts::OS.into()),
        release: Some("mumbo 0.1.0".into()),
        os_version: None,

        // required
        version_v1: Some(our_version),
        version_v2: Some(our_version_v2),
    };
    writer.send(proto::ClientToServerMessage::Version(version)).await?;
    log::info!("Version message sent to server");

    // Wait for the server's version message.
    //
    // ```
    // [2026-10-15][01:44:44][INFO][mumbo_lib] Received version message: Version
    // { version_v1: Some(67071), version_v2: Some(281496510595072), release:
    // Some("1.5.901"), os: Some("Linux"), os_version: Some("Ubuntu 24.04.4 LTS
    // [arm64]") }
    // ```
    let Some(proto::ServerToClientMessage::Version(version)) = reader
        .try_next()
        .await? else {
        return Err(Error::other("Unexpected server close"));
    };
    log::info!("Received version message: {:?}", version);

    let authenticate = proto::raw::Authenticate {
        username: Some(name.into()),
        password: Some(password.into()),

        // We don't support authentication tokens for now
        tokens: vec![],

        // Outdated params
        celt_versions: vec![],
        opus: Some(true),
        client_type: Some(0),
    };
    writer.send(proto::ClientToServerMessage::Authenticate(authenticate)).await?;
    log::info!("Authenticate message sent to server");

    // We need a bunch of setup messages, which should arrive in order, but no
    // need to be precise:
    let mut cryptSetup: Option<proto::raw::CryptSetup> = None;
    let mut codecVersion: Option<proto::raw::CodecVersion> = None;
    let mut channelState: Vec<proto::raw::ChannelState> = vec![];
    let mut userState: Vec<proto::raw::UserState> = vec![];
    let mut serverSync: Option<proto::raw::ServerSync> = None;

    // /* cryptSetup.is_none() || codecVersion.is_none() || */
    // ^ don't need these.
    //
    // - cryptSetup - only for UDP, which we don't use.
    // - codecVersion - clients use Opus, so this is not useful. Grumble doesn't reliably send.
    while channelState.is_empty() || userState.is_empty() || serverSync.is_none() {
        let message = reader.try_next().await?;
        match &message {
            Some(proto::ServerToClientMessage::Reject(m)) => {
                log::error!("Server rejected setup message: {} - {}", m.r#type().as_str_name(), m.reason());
                return Err(Error::other(format!("Server rejected setup message: {} - {}", m.r#type().as_str_name(), m.reason())));
            }
            Some(proto::ServerToClientMessage::CryptSetup(m)) => cryptSetup = Some(m.clone()),
            Some(proto::ServerToClientMessage::CodecVersion(m)) => codecVersion = Some(m.clone()),
            Some(proto::ServerToClientMessage::ChannelState(m)) => channelState.push(m.clone()),

            // UserState(UserState { session: Some(165), actor: None, name: Some("kelvinbot"), user_id: None, channel_id: None, mute: None, deaf: None, suppress: None, self_mute: None, self_deaf: None, texture: None, plugin_context: None, plugin_identity: None, comment: None, hash: None, comment_hash: None, texture_hash: None, priority_speaker: None, recording: None, temporary_access_tokens: [], listening_channel_add: [], listening_channel_remove: [], listening_volume_adjustment: [] })
            // UserState(UserState { session: Some(160), actor: None, name: Some("besto_testo"), user_id: None, channel_id: Some(0), mute: None, deaf: None, suppress: None, self_mute: None, self_deaf: None, texture: None, plugin_context: None, plugin_identity: None, comment: None, hash: None, comment_hash: None, texture_hash: None, priority_speaker: None, recording: None, temporary_access_tokens: [], listening_channel_add: [], listening_channel_remove: [], listening_volume_adjustment: [] })
            Some(proto::ServerToClientMessage::UserState(m)) => userState.push(m.clone()),
            Some(proto::ServerToClientMessage::ServerSync(m)) => serverSync = Some(m.clone()),
            Some(_) => {
                log::warn!("Received unexpected message while waiting for setup messages: {:?}", message);
            }
            None => {
                log::error!("Stream ended unexpectedly while waiting for setup messages");
                return Err(Error::other("Stream ended unexpectedly while waiting for setup messages"));
            }
        }
        log::info!("Handshake message: {:?}", message);
    }
    log::info!("Handshake complete!");

    let mut speakers: HashMap<u32, Speaker> = HashMap::new();

    // Match Opus sample rate for output.
    const HEADROOM_MS: u32 = 200;
    const HEADROOM_SAMPLES: u32 = (OUTPUT_SAMPLE_RATE * HEADROOM_MS) / 1000;

    // TODO: we only have 1 ringbuffer for audio, which is fine... but we need to combine all speakers.
    let (mut rbProducer, mut rbConsumer) = HeapRb::<i16>::new(HEADROOM_SAMPLES as usize).split();

    // TODO: allow user to select output device and configure sample rate.
    // TODO: handle errors, not unwrap.
    let host = cpal::default_host();
    let device = host.default_output_device().expect("No output device available");
    let config = device
        .supported_output_configs().unwrap()
        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
        .find_map(|c| c.try_with_sample_rate(48_000))
        .map(|c| c.config())
        .unwrap_or(device.default_output_config().unwrap().config());

    let channels = config.channels as usize;
    let stream = device.build_output_stream(
        config,
        move |output: &mut [f32], _| {
            // same value in left and right
            for frame in output.chunks_mut(channels) {
                let sample = rbConsumer.try_pop().unwrap_or(0) as f32 / i16::MAX as f32;
                frame.fill(sample);
            }
        },
        |err| log::error!("audio stream error: {err}"),
        None,
    ).unwrap();
    stream.play().unwrap();

    // TODO: extract read loop.
    // Mumble requires pings every 30s by default; be eager.
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    // NetEq absorbs network jitter; this only needs to cover timer slop.
    const MIX_TARGET_SAMPLES: usize = (OUTPUT_SAMPLE_RATE as usize / 1000) * 40;
    let mut mix_interval = tokio::time::interval(std::time::Duration::from_millis(5));
    loop {
        // Simultaneously read from the server & manage a ping loop.
        tokio::select! {
            _ = token.cancelled() => {
                log::info!("Breaking; cancellation requested");
                break;
            }
            message = reader.try_next() => {
                match &message? {
                    Some(message) => {
                        // Handle the message here
                        log::info!("Received message from server: {:?}", message);
                        match message {
                            proto::ServerToClientMessage::UdpTunnel(proto::ServerToClientUdpMessage::Audio(m)) => {
                                // TODO: volume adjustment
                                let sender = m.sender_session;

                                let speaker = match speakers.entry(sender) {
                                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                                    std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Speaker::new()?),
                                };
                                if let Err(e) = speaker.insert_packet(sender, &m.opus_data) {
                                    log::warn!("Dropping audio from session {sender}: {e}");
                                }
                            }
                            _ => {
                                // TODO: handle other messages.
                                // log::warn!("Received unknown UDP message from server");
                            }
                        }
                    }
                    None => {
                        // Server disconnected.
                        break;
                    }
                }
            }
            _ = mix_interval.tick() => {
                // Top the ring buffer up to a fixed level rather than mixing once per
                // tick: the card drains it at its own rate, so this follows the card's
                // clock instead of drifting against this timer.
                while rbProducer.occupied_len() < MIX_TARGET_SAMPLES {
                    let frame = mix_frame(&mut speakers)?;
                    rbProducer.push_slice(&frame);
                }
            }
            _ = ping_interval.tick() => {
                log::trace!("Sending ping to server");
                writer.send(proto::ClientToServerMessage::Ping(Default::default())).await?;
            }
        }
    }
    log::info!("Disconnected from server");
    Ok(())
}

const OUTPUT_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size `NetEq::get_audio` produces.
const MIX_FRAME_SAMPLES: usize = OUTPUT_SAMPLE_RATE as usize / 100;

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
        let decoder = opus::Decoder::new(OUTPUT_SAMPLE_RATE, opus::Channels::Mono).map_err(Error::other)?;
        neteq.register_decoder(OPUS_PAYLOAD_TYPE, Box::new(OpusDecoder(decoder)));
        Ok(Self { neteq, next_sequence: 0, next_timestamp: 0 })
    }

    fn insert_packet(&mut self, session: u32, opus_data: &[u8]) -> Result<(), Error> {
        let samples = opus::packet::get_nb_samples(opus_data, OUTPUT_SAMPLE_RATE).map_err(Error::other)?;
        let header = neteq::RtpHeader::new(self.next_sequence, self.next_timestamp, session, OPUS_PAYLOAD_TYPE, false);
        let duration_ms = (samples * 1000 / OUTPUT_SAMPLE_RATE as usize) as u32;
        let packet = neteq::AudioPacket::new(header, opus_data.to_vec(), OUTPUT_SAMPLE_RATE, 1, duration_ms);
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
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Info)
            .level_for("mumbo_lib", log::LevelFilter::Trace)
            .with_colors(ColoredLevelConfig::default())
            .build())
        .invoke_handler(tauri::generate_handler![
            connect_and_listen,
            get_users,
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
        let mut encoder =
            opus::Encoder::new(OUTPUT_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
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
        frame.iter().map(|&s| (s as f32 / i16::MAX as f32).powi(2)).sum()
    }

    #[test]
    fn speaker_audio_comes_out_of_the_mix() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);
        for packet in tone_packets(440.0, 0.5, 25) {
            speakers.get_mut(&7).unwrap().insert_packet(7, &packet).unwrap();
        }

        let total: f32 = (0..50).map(|_| energy(&mix_frame(&mut speakers).unwrap())).sum();

        // A 0.5-amplitude tone has mean power 0.125 per sample; 500 ms of it is
        // ~3000. Require a healthy fraction to allow for NetEq's startup delay.
        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    /// Amplitude of the `frequency` component of `samples` (a single DFT bin).
    fn tone_strength(samples: &[f32], frequency: f32) -> f32 {
        let (re, im) = samples.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &x)| {
            let phase = std::f32::consts::TAU * frequency * n as f32 / OUTPUT_SAMPLE_RATE as f32;
            (re + x * phase.cos(), im - x * phase.sin())
        });
        2.0 * (re * re + im * im).sqrt() / samples.len() as f32
    }

    #[test]
    fn two_speakers_are_heard_together() {
        let mut speakers = HashMap::from([(1, Speaker::new().unwrap()), (2, Speaker::new().unwrap())]);
        // Quiet enough that the sum stays below soft_clip's threshold.
        let alice = tone_packets(440.0, 0.25, 25);
        let bob = tone_packets(1000.0, 0.25, 25);
        for (alice_packet, bob_packet) in alice.iter().zip(&bob) {
            speakers.get_mut(&1).unwrap().insert_packet(1, alice_packet).unwrap();
            speakers.get_mut(&2).unwrap().insert_packet(2, bob_packet).unwrap();
        }

        let mixed: Vec<f32> = (0..50)
            .flat_map(|_| mix_frame(&mut speakers).unwrap())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect();

        let (alice_level, bob_level) = (tone_strength(&mixed, 440.0), tone_strength(&mixed, 1000.0));
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
        assert!(soft_clip(1.0) < 1.0, "a moderate overlap should keep some headroom");
    }

    #[test]
    fn soft_clip_is_smooth_and_increasing() {
        // No jump at the threshold, and louder input never comes out quieter.
        let samples: Vec<f32> = (0..=400).map(|i| i as f32 / 100.0).collect();
        for pair in samples.windows(2) {
            let (a, b) = (soft_clip(pair[0]), soft_clip(pair[1]));
            assert!(b >= a, "soft_clip decreased between {} and {}", pair[0], pair[1]);
            assert!(b - a <= 0.01 + f32::EPSILON, "soft_clip jumped between {} and {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn idle_speaker_mixes_to_silence() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);

        let frame = mix_frame(&mut speakers).unwrap();

        assert_eq!(frame, [0; MIX_FRAME_SAMPLES]);
    }
}
