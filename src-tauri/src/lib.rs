use std::io::Error;

use mumble_protocol::mumble_proto as proto;
use mumble_protocol::Message;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::net::TcpStream;
use tokio::{self, io::AsyncReadExt, io::AsyncWriteExt};
use tokio_rustls::client::TlsStream;
use mumble_protocol::mumble_proto::TcpMessageId;

mod AcceptAnyCert;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
async fn connect_disconnect(server: String, port: u16, password: String, name: String) -> Result<(), String> {
    connect(server, port, password, name).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn connect(server: String, port: u16, password: String, name: String) -> Result<(), Error> {
    log::info!("Connecting to server {} on port {}", server, port);
    let server_name = rustls::pki_types::ServerName::try_from(server.clone()).map_err(Error::other)?;
    let tcp = tokio::net::TcpStream::connect((server.clone(), port)).await?;
    let tls = AcceptAnyCert::make_connector().map_err(Error::other)?
        .connect(server_name, tcp)
        .await?;
    let (mut tcp_read, mut tcp_write) = tokio::io::split(tls);
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
    let version_message = proto::ClientToServerMessage::Version(version);
    let send_promise = write_message(&mut tcp_write, version_message);

    // Wait for the server's version message.
    //
    // ```
    // [2026-10-15][01:44:44][INFO][mumbo_lib] Received version message: Version
    // { version_v1: Some(67071), version_v2: Some(281496510595072), release:
    // Some("1.5.901"), os: Some("Linux"), os_version: Some("Ubuntu 24.04.4 LTS
    // [arm64]") }
    // ```
    let proto::ServerToClientMessage::Version(version) = read_message(&mut tcp_read).await? else {
        return Err(Error::other("Expected version message from server"))
    };
    log::info!("Received version message: {:?}", version);

    send_promise.await?;
    log::info!("Version message sent to server");

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

    let response = read_message(&mut tcp_read).await?;
    match response {
        proto::ServerToClientMessage::Reject(reject) => {
            log::error!("Received reject message from server: {:?}", reject);
            return Err(Error::other(format!("Server rejected authentication: {} - {}", reject.r#type().as_str_name(), reject.reason())));
        }
        proto::ServerToClientMessage::CryptSetup(crypt_setup) => {
            log::info!("Received crypt setup message from server: {:?}", crypt_setup);
        }
        _ => {
            log::warn!("Received unexpected message from server: {:?}", response);
        }
    }

    loop {
        let message = read_message(&mut tcp_read).await?;
        log::info!("Received message from server: {:?}", message);
    }
    // Ok(())
}

async fn write_message(tcp_write: &mut WriteHalf<TlsStream<TcpStream>>, message: proto::ClientToServerMessage) -> Result<(), Error> {
    // IDs:
    // https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/protocol_stack_tcp.md
    let (msg_type, payload) = match message {
        proto::ClientToServerMessage::Version(version) => (proto::raw::Version::TYPE_ID, proto::raw::Version::encode_to_vec(&version)),
        proto::ClientToServerMessage::Authenticate(a) => (proto::raw::Authenticate::TYPE_ID, proto::raw::Authenticate::encode_to_vec(&a)),
    };
    tcp_write.write_u16(msg_type).await?;
    tcp_write.write_u32(payload.len() as u32).await?;
    tcp_write.write_all(&payload).await?;
    Ok(())
}

async fn read_message(tcp_read: &mut ReadHalf<TlsStream<TcpStream>>) -> Result<proto::ServerToClientMessage, Error> {
    let msg_type = tcp_read.read_u16().await?;
    let len = tcp_read.read_u32().await? as usize;
    let mut payload = vec![0u8; len];
    tcp_read.read_exact(&mut payload).await?;

    let message = match msg_type {
        proto::raw::Version::TYPE_ID => proto::ServerToClientMessage::Version(proto::raw::Version::decode(&payload[..])?),
        proto::raw::Reject::TYPE_ID => proto::ServerToClientMessage::Reject(proto::raw::Reject::decode(&payload[..])?),
        proto::raw::CryptSetup::TYPE_ID => proto::ServerToClientMessage::CryptSetup(proto::raw::CryptSetup::decode(&payload[..])?),
        proto::raw::CodecVersion::TYPE_ID => proto::ServerToClientMessage::CodecVersion(proto::raw::CodecVersion::decode(&payload[..])?),
        proto::raw::ChannelState::TYPE_ID => proto::ServerToClientMessage::ChannelState(proto::raw::ChannelState::decode(&payload[..])?),
        _ => {
            log::warn!("Unknown message type: {} (size: {})", msg_type, len);
            return Err(Error::new(std::io::ErrorKind::InvalidData, format!("Unknown message type: {} (size: {})", msg_type, len)));
        }
    };
    Ok(message)
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
        .invoke_handler(tauri::generate_handler![greet, connect_disconnect])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
