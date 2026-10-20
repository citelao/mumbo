use std::io;
use bytes::Buf;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
use tokio_util::{codec::{FramedRead, FramedWrite}, sync::CancellationToken};
use tokio::io::{AsyncRead, AsyncWrite};
use std::sync::{Arc, Mutex};
use futures_util::{Sink, SinkExt, Stream, StreamExt, TryFutureExt, TryStreamExt};

mod accept_any_cert;

use crate::{ServerToClientMessage, ServerToClientUdpMessage, state::{ConnectionStatus, MumbleState}};

#[derive(Clone)]
pub struct MumbleClient {
    // TODO: Exposed via helper methods?
    state: Arc<Mutex<MumbleState>>,
    cancel: CancellationToken,

    outgoing: mpsc::UnboundedSender<crate::ClientToServerMessage>,
}

pub struct Connection {
    pub client: MumbleClient,

    // All incoming events! For debug.
    pub debug: broadcast::Receiver<ServerToClientMessage>,

    // Audio packets, ready for decoding.
    pub audio: mpsc::Receiver<crate::raw_udp::Audio>,

    // General server events, like users joining, leaving, & updating.
    pub events: mpsc::UnboundedReceiver<crate::state::MumbleEvent>,
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

    #[error("Disconnected from server")]
    Disconnected,
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

    connect_over(tls, options).await
}

/// Runs the Mumble handshake and starts the connection over an already-open
/// stream: TLS in production, an in-memory pipe in tests.
async fn connect_over(
    stream: impl AsyncRead + AsyncWrite + Send + 'static,
    options: ConnectionOptions,
) -> Result<Connection, MumbleConnectionError> {
    const DEBUG_CHANNEL_CAPACITY: usize = 512;
    let (debug_tx, debug_rx) = broadcast::channel::<ServerToClientMessage>(DEBUG_CHANNEL_CAPACITY);

    let (stream_read, stream_write) = tokio::io::split(stream);
    let mut reader = FramedRead::new(stream_read, crate::codec::MumbleCodec::default())
        .inspect(move |msg| {
            if let Ok(msg) = msg {
                // Audio messages are noisy.
                if let ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Audio(_)) = msg {
                    log::trace!("Received audio message: {:?}", msg);
                } else {
                    log::debug!("Received server message: {:?}", msg);
                }
                if debug_tx.receiver_count() > 0 {
                    let _ = debug_tx.send(msg.clone());
                }
            }
        });
    let mut writer = FramedWrite::new(stream_write, crate::codec::MumbleCodec::default());

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

    // Unbounded: an event must never be dropped (a lost UserRemove leaves a
    // ghost user), and the driver must never wait on the app (that would stall
    // pings). Events are infrequent, so the queue stays small.
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    // Assume 20ms packets, let's buffer ~10 seconds worth of audio.
    const AUDIO_CHANNEL_BUFFER_SIZE: usize = 512;
    // Checked at compile time.
    const _: () = assert!(AUDIO_CHANNEL_BUFFER_SIZE > (10 * 1000 / 20));
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(AUDIO_CHANNEL_BUFFER_SIZE);

    // Let's wait for the handshake to complete.
    while state.lock().unwrap().connection_status() == &ConnectionStatus::Connecting {
        let Some(message) = reader.try_next().await? else {
            return Err(MumbleConnectionError::UnexpectedServerClose);
        };
        apply_and_publish(&state, &events_tx, message)?;
    }

    let (server_tx, server_rx) = mpsc::unbounded_channel::<crate::ClientToServerMessage>();

    let cancellation_token = CancellationToken::new();
    tokio::spawn(run(reader, writer, state.clone(), audio_tx, events_tx, server_rx, cancellation_token.clone())
        .inspect_err(|e| log::error!("connection ended: {e}")));

    Ok(Connection {
        client: MumbleClient {
            state,
            cancel: cancellation_token,
            outgoing: server_tx,
        },
        events: events_rx,
        debug: debug_rx,
        audio: audio_rx,
    })
}

/// Applies a message to the state and publishes any resulting event.
fn apply_and_publish(
    state: &Mutex<MumbleState>,
    events: &mpsc::UnboundedSender<crate::state::MumbleEvent>,
    message: ServerToClientMessage,
) -> Result<(), MumbleConnectionError> {
    let event = state.lock().unwrap().apply(message)?;
    if let Some(event) = event {
        // Only fails if the app dropped its receiver, i.e. nobody is listening.
        let _ = events.send(event);
    }
    Ok(())
}

