//! Length-prefixed frames over an ordered byte stream.

use multiplex_screen_codec::MAX_BATCH_BYTES;

use crate::{Message, ProtocolError};

/// Largest control frame: every message except a batch.
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
/// Largest frame of any kind: a batch plus its kind byte.
pub const MAX_FRAME_BYTES: usize = MAX_BATCH_BYTES + 1;

/// Serialises a message as one frame: length, kind, body.
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, ProtocolError> {
    let body = message.encode()?;
    let limit = if message.is_batch() {
        MAX_FRAME_BYTES
    } else {
        MAX_CONTROL_FRAME_BYTES
    };
    if body.len() > limit {
        return Err(ProtocolError::FrameTooLarge);
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend((body.len() as u32).to_be_bytes());
    frame.extend(body);
    Ok(frame)
}

/// Collects stream bytes and yields complete messages. Feed it whatever the transport delivers,
/// in any chunk sizes. After an error the stream must be closed.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
    failed: bool,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes received but not yet returned as a message.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the next complete message, `Ok(None)` when more bytes are needed.
    pub fn next_message(&mut self) -> Result<Option<Message>, ProtocolError> {
        if self.failed {
            return Err(ProtocolError::Malformed);
        }
        let result = self.read();
        if result.is_err() {
            self.failed = true;
            self.buffer.clear();
        }
        result
    }

    fn read(&mut self) -> Result<Option<Message>, ProtocolError> {
        let Some(header) = self.buffer.get(..5) else {
            // A length above the largest frame is rejected as soon as its four bytes arrive.
            if let Some(length) = self.buffer.get(..4) {
                check_length(
                    u32::from_be_bytes(length.try_into().expect("four bytes")) as usize,
                    None,
                )?;
            }
            return Ok(None);
        };
        let length = u32::from_be_bytes(header[..4].try_into().expect("four bytes")) as usize;
        check_length(length, Some(header[4]))?;
        if self.buffer.len() < 4 + length {
            return Ok(None);
        }
        let message = Message::decode(&self.buffer[4..4 + length])?;
        self.buffer.drain(..4 + length);
        Ok(Some(message))
    }
}

fn check_length(length: usize, kind: Option<u8>) -> Result<(), ProtocolError> {
    if length == 0 {
        return Err(ProtocolError::Malformed);
    }
    let limit = match kind {
        Some(kind) if !Message::kind_is_batch(kind) => MAX_CONTROL_FRAME_BYTES,
        _ => MAX_FRAME_BYTES,
    };
    if length > limit {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointerButton, Viewport};
    use multiplex_screen_codec::Rect;
    use proptest::prelude::*;

    fn messages() -> Vec<Message> {
        vec![
            Message::Viewport(Viewport {
                surface: 1,
                rect: Rect::new(10, 20, 300, 200),
                scale_milli: 1500,
            }),
            Message::PointerButton {
                surface: 1,
                x: 5,
                y: 6,
                button: PointerButton::Primary,
                pressed: true,
            },
            Message::Text {
                surface: 1,
                text: "ls\n".to_owned(),
            },
        ]
    }

    #[test]
    fn frames_arrive_in_any_chunks() {
        let stream: Vec<u8> = messages()
            .iter()
            .flat_map(|m| encode_frame(m).unwrap())
            .collect();
        for chunk in [1, 2, 3, 7, stream.len()] {
            let mut reader = FrameReader::new();
            let mut received = Vec::new();
            for piece in stream.chunks(chunk) {
                reader.push(piece);
                while let Some(message) = reader.next_message().unwrap() {
                    received.push(message);
                }
            }
            assert_eq!(received, messages(), "chunk size {chunk}");
            assert_eq!(reader.buffered(), 0);
        }
    }

    #[test]
    fn oversized_and_empty_frames_close_the_stream() {
        let mut reader = FrameReader::new();
        reader.push(&(MAX_FRAME_BYTES as u32 + 1).to_be_bytes());
        assert_eq!(reader.next_message(), Err(ProtocolError::FrameTooLarge));
        assert_eq!(
            reader.next_message(),
            Err(ProtocolError::Malformed),
            "the reader stays failed"
        );

        let mut reader = FrameReader::new();
        let mut control = (MAX_CONTROL_FRAME_BYTES as u32 + 1).to_be_bytes().to_vec();
        control.push(0x12);
        reader.push(&control);
        assert_eq!(
            reader.next_message(),
            Err(ProtocolError::FrameTooLarge),
            "control frames have a smaller bound"
        );

        let mut reader = FrameReader::new();
        reader.push(&[0, 0, 0, 0]);
        assert_eq!(reader.next_message(), Err(ProtocolError::Malformed));
    }

    proptest! {
        #[test]
        fn random_streams_never_panic(chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..16)) {
            let mut reader = FrameReader::new();
            for chunk in chunks {
                reader.push(&chunk);
                while let Ok(Some(_)) = reader.next_message() {}
            }
        }
    }
}
