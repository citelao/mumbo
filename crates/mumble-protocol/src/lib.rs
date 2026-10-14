pub mod mumble_proto {
    include!(concat!(env!("OUT_DIR"), "/mumble_proto.rs"));
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