async fn run(
    mut reader: impl Stream<Item = Result<ServerToClientMessage, io::Error>> + Unpin + Send + 'static,
    mut writer: impl Sink<crate::ClientToServerMessage, Error = io::Error> + Unpin + Send + 'static,
    state: Arc<Mutex<MumbleState>>,
    audio_tx: tokio::sync::mpsc::Sender<crate::raw_udp::Audio>,
    events_tx: mpsc::UnboundedSender<crate::state::MumbleEvent>,
    mut server_rx: mpsc::UnboundedReceiver<crate::ClientToServerMessage>,
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
                        apply_and_publish(&state, &events_tx, message)?;
                    }
                }
            }
            message = server_rx.recv() => {
                if let Some(message) = message {
                    log::trace!("Sending message to server: {:?}", message);
                    writer.send(message.clone()).await?;

                    // Handle any bookkeeping needed for specific messages.
                    if let crate::ClientToServerMessage::TextMessage(m) = &message {
                        // Add the message to the local chat history.
                        let mut state = state.lock().unwrap();
                        let user_id = state.current_user_session;
                        state.chat_messages.push(crate::state::TextMessage::ChatTextMessage(crate::state::ChatTextMessage {
                            timestamp: chrono::Utc::now(),
                            user_session: user_id,
                            content: m.message.clone(),
                        }));
                        events_tx.send(crate::state::MumbleEvent::ChatMessage).unwrap();
                    }
                } else {
                    log::info!("Server channel closed, disconnecting");
                    break; // Channel closed, disconnect
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

    pub fn send_audio(&self, audio: &[u8], frame_counter: u64) -> Result<(), MumbleConnectionError> {
        let packet = crate::ClientToServerMessage::UdpTunnel(
            crate::ClientToServerUdpMessage::Audio(
            crate::raw_udp::Audio {
                opus_data: audio.to_vec(),

                // Normal speech
                // header: Some(crate::raw_udp::audio::Header::Target(0)),

                // Server loop-back
                header: Some(crate::raw_udp::audio::Header::Target(31)),
                frame_number: frame_counter,

                ..Default::default()
            }));
        self.send(packet)
    }

    /// Sends a text message to the specified channel.
    ///
    /// Defaults to the current user's channel.
    pub fn send_channel_message(&self, message: String, channel_id: Option<u32>) -> Result<(), MumbleConnectionError> {
        let channel_id = if let Some(channel_id) = channel_id {
            channel_id
        } else {
            self.state.lock().unwrap().current_user_channel().unwrap()
        };
        log::trace!("Sending text message: {} to channel: {:?}", message, channel_id);
        self.send(crate::ClientToServerMessage::TextMessage(crate::raw::TextMessage {
            message: message.clone(),
            channel_id: vec![channel_id],
            ..Default::default()
        }))?;
        Ok(())
    }

    fn send(&self, message: crate::ClientToServerMessage) -> Result<(), MumbleConnectionError> {
        self.outgoing.send(message).map_err(|_| MumbleConnectionError::Disconnected)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{MumbleFrame, MumbleFrameCodec};
    use crate::{TcpMessageId, UdpMessageId, raw, raw_udp};
    use prost::Message;
    use std::time::Duration;
    use tokio::io::DuplexStream;
    use tokio_util::codec::Framed;

    /// Generous bound so a broken test fails instead of hanging.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// The server end of an in-memory connection, speaking raw frames.
    struct FakeServer {
        frames: Framed<DuplexStream, MumbleFrameCodec>,
    }

    impl FakeServer {
        async fn send<M: TcpMessageId>(&mut self, message: M) {
            let frame = MumbleFrame { type_id: M::TYPE_ID, payload: message.encode_to_vec().into() };
            self.frames.send(frame).await.unwrap();
        }

        async fn send_audio(&mut self, audio: raw_udp::Audio) {
            // Tunnelled voice: one UDP type byte, then the protobuf.
            let mut payload = vec![raw_udp::Audio::TYPE_ID];
            payload.extend(audio.encode_to_vec());
            self.frames.send(MumbleFrame { type_id: raw::UdpTunnel::TYPE_ID, payload: payload.into() }).await.unwrap();
        }

        /// The next frame the client sent, or `None` once it hung up.
        async fn receive(&mut self) -> Option<MumbleFrame> {
            tokio::time::timeout(TIMEOUT, self.frames.next()).await.unwrap().map(Result::unwrap)
        }

        async fn receive_message<M: TcpMessageId>(&mut self) -> M {
            let frame = self.receive().await.expect("client hung up");
            assert_eq!(frame.type_id, M::TYPE_ID, "unexpected message type");
            M::decode(frame.payload).unwrap()
        }

        /// Reads the client's opening Version and Authenticate, and answers with Version.
        async fn greet(&mut self) -> raw::Authenticate {
            self.receive_message::<raw::Version>().await;
            self.send(raw::Version { version_v2: Some(1 << 48 | 5 << 32), ..Default::default() }).await;
            self.receive_message::<raw::Authenticate>().await
        }

        /// Completes the handshake with one other user already on the server.
        async fn accept(&mut self) {
            self.greet().await;
            self.send(user(1, "Alice")).await;
            self.send(user(2, "me")).await;
            self.send(raw::ServerSync { session: Some(2), ..Default::default() }).await;
        }
    }

    fn user(session: u32, name: &str) -> raw::UserState {
        raw::UserState { session: Some(session), name: Some(name.to_string()), ..Default::default() }
    }

    fn options() -> ConnectionOptions {
        ConnectionOptions {
            server: "test.invalid".to_string(),
            port: None,
            password: Some("hunter2".to_string()),
            username: Some("me".to_string()),
        }
    }

    /// Starts a client handshake against a fake server.
    fn start() -> (tokio::task::JoinHandle<Result<Connection, MumbleConnectionError>>, FakeServer) {
        let (client_end, server_end) = tokio::io::duplex(64 * 1024);
        let connecting = tokio::spawn(connect_over(client_end, options()));
        (connecting, FakeServer { frames: Framed::new(server_end, MumbleFrameCodec) })
    }

    async fn connected() -> (Connection, FakeServer) {
        let (connecting, mut server) = start();
        server.accept().await;
        let connection = tokio::time::timeout(TIMEOUT, connecting).await.unwrap().unwrap().unwrap();
        (connection, server)
    }

    fn user_names(client: &MumbleClient) -> Vec<String> {
        client.state().lock().unwrap().users().iter().map(|u| u.name().to_string()).collect()
    }

    #[tokio::test]
    async fn handshake_sends_version_then_credentials() {
        let (_connecting, mut server) = start();

        let version = server.receive_message::<raw::Version>().await;
        server.send(raw::Version::default()).await;
        let authenticate = server.receive_message::<raw::Authenticate>().await;

        assert_eq!(version.version_v2, Some(1 << 48 | 5 << 32), "should claim 1.5.0");
        assert_eq!(authenticate.username.as_deref(), Some("me"));
        assert_eq!(authenticate.password.as_deref(), Some("hunter2"));
        assert_eq!(authenticate.opus, Some(true), "must advertise Opus or the server may fall back to CELT");
    }

    #[tokio::test]
    async fn connect_keeps_every_user_sent_before_server_sync() {
        let (connection, _server) = connected().await;

        assert_eq!(user_names(&connection.client), ["Alice", "me"]);
    }

    #[tokio::test]
    async fn connect_waits_for_server_sync() {
        let (mut connecting, mut server) = start();
        server.greet().await;
        server.send(user(1, "Alice")).await;

        // Everything but ServerSync has arrived: still connecting.
        let still_waiting = tokio::time::timeout(Duration::from_millis(50), &mut connecting).await;
        assert!(still_waiting.is_err(), "connect returned before ServerSync");

        server.send(raw::ServerSync { session: Some(2), ..Default::default() }).await;
        tokio::time::timeout(TIMEOUT, connecting).await.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    async fn connect_fails_when_the_server_rejects() {
        let (connecting, mut server) = start();
        server.greet().await;
        server.send(raw::Reject {
            r#type: Some(raw::reject::RejectType::WrongServerPw as i32),
            reason: Some("Invalid server password".to_string()),
        }).await;

        let error = tokio::time::timeout(TIMEOUT, connecting).await.unwrap().unwrap().err().expect("connect should fail");

        assert!(error.to_string().contains("Invalid server password"), "{error}");
    }

    #[tokio::test]
    async fn connect_fails_when_the_server_hangs_up_mid_handshake() {
        let (connecting, mut server) = start();
        server.greet().await;
        drop(server);

        let error = tokio::time::timeout(TIMEOUT, connecting).await.unwrap().unwrap().err().expect("connect should fail");

        assert!(matches!(error, MumbleConnectionError::UnexpectedServerClose), "{error}");
    }

    #[tokio::test]
    async fn audio_goes_to_the_audio_channel_not_the_state() {
        let (mut connection, mut server) = connected().await;

        server.send_audio(raw_udp::Audio { sender_session: 1, opus_data: vec![1, 2, 3], ..Default::default() }).await;
        let audio = tokio::time::timeout(TIMEOUT, connection.audio.recv()).await.unwrap().unwrap();

        assert_eq!((audio.sender_session, audio.opus_data), (1, vec![1, 2, 3]));
        assert_eq!(user_names(&connection.client), ["Alice", "me"]);
    }

    #[tokio::test]
    async fn debug_stream_includes_the_handshake() {
        let (mut connection, _server) = connected().await;

        let mut received = Vec::new();
        while let Ok(message) = connection.debug.try_recv() {
            received.push(message);
        }

        assert!(matches!(received[0], ServerToClientMessage::Version(_)), "{received:?}");
        assert!(matches!(received.last(), Some(ServerToClientMessage::ServerSync(_))), "{received:?}");
        assert_eq!(received.len(), 4, "Version, two UserStates, ServerSync: {received:?}");
    }

    #[tokio::test]
    async fn events_report_the_handshake_and_later_changes() {
        let (mut connection, mut server) = connected().await;

        // Both users from the handshake were reported before connect returned…
        let mut handshake = Vec::new();
        while let Ok(event) = connection.events.try_recv() {
            handshake.push(event);
        }
        assert!(
            matches!(handshake[..], [crate::state::MumbleEvent::UsersChanged, crate::state::MumbleEvent::UsersChanged]),
            "{handshake:?}"
        );

        // …and changes keep being reported afterwards.
        server.send(raw::TextMessage { message: "hi".to_string(), ..Default::default() }).await;
        let event = tokio::time::timeout(TIMEOUT, connection.events.recv()).await.unwrap();
        assert!(matches!(event, Some(crate::state::MumbleEvent::ChatMessage)), "{event:?}");
    }

    #[tokio::test]
    async fn state_keeps_updating_after_connect() {
        let (connection, mut server) = connected().await;

        server.send(user(3, "Bob")).await;

        wait_until(|| user_names(&connection.client).contains(&"Bob".to_string())).await;
    }

    #[tokio::test]
    async fn disconnect_closes_the_connection() {
        let (mut connection, mut server) = connected().await;

        connection.client.disconnect();

        // The server sees the socket close (after any pings already in flight)…
        while server.receive().await.is_some() {}
        // …and the audio channel closes, which is how the app's audio task stops.
        assert!(tokio::time::timeout(TIMEOUT, connection.audio.recv()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn server_hanging_up_closes_the_audio_channel() {
        let (mut connection, server) = connected().await;

        drop(server);

        assert!(tokio::time::timeout(TIMEOUT, connection.audio.recv()).await.unwrap().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn pings_every_15_seconds_to_stay_connected() {
        let (_connection, mut server) = connected().await;

        // The first ping goes out as soon as the loop starts.
        assert_eq!(server.receive().await.unwrap().type_id, raw::Ping::TYPE_ID);

        tokio::time::advance(Duration::from_secs(15)).await;
        assert_eq!(server.receive().await.unwrap().type_id, raw::Ping::TYPE_ID);
    }

    #[tokio::test]
    async fn a_full_audio_channel_does_not_stall_the_connection() {
        let (connection, mut server) = connected().await;

        // Nobody reads `connection.audio`; overflow it, then send a state update.
        for _ in 0..600 {
            server.send_audio(raw_udp::Audio { sender_session: 1, ..Default::default() }).await;
        }
        server.send(user(3, "Bob")).await;

        wait_until(|| user_names(&connection.client).contains(&"Bob".to_string())).await;
    }

    async fn wait_until(condition: impl Fn() -> bool) {
        tokio::time::timeout(TIMEOUT, async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("condition never became true");
    }
}
