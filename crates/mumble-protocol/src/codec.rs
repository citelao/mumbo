use bytes::{Bytes, Buf};

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub type_id: u16,
    pub payload: Bytes,
}

pub struct MumbleCodec;

impl tokio_util::codec::Decoder for MumbleCodec {
    type Item = Frame;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Don't have a full payload yet.
        const HEADER_LEN: usize = 6;
        if src.len() < HEADER_LEN {
            return Ok(None);
        }

        let mut header = &src[..HEADER_LEN];
        let type_id = header.get_u16();
        let len = header.get_u32() as usize;

        let total_len = HEADER_LEN + len;
        if src.len() < total_len {
            return Ok(None);
        }

        src.advance(HEADER_LEN);
        let payload = src.split_to(len).freeze();
        Ok(Some(Frame { type_id, payload }))
    }
}

impl tokio_util::codec::Encoder<Frame> for MumbleCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Frame, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        todo!()
    }
}

// Ttests
#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use tokio_util::codec::Decoder;

    #[test]
    fn test_decode() {
        let mut codec = MumbleCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1, 2, 3]);
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.type_id, 1);
        assert_eq!(frame.payload, Bytes::from(&[1, 2, 3][..]));
    }

    // Decode an insufficient header
    #[test]
    fn test_decode_insufficient_header() {
        let mut codec = MumbleCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0]); // Only 5 bytes, less than HEADER_LEN
        let frame = codec.decode(&mut buf).unwrap();
        assert!(frame.is_none());
    }

    // Decode an insufficient payload
    #[test]
    fn test_decode_insufficient_payload() {
        let mut codec = MumbleCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1]); // Only 1 byte of payload, less than the declared 3
        let frame = codec.decode(&mut buf).unwrap();
        assert!(frame.is_none());
    }
}