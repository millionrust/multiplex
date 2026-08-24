use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use termirust_host_protocol::{FrameDecoder, WireEnvelope};

use crate::{HostError, HostErrorCode};

pub struct HostWireStream<S> {
    stream: S,
    decoder: FrameDecoder,
    ready: std::collections::VecDeque<WireEnvelope>,
}

impl<S> HostWireStream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            ready: std::collections::VecDeque::new(),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> HostWireStream<S> {
    pub async fn read(&mut self, cancel: &CancellationToken) -> Result<WireEnvelope, HostError> {
        loop {
            if let Some(envelope) = self.ready.pop_front() {
                return Ok(envelope);
            }
            let mut bytes = [0_u8; 8192];
            let count = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(HostError::new(HostErrorCode::Cancelled)),
                result = self.stream.read(&mut bytes) => result.map_err(HostError::io)?,
            };
            if count == 0 {
                return Err(HostError::new(HostErrorCode::Cancelled));
            }
            self.ready.extend(self.decoder.push(&bytes[..count])?);
        }
    }

    pub async fn write(
        &mut self,
        envelope: &WireEnvelope,
        cancel: &CancellationToken,
    ) -> Result<(), HostError> {
        let bytes = envelope.encode()?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(HostError::new(HostErrorCode::Cancelled)),
            result = self.stream.write_all(&bytes) => result.map_err(HostError::io),
        }
    }
}
