use std::fmt;
use std::io::{Cursor, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use image::{ColorType, ImageDecoder as _, ImageFormat, ImageReader, Limits};
use termirust_domain::{
    ArtifactCancellation, ArtifactError, ArtifactLimits, ArtifactMediaType, ArtifactPreviewKind,
};
use termirust_store::ArtifactPayload;

const WORKER_MAGIC: &[u8; 8] = b"TRAPRVW1";
const WORKER_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const RESPONSE_OK: u8 = 0;
const RESPONSE_ERROR: u8 = 1;

#[derive(Clone, Eq, PartialEq)]
pub enum ArtifactPreview {
    Text {
        value: String,
        truncated: bool,
    },
    Raster {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    MetadataOnly,
}

impl fmt::Debug for ArtifactPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { value, truncated } => formatter
                .debug_struct("ArtifactPreview::Text")
                .field("value", &format_args!("<redacted:{} bytes>", value.len()))
                .field("truncated", truncated)
                .finish(),
            Self::Raster {
                width,
                height,
                rgba,
            } => formatter
                .debug_struct("ArtifactPreview::Raster")
                .field("width", width)
                .field("height", height)
                .field("rgba", &format_args!("<redacted:{} bytes>", rgba.len()))
                .finish(),
            Self::MetadataOnly => formatter.write_str("ArtifactPreview::MetadataOnly"),
        }
    }
}

pub fn build_preview(
    payload: &ArtifactPayload,
    limits: ArtifactLimits,
    cancellation: &ArtifactCancellation,
) -> Result<ArtifactPreview, ArtifactError> {
    limits.validate()?;
    cancellation.check()?;
    match payload.metadata.preview_kind {
        ArtifactPreviewKind::Text => build_text_preview(&payload.bytes, limits, cancellation),
        ArtifactPreviewKind::Raster => build_raster_preview(
            payload.metadata.media_type,
            &payload.bytes,
            limits,
            cancellation,
        ),
        ArtifactPreviewKind::MetadataOnly => Ok(ArtifactPreview::MetadataOnly),
    }
}

fn build_text_preview(
    bytes: &[u8],
    limits: ArtifactLimits,
    cancellation: &ArtifactCancellation,
) -> Result<ArtifactPreview, ArtifactError> {
    cancellation.check()?;
    let limit =
        usize::try_from(limits.text_preview_bytes).map_err(|_| ArtifactError::DecodeFailed)?;
    let candidate_end = bytes.len().min(limit);
    let end = match std::str::from_utf8(&bytes[..candidate_end]) {
        Ok(_) => candidate_end,
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => return Err(ArtifactError::DecodeFailed),
    };
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| ArtifactError::DecodeFailed)?;
    let value = strip_terminal_controls(text);
    cancellation.check()?;
    Ok(ArtifactPreview::Text {
        value,
        truncated: end < bytes.len(),
    })
}

fn strip_terminal_controls(value: &str) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        Ground,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }

    let mut output = String::with_capacity(value.len());
    let mut state = EscapeState::Ground;
    for character in value.chars() {
        state = match state {
            EscapeState::Ground if character == '\u{1b}' => EscapeState::Escape,
            EscapeState::Ground => {
                if !character.is_control() || matches!(character, '\n' | '\r' | '\t') {
                    output.push(character);
                }
                EscapeState::Ground
            }
            EscapeState::Escape if character == '[' => EscapeState::Csi,
            EscapeState::Escape if character == ']' => EscapeState::Osc,
            EscapeState::Escape => EscapeState::Ground,
            EscapeState::Csi if ('\u{40}'..='\u{7e}').contains(&character) => EscapeState::Ground,
            EscapeState::Csi => EscapeState::Csi,
            EscapeState::Osc if character == '\u{7}' => EscapeState::Ground,
            EscapeState::Osc if character == '\u{1b}' => EscapeState::OscEscape,
            EscapeState::Osc => EscapeState::Osc,
            EscapeState::OscEscape if character == '\\' => EscapeState::Ground,
            EscapeState::OscEscape => EscapeState::Osc,
        };
    }
    output
}

