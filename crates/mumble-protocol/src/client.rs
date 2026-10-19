use std::io;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
use tokio_util::{codec::{FramedRead, FramedWrite}, sync::CancellationToken};
use tokio::{self, io::AsyncReadExt, io::AsyncWriteExt};
use std::sync::{Arc, Mutex};
use futures_util::{Sink, SinkExt, Stream, StreamExt, TryFutureExt, TryStreamExt};

mod accept_any_cert;
use accept_any_cert::AcceptAnyCert;

use crate::{ServerToClientMessage, ServerToClientUdpMessage, state::{ConnectionStatus, MumbleState}};

pub struct MumbleClient {
    // TODO: Exposed via helper methods?
    state: Arc<Mutex<MumbleState>>,
    cancel: CancellationToken,
}

pub struct Connection {
    pub client: MumbleClient,

    // All incoming events! For debug.
    pub debug: broadcast::Receiver<ServerToClientMessage>,

    // Audio packets, ready for decoding.
    pub audio: mpsc::Receiver<crate::raw_udp::Audio>,

    // TODO: expose useful events, like connection etc.
}

#[derive(Debug, Error)]
pub enum MumbleConnectionError {
    #[error("Invalid DNS name: {0}")]
    InvalidDnsError(#[from] rustls::pki_types::InvalidDnsNameError),

    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("TLS error: {0}")]
    TlsError(#[from] rustls::Error),

    #[error("Unexpected server close")]
    UnexpectedServerClose,
}

pub struct ConnectionOptions {
    // URL to Server, e.g. "mywebsite.chat"
    pub server: String,

    // Port to connect to; defaults to 64738
    pub port: Option<u16>,

    // Server password, if required
    pub password: Option<String>,

    // Username to use
    pub username: Option<String>,
}

pub async fn connect(options: ConnectionOptions) -> Result<Connection, MumbleConnectionError> {
    // Connection process:
    //
    // https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/establishing_connection.md
    let server = &options.server;
    let port = options.port.unwrap_or(64738);
    log::info!("Connecting to server {} on port {}", server, port);

    let server_name = rustls::pki_types::ServerName::try_from(server.clone())?;
    let tcp = tokio::net::TcpStream::connect((server.clone(), port)).await?;

    // TODO: we accept any cert because many servers self-sign. We should figure
    // out how to handle this better.
    let tls = accept_any_cert::make_connector()?
        .connect(server_name, tcp)
        .await?;

    const DEBUG_CHANNEL_CAPACITY: usize = 512;
    let (debug_tx, debug_rx) = broadcast::channel::<ServerToClientMessage>(DEBUG_CHANNEL_CAPACITY);

    let (mut tcp_read, mut tcp_write) = tokio::io::split(tls);
    let mut reader = FramedRead::new(tcp_read, crate::codec::MumbleCodec::default())
        .inspect(move |msg| {
            if let Ok(msg) = msg {
                log::debug!("Received server message: {:?}", msg);
                if debug_tx.receiver_count() > 0 {
                    let _ = debug_tx.send(msg.clone());
                }
            }
        });
    let mut writer = FramedWrite::new(tcp_write, crate::codec::MumbleCodec::default());

    // Send a version message!
    let (major, minor, patch) = (1u32, 5u32, 0u32);
    let our_version: u32 = (major << 16) | (minor << 8) | patch;
    let our_version_v2: u64 = (u64::from(major) << 48) | (u64::from(minor) << 32) | (u64::from(patch) << 16);

    let version = crate::raw::Version {
        // not necessary, but nice
        os: Some(std::env::consts::OS.into()),
        release: Some("mumbo 0.1.0".into()),
        os_version: None,

        // required
        version_v1: Some(our_version),
        version_v2: Some(our_version_v2),
    };
    writer.send(crate::ClientToServerMessage::Version(version)).await?;
    log::info!("Version message sent to server");

    // Wait for the server's version message.
    //
    // ```
    // [2026-10-15][01:44:44][INFO][mumbo_lib] Received version message: Version
    // { version_v1: Some(67071), version_v2: Some(281496510595072), release:
    // Some("1.5.901"), os: Some("Linux"), os_version: Some("Ubuntu 24.04.4 LTS
    // [arm64]") }
    // ```
    let Some(crate::ServerToClientMessage::Version(version)) = reader
        .try_next()
        .await? else {
        return Err(MumbleConnectionError::UnexpectedServerClose);
    };
    log::info!("Received version message: {:?}", version);

    let authenticate = crate::raw::Authenticate {
        username: options.username,
        password: options.password,

        // We don't support authentication tokens for now
        tokens: vec![],

        // Outdated params
        celt_versions: vec![],
        opus: Some(true),
        client_type: Some(0),
    };
    writer.send(crate::ClientToServerMessage::Authenticate(authenticate)).await?;
    log::info!("Authenticate message sent to server");

    let state = Arc::new(Mutex::new(MumbleState::default()));

    // Assume 20ms packets, let's buffer ~10 seconds worth of audio.
    const AUDIO_CHANNEL_BUFFER_SIZE: usize = 512;
    assert!(AUDIO_CHANNEL_BUFFER_SIZE > (10 * 1000 / 20));
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(AUDIO_CHANNEL_BUFFER_SIZE);

    // Let's wait for the handshake to complete.
    while state.lock().unwrap().connection_status() == &ConnectionStatus::Connecting {
        let Some(message) = reader.try_next().await? else {
            return Err(MumbleConnectionError::UnexpectedServerClose);
        };
        state.lock().unwrap().apply(message)?;
    }

    let cancellation_token = CancellationToken::new();
    tokio::spawn(run(reader, writer, state.clone(), audio_tx, cancellation_token.clone())
        .inspect_err(|e| log::error!("connection ended: {e}")));

    Ok(Connection {
        client: MumbleClient {
            state: state,
            cancel: cancellation_token,
        },
        debug: debug_rx,
        audio: audio_rx,
    })
}

async fn run(
    mut reader: impl Stream<Item = Result<ServerToClientMessage, io::Error>> + Unpin + Send + 'static,
    mut writer: impl Sink<crate::ClientToServerMessage, Error = io::Error> + Unpin + Send + 'static,
    state: Arc<Mutex<MumbleState>>,
    audio_tx: tokio::sync::mpsc::Sender<crate::raw_udp::Audio>,
    cancellation_token: CancellationToken) -> Result<(), MumbleConnectionError> {

    // Mumble requires pings every 30s by default; be eager.
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));

    log::info!("Starting client run loop");
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                log::info!("Cancellation requested, shutting down client loop");
                break;
            }
            message = reader.try_next() => {
                let Some(message) = message? else { break }; // None -> server closed

                // Audio messages are handled outside of MumbleState
                match message {
                    ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Audio(m)) => {
                        if let Err(e) = audio_tx.try_send(m) {
                            log::error!("Dropping voice packet: {e}");
                        }
                    }
                    _ => {
                        state.lock().unwrap().apply(message)?;
                    }
                }
            }
            _ = ping_interval.tick() => {
                // TODO: include timestamp in Ping.
                log::trace!("Sending ping to server");
                writer.send(crate::ClientToServerMessage::Ping(Default::default())).await?;
            }
        }
    }
    log::info!("Disconnected from server");
    Ok(())
}

impl MumbleClient {
    pub fn disconnect(&self) {
        self.cancel.cancel();
    }

    pub fn state(&self) -> Arc<Mutex<MumbleState>> {
        self.state.clone()
    }
}