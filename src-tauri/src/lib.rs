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
use ringbuf::traits::{Consumer, Producer, Split};

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

    let mut decoders: HashMap<u32, opus::Decoder> = HashMap::new();

    // Match Opus sample rate for output.
    const OUTPUT_SAMPLE_RATE: u32 = 48000;
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

                                let decoder = decoders
                                    .entry(sender)
                                    .or_insert_with(|| opus::Decoder::new(OUTPUT_SAMPLE_RATE, opus::Channels::Mono).unwrap());

                                let mut pcm = [0i16; 5760]; // 120 ms at 48 kHz, Opus's largest frame
                                let decoded_sample_count = decoder.decode(&m.opus_data, &mut pcm, false).unwrap();
                                log::info!("Decoded {} samples from sender {}", decoded_sample_count, sender);
                                rbProducer.push_slice(&pcm[..decoded_sample_count]);

                                log::info!("Received UDP message from server");
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
            _ = ping_interval.tick() => {
                log::trace!("Sending ping to server");
                writer.send(proto::ClientToServerMessage::Ping(Default::default())).await?;
            }
        }
    }
    log::info!("Disconnected from server");
    Ok(())
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
