pub use prost::Message;
use std::io::Error;

pub mod raw {
    include!(concat!(env!("OUT_DIR"), "/mumble_proto.rs"));
}

pub mod raw_udp {
    include!(concat!(env!("OUT_DIR"), "/mumble_udp.rs"));
}

pub mod codec;

pub trait TcpMessageId: prost::Message + Default {
    const TYPE_ID: u16;
}

// https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/protocol_stack_tcp.md
impl TcpMessageId for raw::Version { const TYPE_ID: u16 = 0; }
impl TcpMessageId for raw::UdpTunnel { const TYPE_ID: u16 = 1; }
impl TcpMessageId for raw::Authenticate { const TYPE_ID: u16 = 2; }
impl TcpMessageId for raw::Ping { const TYPE_ID: u16 = 3; }
impl TcpMessageId for raw::Reject { const TYPE_ID: u16 = 4; }
impl TcpMessageId for raw::ServerSync { const TYPE_ID: u16 = 5; }
impl TcpMessageId for raw::ChannelRemove { const TYPE_ID: u16 = 6; }
impl TcpMessageId for raw::ChannelState { const TYPE_ID: u16 = 7; }
impl TcpMessageId for raw::UserRemove { const TYPE_ID: u16 = 8; }
impl TcpMessageId for raw::UserState { const TYPE_ID: u16 = 9; }
impl TcpMessageId for raw::BanList { const TYPE_ID: u16 = 10; }
impl TcpMessageId for raw::TextMessage { const TYPE_ID: u16 = 11; }
impl TcpMessageId for raw::PermissionDenied { const TYPE_ID: u16 = 12; }
impl TcpMessageId for raw::Acl { const TYPE_ID: u16 = 13; }
impl TcpMessageId for raw::QueryUsers { const TYPE_ID: u16 = 14; }
impl TcpMessageId for raw::CryptSetup { const TYPE_ID: u16 = 15; }

impl TcpMessageId for raw::ContextActionModify { const TYPE_ID: u16 = 16; }
impl TcpMessageId for raw::ContextAction { const TYPE_ID: u16 = 17; }
impl TcpMessageId for raw::UserList { const TYPE_ID: u16 = 18; }
impl TcpMessageId for raw::VoiceTarget { const TYPE_ID: u16 = 19; }
impl TcpMessageId for raw::PermissionQuery { const TYPE_ID: u16 = 20; }
impl TcpMessageId for raw::CodecVersion { const TYPE_ID: u16 = 21; }
impl TcpMessageId for raw::UserStats { const TYPE_ID: u16 = 22; }
impl TcpMessageId for raw::RequestBlob { const TYPE_ID: u16 = 23; }
impl TcpMessageId for raw::ServerConfig { const TYPE_ID: u16 = 24; }
impl TcpMessageId for raw::SuggestConfig { const TYPE_ID: u16 = 25; }
impl TcpMessageId for raw::PluginDataTransmission { const TYPE_ID: u16 = 26; }

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerToClientUdpMessage {
    Audio(raw_udp::Audio),
    Ping(raw_udp::Ping),
}

pub trait UdpMessageId: prost::Message + Default {
    const TYPE_ID: u8;
}

impl UdpMessageId for raw_udp::Audio { const TYPE_ID: u8 = 0; }
impl UdpMessageId for raw_udp::Ping { const TYPE_ID: u8 = 1; }

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerToClientMessage {
    Version(raw::Version),
    UdpTunnel(ServerToClientUdpMessage),
    Reject(raw::Reject),
    ServerSync(raw::ServerSync),
    ChannelRemove(raw::ChannelRemove),
    ChannelState(raw::ChannelState),
    UserRemove(raw::UserRemove),
    UserState(raw::UserState),
    CryptSetup(raw::CryptSetup),
    PermissionQuery(raw::PermissionQuery),
    CodecVersion(raw::CodecVersion),
    ServerConfig(raw::ServerConfig),
}