fn build_raster_preview(
    media_type: ArtifactMediaType,
    bytes: &[u8],
    limits: ArtifactLimits,
    cancellation: &ArtifactCancellation,
) -> Result<ArtifactPreview, ArtifactError> {
    if bytes.len() as u64 > limits.item_bytes {
        return Err(ArtifactError::ItemQuotaExceeded);
    }
    let started = Instant::now();
    let format = worker_format(media_type)?;
    let executable = std::env::current_exe().map_err(|_| ArtifactError::Unavailable)?;
    let mut child = Command::new(executable)
        .arg(crate::ARTIFACT_PREVIEW_MODE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ArtifactError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(ArtifactError::Unavailable)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let raster_bytes = limits.raster_bytes;
    let raster_pixels = limits.raster_pixels;
    let reader = thread::spawn(move || {
        let result = read_worker_response(stdout, raster_pixels, raster_bytes);
        let _ = sender.send(result);
    });

    let write_result = child
        .stdin
        .take()
        .ok_or(ArtifactError::Unavailable)
        .and_then(|mut stdin| {
            write_worker_request(
                &mut stdin,
                format,
                limits.raster_pixels,
                limits.raster_bytes,
                bytes,
            )
        });
    if let Err(error) = write_result {
        terminate_worker(&mut child);
        let _ = reader.join();
        return Err(error);
    }

    let result = loop {
        if cancellation.is_cancelled() {
            terminate_worker(&mut child);
            break Err(ArtifactError::Cancelled);
        }
        if started.elapsed() >= WORKER_TIMEOUT {
            terminate_worker(&mut child);
            break Err(ArtifactError::Timeout);
        }
        match receiver.try_recv() {
            Ok(result) => {
                let status = child.wait().map_err(|_| ArtifactError::DecodeFailed)?;
                break if status.success() {
                    result
                } else {
                    Err(ArtifactError::DecodeFailed)
                };
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                terminate_worker(&mut child);
                break Err(ArtifactError::DecodeFailed);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Some(status) = child.try_wait().map_err(|_| ArtifactError::DecodeFailed)? {
            let response = receiver
                .recv_timeout(WORKER_POLL_INTERVAL)
                .unwrap_or(Err(ArtifactError::DecodeFailed));
            break if status.success() {
                response
            } else {
                Err(ArtifactError::DecodeFailed)
            };
        }
        thread::sleep(WORKER_POLL_INTERVAL);
    };
    let _ = reader.join();
    result
}

fn terminate_worker(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn worker_format(media_type: ArtifactMediaType) -> Result<u8, ArtifactError> {
    match media_type {
        ArtifactMediaType::ImagePng => Ok(1),
        ArtifactMediaType::ImageJpeg => Ok(2),
        ArtifactMediaType::TextPlainUtf8 | ArtifactMediaType::MetadataOnly => {
            Err(ArtifactError::DecodeFailed)
        }
    }
}

fn write_worker_request(
    output: &mut impl Write,
    format: u8,
    raster_pixels: u64,
    raster_bytes: u64,
    bytes: &[u8],
) -> Result<(), ArtifactError> {
    output
        .write_all(WORKER_MAGIC)
        .and_then(|_| output.write_all(&[format]))
        .and_then(|_| output.write_all(&raster_pixels.to_le_bytes()))
        .and_then(|_| output.write_all(&raster_bytes.to_le_bytes()))
        .and_then(|_| output.write_all(&(bytes.len() as u64).to_le_bytes()))
        .and_then(|_| output.write_all(bytes))
        .and_then(|_| output.flush())
        .map_err(|_| ArtifactError::DecodeFailed)
}

fn read_worker_response(
    mut input: impl Read,
    raster_pixels: u64,
    raster_bytes: u64,
) -> Result<ArtifactPreview, ArtifactError> {
    let mut magic = [0_u8; WORKER_MAGIC.len()];
    input
        .read_exact(&mut magic)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    if &magic != WORKER_MAGIC {
        return Err(ArtifactError::DecodeFailed);
    }
    let status = read_u8(&mut input)?;
    if status != RESPONSE_OK {
        return Err(ArtifactError::DecodeFailed);
    }
    let width = read_u32(&mut input)?;
    let height = read_u32(&mut input)?;
    let byte_len = read_u64(&mut input)?;
    validate_raster_bounds(width, height, byte_len, raster_pixels, raster_bytes)?;
    let byte_len = usize::try_from(byte_len).map_err(|_| ArtifactError::DecodeFailed)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(byte_len)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    rgba.resize(byte_len, 0);
    input
        .read_exact(&mut rgba)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    Ok(ArtifactPreview::Raster {
        width,
        height,
        rgba,
    })
}

pub fn run_worker_mode() -> i32 {
    let result = read_worker_request(std::io::stdin().lock()).and_then(
        |(format, raster_pixels, raster_bytes, bytes)| {
            decode_raster(format, &bytes, raster_pixels, raster_bytes)
        },
    );
    let mut stdout = std::io::stdout().lock();
    let write_result = match result {
        Ok(decoded) => write_worker_success(&mut stdout, &decoded),
        Err(_) => stdout
            .write_all(WORKER_MAGIC)
            .and_then(|_| stdout.write_all(&[RESPONSE_ERROR]))
            .map_err(|_| ArtifactError::DecodeFailed),
    };
    if write_result
        .and_then(|_| stdout.flush().map_err(|_| ArtifactError::DecodeFailed))
        .is_ok()
    {
        0
    } else {
        1
    }
}

fn read_worker_request(mut input: impl Read) -> Result<(u8, u64, u64, Vec<u8>), ArtifactError> {
    let mut magic = [0_u8; WORKER_MAGIC.len()];
    input
        .read_exact(&mut magic)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    if &magic != WORKER_MAGIC {
        return Err(ArtifactError::DecodeFailed);
    }
    let format = read_u8(&mut input)?;
    if !matches!(format, 1 | 2) {
        return Err(ArtifactError::DecodeFailed);
    }
    let raster_pixels = read_u64(&mut input)?;
    let raster_bytes = read_u64(&mut input)?;
    let input_len = read_u64(&mut input)?;
    let hard_limits = ArtifactLimits::default();
    if raster_pixels == 0
        || raster_pixels > hard_limits.raster_pixels
        || raster_bytes == 0
        || raster_bytes > hard_limits.raster_bytes
        || input_len > hard_limits.item_bytes
    {
        return Err(ArtifactError::DecodeFailed);
    }
    let input_len = usize::try_from(input_len).map_err(|_| ArtifactError::DecodeFailed)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(input_len)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    bytes.resize(input_len, 0);
    input
        .read_exact(&mut bytes)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    Ok((format, raster_pixels, raster_bytes, bytes))
}

struct DecodedRaster {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_raster(
    format: u8,
    bytes: &[u8],
    raster_pixels: u64,
    raster_bytes: u64,
) -> Result<DecodedRaster, ArtifactError> {
    let format = match format {
        1 => ImageFormat::Png,
        2 => ImageFormat::Jpeg,
        _ => return Err(ArtifactError::DecodeFailed),
    };
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|_| ArtifactError::DecodeFailed)?;
    let (width, height) = decoder.dimensions();
    let rgba_len = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ArtifactError::DecodeFailed)?;
    validate_raster_bounds(width, height, rgba_len, raster_pixels, raster_bytes)?;
    let raw_len = decoder.total_bytes();
    if raw_len > raster_bytes {
        return Err(ArtifactError::DecodeFailed);
    }
    let max_dimension = u32::try_from(raster_pixels).unwrap_or(u32::MAX);
    let mut image_limits = Limits::default();
    image_limits.max_image_width = Some(max_dimension);
    image_limits.max_image_height = Some(max_dimension);
    image_limits.max_alloc = Some(raster_bytes);
    decoder
        .set_limits(image_limits)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    let color_type = decoder.color_type();
    if !matches!(
        color_type,
        ColorType::L8
            | ColorType::La8
            | ColorType::Rgb8
            | ColorType::Rgba8
            | ColorType::L16
            | ColorType::La16
            | ColorType::Rgb16
            | ColorType::Rgba16
    ) {
        return Err(ArtifactError::DecodeFailed);
    }
    let capacity =
        usize::try_from(raw_len.max(rgba_len)).map_err(|_| ArtifactError::DecodeFailed)?;
    let raw_len = usize::try_from(raw_len).map_err(|_| ArtifactError::DecodeFailed)?;
    let rgba_len = usize::try_from(rgba_len).map_err(|_| ArtifactError::DecodeFailed)?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(capacity)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    buffer.resize(raw_len, 0);
    decoder
        .read_image(&mut buffer)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    convert_to_rgba(&mut buffer, color_type, rgba_len)?;
    Ok(DecodedRaster {
        width,
        height,
        rgba: buffer,
    })
}

fn validate_raster_bounds(
    width: u32,
    height: u32,
    byte_len: u64,
    raster_pixels: u64,
    raster_bytes: u64,
) -> Result<(), ArtifactError> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ArtifactError::DecodeFailed)?;
    let expected = pixels.checked_mul(4).ok_or(ArtifactError::DecodeFailed)?;
    if width == 0
        || height == 0
        || pixels > raster_pixels
        || byte_len != expected
        || byte_len > raster_bytes
    {
        return Err(ArtifactError::DecodeFailed);
    }
    Ok(())
}

fn convert_to_rgba(
    buffer: &mut Vec<u8>,
    color_type: ColorType,
    rgba_len: usize,
) -> Result<(), ArtifactError> {
    let source_len = buffer.len();
    let source_pixel_bytes = usize::from(color_type.bytes_per_pixel());
    if source_pixel_bytes == 0 || !source_len.is_multiple_of(source_pixel_bytes) {
        return Err(ArtifactError::DecodeFailed);
    }
    let pixels = source_len / source_pixel_bytes;
    if pixels.checked_mul(4) != Some(rgba_len) {
        return Err(ArtifactError::DecodeFailed);
    }
    match color_type {
        ColorType::Rgba8 => {}
        ColorType::Rgb8 => {
            buffer.resize(rgba_len, 0);
            for index in (0..pixels).rev() {
                let source = index * 3;
                let destination = index * 4;
                let rgb = [buffer[source], buffer[source + 1], buffer[source + 2]];
                buffer[destination..destination + 4]
                    .copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        ColorType::L8 => {
            buffer.resize(rgba_len, 0);
            for index in (0..pixels).rev() {
                let luma = buffer[index];
                let destination = index * 4;
                buffer[destination..destination + 4].copy_from_slice(&[luma, luma, luma, 255]);
            }
        }
        ColorType::La8 => {
            buffer.resize(rgba_len, 0);
            for index in (0..pixels).rev() {
                let source = index * 2;
                let luma = buffer[source];
                let alpha = buffer[source + 1];
                let destination = index * 4;
                buffer[destination..destination + 4].copy_from_slice(&[luma, luma, luma, alpha]);
            }
        }
        ColorType::L16 => {
            buffer.resize(rgba_len, 0);
            for index in (0..pixels).rev() {
                let source = index * 2;
                let luma = high_byte(&buffer[source..source + 2]);
                let destination = index * 4;
                buffer[destination..destination + 4].copy_from_slice(&[luma, luma, luma, 255]);
            }
        }
        ColorType::La16 => {
            for index in 0..pixels {
                let source = index * 4;
                let luma = high_byte(&buffer[source..source + 2]);
                let alpha = high_byte(&buffer[source + 2..source + 4]);
                buffer[source..source + 4].copy_from_slice(&[luma, luma, luma, alpha]);
            }
        }
        ColorType::Rgb16 => {
            for index in 0..pixels {
                let source = index * 6;
                let destination = index * 4;
                let red = high_byte(&buffer[source..source + 2]);
                let green = high_byte(&buffer[source + 2..source + 4]);
                let blue = high_byte(&buffer[source + 4..source + 6]);
                buffer[destination..destination + 4].copy_from_slice(&[red, green, blue, 255]);
            }
            buffer.truncate(rgba_len);
        }
        ColorType::Rgba16 => {
            for index in 0..pixels {
                let source = index * 8;
                let destination = index * 4;
                let red = high_byte(&buffer[source..source + 2]);
                let green = high_byte(&buffer[source + 2..source + 4]);
                let blue = high_byte(&buffer[source + 4..source + 6]);
                let alpha = high_byte(&buffer[source + 6..source + 8]);
                buffer[destination..destination + 4].copy_from_slice(&[red, green, blue, alpha]);
            }
            buffer.truncate(rgba_len);
        }
        _ => return Err(ArtifactError::DecodeFailed),
    }
    Ok(())
}

fn high_byte(bytes: &[u8]) -> u8 {
    (u16::from_ne_bytes([bytes[0], bytes[1]]) >> 8) as u8
}

fn write_worker_success(
    output: &mut impl Write,
    decoded: &DecodedRaster,
) -> Result<(), ArtifactError> {
    output
        .write_all(WORKER_MAGIC)
        .and_then(|_| output.write_all(&[RESPONSE_OK]))
        .and_then(|_| output.write_all(&decoded.width.to_le_bytes()))
        .and_then(|_| output.write_all(&decoded.height.to_le_bytes()))
        .and_then(|_| output.write_all(&(decoded.rgba.len() as u64).to_le_bytes()))
        .and_then(|_| output.write_all(&decoded.rgba))
        .map_err(|_| ArtifactError::DecodeFailed)
}

fn read_u8(input: &mut impl Read) -> Result<u8, ArtifactError> {
    let mut bytes = [0_u8; 1];
    input
        .read_exact(&mut bytes)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    Ok(bytes[0])
}

fn read_u32(input: &mut impl Read) -> Result<u32, ArtifactError> {
    let mut bytes = [0_u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> Result<u64, ArtifactError> {
    let mut bytes = [0_u8; 8];
    input
        .read_exact(&mut bytes)
        .map_err(|_| ArtifactError::DecodeFailed)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use image::{ExtendedColorType, ImageEncoder as _, codecs::png::PngEncoder};
    use termirust_domain::{
        ArtifactDisplayName, ArtifactId, ArtifactMetadata, ArtifactOrigin, ArtifactScope,
        ArtifactSha256, ArtifactState, HostedSessionId,
    };

    use super::*;

    fn payload(media_type: ArtifactMediaType, bytes: Vec<u8>) -> ArtifactPayload {
        let preview_kind = match media_type {
            ArtifactMediaType::TextPlainUtf8 => ArtifactPreviewKind::Text,
            ArtifactMediaType::ImagePng | ArtifactMediaType::ImageJpeg => {
                ArtifactPreviewKind::Raster
            }
            ArtifactMediaType::MetadataOnly => ArtifactPreviewKind::MetadataOnly,
        };
        ArtifactPayload {
            metadata: ArtifactMetadata {
                id: ArtifactId::new(),
                scope: ArtifactScope {
                    session_id: HostedSessionId::new(),
                },
                display_name: ArtifactDisplayName::new("preview").unwrap(),
                origin: ArtifactOrigin::ExplicitImport,
                media_type,
                byte_len: bytes.len() as u64,
                sha256: ArtifactSha256::new([0; 32]),
                created_at: 0,
                preview_kind,
                state: ArtifactState::Ready,
            },
            bytes,
        }
    }

    #[test]
    fn artifact_preview_text_is_literal_bounded_and_strips_terminal_controls() {
        let limits = ArtifactLimits {
            text_preview_bytes: 32,
            ..ArtifactLimits::default()
        };
        let preview = build_preview(
            &payload(
                ArtifactMediaType::TextPlainUtf8,
                b"**literal**\x1b[31mRED\x1b[0m\nlink: [x](https://example.invalid)".to_vec(),
            ),
            limits,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            preview,
            ArtifactPreview::Text {
                value: "**literal**RED\nlink: [x".to_string(),
                truncated: true,
            }
        );
    }

    #[test]
    fn artifact_preview_text_truncates_on_a_utf8_boundary_and_rejects_invalid_content() {
        let limits = ArtifactLimits {
            text_preview_bytes: 2,
            ..ArtifactLimits::default()
        };
        assert_eq!(
            build_preview(
                &payload(ArtifactMediaType::TextPlainUtf8, "éx".as_bytes().to_vec()),
                limits,
                &Default::default(),
            ),
            Ok(ArtifactPreview::Text {
                value: "é".to_string(),
                truncated: true,
            })
        );
        assert_eq!(
            build_preview(
                &payload(ArtifactMediaType::TextPlainUtf8, vec![b'a', 0xff]),
                limits,
                &Default::default(),
            ),
            Err(ArtifactError::DecodeFailed)
        );
    }

    #[test]
    fn artifact_preview_metadata_never_decodes_bytes() {
        let preview = build_preview(
            &payload(
                ArtifactMediaType::MetadataOnly,
                b"<svg onload='never()'>".to_vec(),
            ),
            ArtifactLimits::default(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(preview, ArtifactPreview::MetadataOnly);
    }

    #[test]
    fn artifact_preview_decodes_allowlisted_png_into_inert_rgba() {
        let mut encoded = Vec::new();
        PngEncoder::new(&mut encoded)
            .write_image(
                &[255, 0, 0, 255, 0, 255, 0, 128],
                2,
                1,
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        let decoded = decode_raster(
            1,
            &encoded,
            ArtifactLimits::default().raster_pixels,
            ArtifactLimits::default().raster_bytes,
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert_eq!(decoded.rgba, [255, 0, 0, 255, 0, 255, 0, 128]);
    }

    #[test]
    fn artifact_preview_rejects_bad_format_dimensions_and_unbounded_worker_output() {
        assert_eq!(
            decode_raster(1, b"not a png", 20_000_000, 80 * 1024 * 1024).err(),
            Some(ArtifactError::DecodeFailed)
        );
        assert_eq!(
            validate_raster_bounds(10, 10, 400, 99, 400),
            Err(ArtifactError::DecodeFailed)
        );
        let mut response = Vec::new();
        response.extend_from_slice(WORKER_MAGIC);
        response.push(RESPONSE_OK);
        response.extend_from_slice(&10_u32.to_le_bytes());
        response.extend_from_slice(&10_u32.to_le_bytes());
        response.extend_from_slice(&401_u64.to_le_bytes());
        assert_eq!(
            read_worker_response(Cursor::new(response), 100, 400),
            Err(ArtifactError::DecodeFailed)
        );
    }

    #[test]
    fn artifact_preview_honors_cancellation_before_work() {
        let cancellation = ArtifactCancellation::default();
        cancellation.cancel();
        assert_eq!(
            build_preview(
                &payload(ArtifactMediaType::TextPlainUtf8, b"text".to_vec()),
                ArtifactLimits::default(),
                &cancellation,
            ),
            Err(ArtifactError::Cancelled)
        );
    }
}
