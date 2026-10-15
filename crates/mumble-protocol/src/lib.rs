pub use prost::Message;

pub mod mumble_proto {
    use std::io::Error;
    use prost::bytes::Buf;
    use prost::Message;

    // https://github.com/mumble-voip/mumble/blob/master/docs/dev/network-protocol/establishing_connection.md
    pub mod raw {
        include!(concat!(env!("OUT_DIR"), "/mumble_proto.rs"));
    }

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

    impl TcpMessageId for raw::ChannelState { const TYPE_ID: u16 = 7; }
    impl TcpMessageId for raw::UserState { const TYPE_ID: u16 = 9; }
    impl TcpMessageId for raw::CryptSetup { const TYPE_ID: u16 = 15; }
    impl TcpMessageId for raw::PermissionQuery { const TYPE_ID: u16 = 20; }
    impl TcpMessageId for raw::CodecVersion { const TYPE_ID: u16 = 21; }

    #[derive(Debug)]
    pub enum ServerToClientMessage {
        Version(raw::Version),
        Reject(raw::Reject),
        ServerSync(raw::ServerSync),
        ChannelState(raw::ChannelState),
        UserState(raw::UserState),
        CryptSetup(raw::CryptSetup),
        PermissionQuery(raw::PermissionQuery),
        CodecVersion(raw::CodecVersion),
    }

    impl ServerToClientMessage {
        pub fn decode(msg_type: u16, payload: &[u8]) -> Result<ServerToClientMessage, Error> {
            let message = match msg_type {
                raw::Version::TYPE_ID => ServerToClientMessage::Version(raw::Version::decode(&payload[..])?),
                raw::Reject::TYPE_ID => ServerToClientMessage::Reject(raw::Reject::decode(&payload[..])?),
                raw::ChannelState::TYPE_ID => ServerToClientMessage::ChannelState(raw::ChannelState::decode(&payload[..])?),
                raw::ServerSync::TYPE_ID => ServerToClientMessage::ServerSync(raw::ServerSync::decode(&payload[..])?),
                raw::UserState::TYPE_ID => ServerToClientMessage::UserState(raw::UserState::decode(&payload[..])?),
                raw::CryptSetup::TYPE_ID => ServerToClientMessage::CryptSetup(raw::CryptSetup::decode(&payload[..])?),
                raw::PermissionQuery::TYPE_ID => ServerToClientMessage::PermissionQuery(raw::PermissionQuery::decode(&payload[..])?),
                raw::CodecVersion::TYPE_ID => ServerToClientMessage::CodecVersion(raw::CodecVersion::decode(&payload[..])?),
                _ => {
                    return Err(Error::new(std::io::ErrorKind::InvalidData, format!("Unknown message type: {} (size: {})", msg_type, payload.len())));
                }
            };
            Ok(message)
        }
    }

    #[derive(Debug)]
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
}

pub mod mumble_udp {
    include!(concat!(env!("OUT_DIR"), "/mumble_udp.rs"));
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

    #[test]
    fn create_authenticate_message() {
        let foo = super::mumble_proto::raw::Authenticate {
            celt_versions: vec![],
            client_type: 0.into(),
            opus: false.into(),
            password: Some("".into()),
            tokens: vec![],
            username: Some("".into()),
        };
    }
}
