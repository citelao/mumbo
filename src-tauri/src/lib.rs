use std::io::Error;

use futures_util::StreamExt;
use mumble_protocol::codec::MumbleCodec;
use mumble_protocol as proto;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tauri::{AppHandle, ipc::Channel};
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::net::TcpStream;
use tokio::{self, io::AsyncReadExt, io::AsyncWriteExt};
use tokio_rustls::client::TlsStream;
use tokio_util::codec::FramedRead;
use futures_util::TryStreamExt;

mod AcceptAnyCert;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
async fn connect_disconnect(app: AppHandle, server: String, port: u16, password: String, name: String, channel: Channel<proto::ServerToClientMessage>) -> Result<(), String> {
    let _ = connect(server, port, password, name, channel)
        .await
        .inspect_err(|e| log::error!("{e}"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct RawMumbleConnection {
    // tcp_read: ReadHalf<TlsStream<TcpStream>>,
    reader: FramedRead<ReadHalf<TlsStream<TcpStream>>, proto::codec::MumbleCodec>,
    tcp_write: WriteHalf<TlsStream<TcpStream>>,
}

async fn connect(server: String, port: u16, password: String, name: String, channel: Channel<proto::ServerToClientMessage>) -> Result<(), Error> {
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
                log::trace!("Received message from server: {:?}", message);
                channel.send(message.clone()).unwrap();
            }
        });
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
    write_message(&mut tcp_write, proto::ClientToServerMessage::Version(version)).await?;
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
    write_message(&mut tcp_write, proto::ClientToServerMessage::Authenticate(authenticate)).await?;
    log::info!("Authenticate message sent to server");

    // We need a bunch of setup messages, which should arrive in order, but no
    // need to be precise:
    let mut cryptSetup: Option<proto::raw::CryptSetup> = None;
    let mut codecVersion: Option<proto::raw::CodecVersion> = None;
    let mut channelState: Vec<proto::raw::ChannelState> = vec![];
    let mut userState: Vec<proto::raw::UserState> = vec![];

    while cryptSetup.is_none() || codecVersion.is_none() || channelState.is_empty() || userState.is_empty() {
        let message = reader.try_next().await?;
        match &message {
            Some(proto::ServerToClientMessage::Reject(m)) => {
                log::error!("Server rejected setup message: {} - {}", m.r#type().as_str_name(), m.reason());
                return Err(Error::other(format!("Server rejected setup message: {} - {}", m.r#type().as_str_name(), m.reason())));
            }
            Some(proto::ServerToClientMessage::CryptSetup(m)) => cryptSetup = Some(m.clone()),
            Some(proto::ServerToClientMessage::CodecVersion(m)) => codecVersion = Some(m.clone()),
            Some(proto::ServerToClientMessage::ChannelState(m)) => channelState.push(m.clone()),
            Some(proto::ServerToClientMessage::UserState(m)) => userState.push(m.clone()),
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

    // TODO: extract read loop.
    while let Some(message) = reader
        .try_next()
        .await? {
        log::info!("Received message from server: {:?}", message);
        // channel.send(message).map_err(|e| e.to_string())?;
    }
    log::info!("Disconnected from server");
    Ok(())
}

async fn write_message(tcp_write: &mut WriteHalf<TlsStream<TcpStream>>, message: proto::ClientToServerMessage) -> Result<(), Error> {
    // IDs:
    // https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/protocol_stack_tcp.md
    let (msg_type, payload) = message.encode()?;
    tcp_write.write_u16(msg_type).await?;
    tcp_write.write_u32(payload.len() as u32).await?;
    tcp_write.write_all(&payload).await?;
    Ok(())
}

// async fn read_message(tcp_read: &mut ReadHalf<TlsStream<TcpStream>>) -> Result<proto::ServerToClientMessage, Error> {
//     // TODO: throw a nice error if our connection gets closed because we didn't ping every 30s.
//     let msg_type = tcp_read.read_u16().await?;
//     let len = tcp_read.read_u32().await? as usize;
//     let mut payload = vec![0u8; len];
//     tcp_read.read_exact(&mut payload).await?;

//     let message = proto::ServerToClientMessage::decode(msg_type, &payload[..])?;
//     Ok(message)
// }

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Info)
            .level_for("mumbo_lib", log::LevelFilter::Trace)
            .with_colors(ColoredLevelConfig::default())
            .build())
        .invoke_handler(tauri::generate_handler![connect_disconnect])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
