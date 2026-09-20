use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{ArtifactId, HostedSessionId};

pub const MAX_ARTIFACT_DISPLAY_NAME_GRAPHEMES: usize = 255;
pub const MAX_ARTIFACT_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_SESSION_ARTIFACT_BYTES: u64 = 250 * 1024 * 1024;
pub const MAX_GLOBAL_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ARTIFACTS_PER_SESSION: usize = 1_000;
pub const MAX_GLOBAL_ARTIFACTS: usize = 10_000;
pub const MAX_TEXT_PREVIEW_BYTES: u64 = 1024 * 1024;
pub const MAX_RASTER_PIXELS: u64 = 20_000_000;
pub const MAX_RASTER_BYTES: u64 = 80 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOrigin {
    ExplicitImport,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactScope {
    pub session_id: HostedSessionId,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactDisplayName(String);

impl ArtifactDisplayName {
    pub fn new(value: &str) -> Result<Self, ArtifactError> {
        let sanitized = sanitize_display_name(value);
        if sanitized.is_empty() {
            return Err(ArtifactError::InvalidDisplayName);
        }
        Ok(Self(sanitized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ArtifactDisplayName {
    fn default() -> Self {
        Self("artifact".to_string())
    }
}

impl fmt::Debug for ArtifactDisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactDisplayName(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactMediaType {
    TextPlainUtf8,
    ImagePng,
    ImageJpeg,
    MetadataOnly,
}

impl ArtifactMediaType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextPlainUtf8 => "text/plain; charset=utf-8",
            Self::ImagePng => "image/png",
            Self::ImageJpeg => "image/jpeg",
            Self::MetadataOnly => "application/octet-stream",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPreviewKind {
    Text,
    Raster,
    MetadataOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactState {
    Staging,
    Ready,
    Quarantined,
    Corrupt,
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactSha256([u8; 32]);

impl ArtifactSha256 {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn short_label(&self) -> String {
        encode_hex(&self.0[..6])
    }
}

impl fmt::Debug for ArtifactSha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactSha256(<redacted>)")
    }
}

impl fmt::Display for ArtifactSha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl FromStr for ArtifactSha256 {
    type Err = ArtifactError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ArtifactError::InvalidDigest);
        }
        let mut bytes = [0_u8; 32];
        for (index, output) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            *output = u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| ArtifactError::InvalidDigest)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for ArtifactSha256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for ArtifactSha256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMetadata {
    pub id: ArtifactId,
    pub scope: ArtifactScope,
    pub display_name: ArtifactDisplayName,
    pub origin: ArtifactOrigin,
    pub media_type: ArtifactMediaType,
    pub byte_len: u64,
    pub sha256: ArtifactSha256,
    pub created_at: u64,
    pub preview_kind: ArtifactPreviewKind,
    pub state: ArtifactState,
}

impl ArtifactMetadata {
    pub fn validate(&self, limits: ArtifactLimits) -> Result<(), ArtifactError> {
        limits.validate()?;
        if ArtifactDisplayName::new(self.display_name.as_str())? != self.display_name {
            return Err(ArtifactError::InvalidDisplayName);
        }
        if self.byte_len > limits.item_bytes {
            return Err(ArtifactError::ItemQuotaExceeded);
        }
        let expected_preview = match self.media_type {
            ArtifactMediaType::TextPlainUtf8 => ArtifactPreviewKind::Text,
            ArtifactMediaType::ImagePng | ArtifactMediaType::ImageJpeg => {
                ArtifactPreviewKind::Raster
            }
            ArtifactMediaType::MetadataOnly => ArtifactPreviewKind::MetadataOnly,
        };
        if self.preview_kind != expected_preview || self.state == ArtifactState::Staging {
            return Err(ArtifactError::InvalidMetadata);
        }
        Ok(())
    }
}

impl fmt::Debug for ArtifactMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactMetadata")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("display_name", &"<redacted>")
            .field("origin", &self.origin)
            .field("media_type", &self.media_type)
            .field("byte_len", &self.byte_len)
            .field("sha256", &"<redacted>")
            .field("created_at", &self.created_at)
            .field("preview_kind", &self.preview_kind)
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactLimits {
    pub item_bytes: u64,
    pub session_bytes: u64,
    pub global_bytes: u64,
    pub artifacts_per_session: usize,
    pub global_artifacts: usize,
    pub text_preview_bytes: u64,
    pub raster_pixels: u64,
    pub raster_bytes: u64,
}

#[derive(Clone, Default)]
pub struct ArtifactCancellation(Arc<AtomicBool>);

impl ArtifactCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<(), ArtifactError> {
        if self.is_cancelled() {
            Err(ArtifactError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for ArtifactCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl ArtifactLimits {
    pub fn validate(self) -> Result<(), ArtifactError> {
        if self.item_bytes == 0
            || self.item_bytes > MAX_ARTIFACT_BYTES
            || self.session_bytes < self.item_bytes
            || self.session_bytes > MAX_SESSION_ARTIFACT_BYTES
            || self.global_bytes < self.session_bytes
            || self.global_bytes > MAX_GLOBAL_ARTIFACT_BYTES
            || self.artifacts_per_session == 0
            || self.artifacts_per_session > MAX_ARTIFACTS_PER_SESSION
            || self.global_artifacts < self.artifacts_per_session
            || self.global_artifacts > MAX_GLOBAL_ARTIFACTS
            || self.text_preview_bytes == 0
            || self.text_preview_bytes > MAX_TEXT_PREVIEW_BYTES
            || self.raster_pixels == 0
            || self.raster_pixels > MAX_RASTER_PIXELS
            || self.raster_bytes == 0
            || self.raster_bytes > MAX_RASTER_BYTES
        {
            return Err(ArtifactError::InvalidLimits);
        }
        Ok(())
    }
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            item_bytes: MAX_ARTIFACT_BYTES,
            session_bytes: MAX_SESSION_ARTIFACT_BYTES,
            global_bytes: MAX_GLOBAL_ARTIFACT_BYTES,
            artifacts_per_session: MAX_ARTIFACTS_PER_SESSION,
            global_artifacts: MAX_GLOBAL_ARTIFACTS,
            text_preview_bytes: MAX_TEXT_PREVIEW_BYTES,
            raster_pixels: MAX_RASTER_PIXELS,
            raster_bytes: MAX_RASTER_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactError {
    InvalidDisplayName,
    InvalidDigest,
    InvalidLimits,
    InvalidMetadata,
    InvalidState,
    ItemQuotaExceeded,
    SessionQuotaExceeded,
    GlobalQuotaExceeded,
    CountQuotaExceeded,
    SourceChanged,
    UnsupportedSource,
    UnsafeEntry,
    Unavailable,
    Conflict,
    Corrupt,
    PermissionDenied,
    StorageFull,
    Cancelled,
    Timeout,
    DecodeFailed,
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDisplayName => "artifact display name is invalid",
            Self::InvalidDigest => "artifact digest is invalid",
            Self::InvalidLimits => "artifact limits are invalid",
            Self::InvalidMetadata => "artifact metadata is invalid",
            Self::InvalidState => "artifact state transition is invalid",
            Self::ItemQuotaExceeded => "artifact exceeds the item quota",
            Self::SessionQuotaExceeded => "artifact exceeds the session quota",
            Self::GlobalQuotaExceeded => "artifact exceeds the global quota",
            Self::CountQuotaExceeded => "artifact count quota is reached",
            Self::SourceChanged => "artifact source changed while importing",
            Self::UnsupportedSource => "artifact source must be a regular file",
            Self::UnsafeEntry => "artifact storage contains an unsafe entry",
            Self::Unavailable => "artifact is unavailable",
            Self::Conflict => "artifact destination already exists",
            Self::Corrupt => "artifact data is corrupt",
            Self::PermissionDenied => "artifact operation is not permitted",
            Self::StorageFull => "artifact storage is full",
            Self::Cancelled => "artifact operation was cancelled",
            Self::Timeout => "artifact operation timed out",
            Self::DecodeFailed => "artifact preview could not be decoded",
        })
    }
}

impl std::error::Error for ArtifactError {}

fn sanitize_display_name(value: &str) -> String {
    let value = value.trim();
    let mut sanitized = String::new();
    for grapheme in value
        .graphemes(true)
        .take(MAX_ARTIFACT_DISPLAY_NAME_GRAPHEMES)
    {
        for character in grapheme.chars() {
            if character == '/'
                || character == '\\'
                || character.is_control()
                || is_directional_control(character)
            {
                sanitized.push('_');
            } else {
                sanitized.push(character);
            }
        }
    }
    let sanitized = sanitized.trim();
    if sanitized == "." || sanitized == ".." {
        "artifact".to_string()
    } else {
        sanitized.to_string()
    }
}

fn is_directional_control(value: char) -> bool {
    matches!(
        value,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn metadata(
        media_type: ArtifactMediaType,
        preview_kind: ArtifactPreviewKind,
    ) -> ArtifactMetadata {
        ArtifactMetadata {
            id: ArtifactId::from_uuid(Uuid::from_u128(1)),
            scope: ArtifactScope {
                session_id: HostedSessionId::from_uuid(Uuid::from_u128(2)),
            },
            display_name: ArtifactDisplayName::new("result.txt").unwrap(),
            origin: ArtifactOrigin::ExplicitImport,
            media_type,
            byte_len: 12,
            sha256: ArtifactSha256::new([7; 32]),
            created_at: 3,
            preview_kind,
            state: ArtifactState::Ready,
        }
    }

    #[test]
    fn artifact_ids_and_digests_round_trip_canonically() {
        let id = ArtifactId::from_uuid(Uuid::from_u128(9));
        assert_eq!(id.to_string().parse(), Ok(id));
        let digest = ArtifactSha256::new([0xab; 32]);
        assert_eq!(digest.to_string().parse(), Ok(digest));
        assert_eq!(digest.short_label(), "abababababab");
        let encoded = serde_json::to_string(&digest).unwrap();
        assert_eq!(
            serde_json::from_str::<ArtifactSha256>(&encoded).unwrap(),
            digest
        );
    }

    #[test]
    fn artifact_names_strip_paths_controls_and_directional_overrides() {
        let name = ArtifactDisplayName::new(" ../secret\\name\u{202e}.txt\n ").unwrap();
        assert_eq!(name.as_str(), ".._secret_name_.txt");
        assert!(!format!("{name:?}").contains("secret"));
        assert!(ArtifactDisplayName::new(" \n ").is_err());
        assert_eq!(ArtifactDisplayName::new("..").unwrap().as_str(), "artifact");
    }

    #[test]
    fn artifact_metadata_and_limits_fail_closed() {
        let limits = ArtifactLimits::default();
        assert!(
            metadata(ArtifactMediaType::ImagePng, ArtifactPreviewKind::Raster)
                .validate(limits)
                .is_ok()
        );
        assert_eq!(
            metadata(ArtifactMediaType::ImagePng, ArtifactPreviewKind::Text).validate(limits),
            Err(ArtifactError::InvalidMetadata)
        );
        let mut raised = limits;
        raised.item_bytes = MAX_ARTIFACT_BYTES + 1;
        assert_eq!(raised.validate(), Err(ArtifactError::InvalidLimits));
    }

    #[test]
    fn artifact_debug_redacts_name_and_digest() {
        let value = metadata(ArtifactMediaType::TextPlainUtf8, ArtifactPreviewKind::Text);
        let debug = format!("{value:?}");
        assert!(!debug.contains("result.txt"));
        assert!(!debug.contains("070707"));
    }
}
