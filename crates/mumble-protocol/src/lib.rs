pub use prost::Message;

pub mod mumble_proto {

    pub mod raw {
        include!(concat!(env!("OUT_DIR"), "/mumble_proto.rs"));
    }

    pub trait TcpMessageId: prost::Message + Default {
        const TYPE_ID: u16;
    }

    impl TcpMessageId for raw::Version { const TYPE_ID: u16 = 0; }
    impl TcpMessageId for raw::Authenticate { const TYPE_ID: u16 = 2; }

    #[derive(Debug)]
    pub enum ServerToClientMessage {
        Version(raw::Version),
    }

    #[derive(Debug)]
    pub enum ClientToServerMessage {
        Version(raw::Version),
        Authenticate(raw::Authenticate),
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
        let foo = super::mumble_proto::Authenticate {
            celt_versions: vec![],
            client_type: 0.into(),
            opus: false.into(),
            password: Some("".into()),
            tokens: vec![],
            username: Some("".into()),
        };
    }
}
