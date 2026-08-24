use std::collections::VecDeque;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use termirust_host_protocol::{FrameDecoder, WireEnvelope};

use crate::{ClientError, ClientErrorCode};

pub struct AsyncEnvelopeStream<S> {
    stream: S,
    decoder: FrameDecoder,
    pending: VecDeque<WireEnvelope>,
}

impl<S> AsyncEnvelopeStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            pending: VecDeque::new(),
        }
    }

    pub fn into_inner(self) -> S {
        self.stream
    }

    pub async fn write(
        &mut self,
        envelope: &WireEnvelope,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let bytes = envelope.encode()?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ClientError::new(ClientErrorCode::Cancelled)),
            result = self.stream.write_all(&bytes) => result.map_err(ClientError::from),
        }
    }

    pub async fn read(&mut self, cancel: &CancellationToken) -> Result<WireEnvelope, ClientError> {
        if let Some(envelope) = self.pending.pop_front() {
            return Ok(envelope);
        }
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            let read = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(ClientError::new(ClientErrorCode::Cancelled));
                }
                result = self.stream.read(&mut chunk) => result.map_err(ClientError::from)?,
            };
            if read == 0 {
                return Err(ClientError::new(ClientErrorCode::EndOfStream));
            }
            self.pending.extend(self.decoder.push(&chunk[..read])?);
            if let Some(envelope) = self.pending.pop_front() {
                return Ok(envelope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_host_protocol::FrameKind;

    fn envelope(payload: &[u8]) -> WireEnvelope {
        WireEnvelope {
            protocol_major: 1,
            protocol_minor: 0,
            kind: FrameKind::ReadyEvent,
            flags: 0,
            request_id: [1; 16],
            payload: payload.to_vec(),
        }
    }

    #[tokio::test]
    async fn stream_handles_fragmented_and_coalesced_frames() {
        let (mut writer, reader) = tokio::io::duplex(128 * 1024);
        let expected = [envelope(b"one"), envelope(b"two")];
        let mut bytes = expected[0].encode().unwrap();
        bytes.extend_from_slice(&expected[1].encode().unwrap());
        let write_task = tokio::spawn(async move {
            for chunk in bytes.chunks(3) {
                writer.write_all(chunk).await.unwrap();
            }
        });
        let cancel = CancellationToken::new();
        let mut stream = AsyncEnvelopeStream::new(reader);
        assert_eq!(stream.read(&cancel).await.unwrap(), expected[0]);
        assert_eq!(stream.read(&cancel).await.unwrap(), expected[1]);
        write_task.await.unwrap();
    }

    #[tokio::test]
    async fn blocked_read_cancels() {
        let (_writer, reader) = tokio::io::duplex(64);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut stream = AsyncEnvelopeStream::new(reader);
        assert_eq!(
            stream.read(&cancel).await.unwrap_err().code,
            ClientErrorCode::Cancelled
        );
    }
}
