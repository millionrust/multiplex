use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::{ListenerError, ListenerErrorCode};

pub async fn read_bounded_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum_bytes: usize,
) -> Result<Vec<u8>, ListenerError> {
    let length = reader.read_u32().await.map_err(ListenerError::from)? as usize;
    if length == 0 {
        return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
    }
    if length > maximum_bytes {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(ListenerError::from)?;
    Ok(bytes)
}

pub async fn write_bounded_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    maximum_bytes: usize,
) -> Result<(), ListenerError> {
    if bytes.is_empty() {
        return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
    }
    if bytes.len() > maximum_bytes || bytes.len() > u32::MAX as usize {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    writer
        .write_u32(bytes.len() as u32)
        .await
        .map_err(ListenerError::from)?;
    writer.write_all(bytes).await.map_err(ListenerError::from)?;
    writer.flush().await.map_err(ListenerError::from)
}
