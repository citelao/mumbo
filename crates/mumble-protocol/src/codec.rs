use bytes::{Bytes, Buf};

#[derive(Debug, Clone, PartialEq)]
pub struct MumbleFrame {
    pub type_id: u16,
    pub payload: Bytes,
}

pub struct MumbleFrameCodec;

impl tokio_util::codec::Decoder for MumbleFrameCodec {
    type Item = MumbleFrame;
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
        Ok(Some(MumbleFrame { type_id, payload }))
    }
}

impl tokio_util::codec::Encoder<MumbleFrame> for MumbleFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: MumbleFrame, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
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
        let mut codec = MumbleFrameCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1, 2, 3]);
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.type_id, 1);
        assert_eq!(frame.payload, Bytes::from(&[1, 2, 3][..]));
    }

    // Decode an insufficient header
    #[test]
    fn test_decode_insufficient_header() {
        let mut codec = MumbleFrameCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0]); // Only 5 bytes, less than HEADER_LEN
        let frame = codec.decode(&mut buf).unwrap();
        assert!(frame.is_none());
        assert_eq!(buf.len(), 5); // Ensure the buffer remains unchanged
    }

    // Decode an insufficient payload
    #[test]
    fn test_decode_insufficient_payload() {
        let mut codec = MumbleFrameCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1]); // Only 1 byte of payload, less than the declared 3
        let frame = codec.decode(&mut buf).unwrap();
        assert!(frame.is_none());
        assert_eq!(buf.len(), 7); // Ensure the buffer remains unchanged
    }

    #[test]
    fn test_two_simultaneous_frames() {
        let mut codec = MumbleFrameCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1, 2, 3, 0, 2, 0, 0, 0, 2, 4, 5]);
        let frame1 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame1.type_id, 1);
        assert_eq!(frame1.payload, Bytes::from(&[1, 2, 3][..]));
        let frame2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame2.type_id, 2);
        assert_eq!(frame2.payload, Bytes::from(&[4, 5][..]));
        assert!(buf.is_empty()); // Ensure the buffer is empty after decoding both frames
    }

    // 1 full frame followed by a partial frame
    #[test]
    fn test_frame_plus_partial() {
        let mut codec = MumbleFrameCodec;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0, 1, 0, 0, 0, 3, 1, 2, 3, 0, 2, 0, 0, 0]); // Second frame is partial
        let frame1 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame1.type_id, 1);
        assert_eq!(frame1.payload, Bytes::from(&[1, 2, 3][..]));
        let frame2 = codec.decode(&mut buf).unwrap();
        assert!(frame2.is_none());
        assert_eq!(buf.len(), 5); // Ensure the buffer contains the partial second frame

        // Attempt to decode again after adding more bytes to complete the second frame
        buf.extend_from_slice(&[3, 6, 7, 8]); // Complete the second frame payload
        let frame2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame2.type_id, 2);
        assert_eq!(frame2.payload, Bytes::from(&[6, 7, 8][..]));
        assert!(buf.is_empty()); // Ensure the buffer is empty after decoding the second frame
    }

}

pub struct MumbleCodec;

impl tokio_util::codec::Decoder for MumbleCodec {
    type Item = crate::ServerToClientMessage;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = MumbleFrameCodec.decode(src)? {
            let message = crate::ServerToClientMessage::decode(frame.type_id, &frame.payload)?;
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}