impl ServerToClientMessage {
    pub fn decode(msg_type: u16, payload: &[u8]) -> Result<ServerToClientMessage, Error> {
        let message = match msg_type {
            raw::Version::TYPE_ID => ServerToClientMessage::Version(raw::Version::decode(&payload[..])?),
            raw::UdpTunnel::TYPE_ID => {
                let (&udp_type, rest) = payload.split_first().ok_or_else(|| Error::new(std::io::ErrorKind::InvalidData, "Empty UDP payload"))?;
                match udp_type {
                    raw_udp::Audio::TYPE_ID => ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Audio(raw_udp::Audio::decode(&rest[..])?)),
                    raw_udp::Ping::TYPE_ID => ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Ping(raw_udp::Ping::decode(&rest[..])?)),
                    _ => return Err(Error::new(std::io::ErrorKind::InvalidData, format!("Unknown UDP message type: {}", udp_type))),
                }
            },
            raw::Reject::TYPE_ID => ServerToClientMessage::Reject(raw::Reject::decode(&payload[..])?),
            raw::ChannelRemove::TYPE_ID => ServerToClientMessage::ChannelRemove(raw::ChannelRemove::decode(&payload[..])?),
            raw::ChannelState::TYPE_ID => ServerToClientMessage::ChannelState(raw::ChannelState::decode(&payload[..])?),
            raw::ServerSync::TYPE_ID => ServerToClientMessage::ServerSync(raw::ServerSync::decode(&payload[..])?),
            raw::UserRemove::TYPE_ID => ServerToClientMessage::UserRemove(raw::UserRemove::decode(&payload[..])?),
            raw::UserState::TYPE_ID => ServerToClientMessage::UserState(raw::UserState::decode(&payload[..])?),
            raw::CryptSetup::TYPE_ID => ServerToClientMessage::CryptSetup(raw::CryptSetup::decode(&payload[..])?),
            raw::PermissionQuery::TYPE_ID => ServerToClientMessage::PermissionQuery(raw::PermissionQuery::decode(&payload[..])?),
            raw::CodecVersion::TYPE_ID => ServerToClientMessage::CodecVersion(raw::CodecVersion::decode(&payload[..])?),
            raw::ServerConfig::TYPE_ID => ServerToClientMessage::ServerConfig(raw::ServerConfig::decode(&payload[..])?),
            _ => {
                return Err(Error::new(std::io::ErrorKind::InvalidData, format!("Unknown message type: {} (size: {})", msg_type, payload.len())));
            }
        };
        Ok(message)
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientToServerMessage {
    Version(raw::Version),
    Authenticate(raw::Authenticate),
    Ping(raw::Ping),
}

impl ClientToServerMessage {
    pub fn encode(&self) -> Result<(u16, Vec<u8>), Error> {
        match self {
            ClientToServerMessage::Version(msg) => Ok((raw::Version::TYPE_ID, msg.encode_to_vec())),
            ClientToServerMessage::Authenticate(msg) => Ok((raw::Authenticate::TYPE_ID, msg.encode_to_vec())),
            ClientToServerMessage::Ping(msg) => Ok((raw::Ping::TYPE_ID, msg.encode_to_vec())),
        }
    }
}

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }

    // One sample of every variant. src/protocol.test.ts parses the same fixture
    // with the zod schema, so drift on either side fails a test.
    #[test]
    fn server_messages_match_shared_fixture() {
        let samples = vec![
            ServerToClientMessage::Version(Default::default()),
            ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Audio(raw_udp::Audio {
                header: Some(raw_udp::audio::Header::Context(2)),
                opus_data: vec![1, 2],
                ..Default::default()
            })),
            ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Ping(Default::default())),
            ServerToClientMessage::Reject(Default::default()),
            ServerToClientMessage::ServerSync(Default::default()),
            ServerToClientMessage::ChannelRemove(Default::default()),
            ServerToClientMessage::ChannelState(Default::default()),
            ServerToClientMessage::UserRemove(Default::default()),
            ServerToClientMessage::UserState(raw::UserState {
                listening_volume_adjustment: vec![Default::default()],
                texture: Some(vec![255]),
                ..Default::default()
            }),
            ServerToClientMessage::CryptSetup(Default::default()),
            ServerToClientMessage::PermissionQuery(Default::default()),
            ServerToClientMessage::CodecVersion(Default::default()),
            ServerToClientMessage::ServerConfig(Default::default()),
        ];
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/server_to_client_messages.json")).unwrap();

        assert_eq!(serde_json::to_value(&samples).unwrap(), fixture);
    }

    // The TS side (src/protocol.ts) parses this exact shape with zod.
    #[test]
    fn server_message_serializes_as_tagged_json() {
        let message = ServerToClientMessage::Reject(raw::Reject {
            r#type: Some(raw::reject::RejectType::WrongUserPw as i32),
            reason: None,
        });

        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({ "type": "Reject", "data": { "type": 3, "reason": null } }),
        );
    }

    #[test]
    fn udp_tunnel_message_serializes_nested_tag() {
        let message = ServerToClientMessage::UdpTunnel(ServerToClientUdpMessage::Ping(raw_udp::Ping {
            timestamp: 1,
            ..Default::default()
        }));

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "UdpTunnel");
        assert_eq!(json["data"]["type"], "Ping");
        assert_eq!(json["data"]["data"]["timestamp"], 1);
    }

    #[test]
    fn create_authenticate_message() {
        let foo = super::raw::Authenticate {
            celt_versions: vec![],
            client_type: 0.into(),
            opus: false.into(),
            password: Some("".into()),
            tokens: vec![],
            username: Some("".into()),
        };
    }
}
