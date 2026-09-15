//! Bounded TUF metadata verification with atomic TermiRust trust state.
//!
//! This crate deliberately has no network, installer, process, UI, or target-download API.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use tough::{ExpirationEnforcement, Limits, RepositoryLoader, TargetName};
use url::Url;

pub const MAX_BOOTSTRAP_ROOT_BYTES: u64 = 256 * 1024;
pub const MAX_ROLE_BYTES: u64 = 1024 * 1024;
pub const MAX_DELEGATED_ROLES: usize = 8;
pub const MAX_TARGETS: usize = 10_000;
pub const MAX_METADATA_FILES: usize = 4 + MAX_DELEGATED_ROLES + 32;
pub const MAX_TRUST_STATE_BYTES: u64 = 16 * 1024;
pub const TRUST_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustErrorCode {
    Cancelled,
    ResourceLimit,
    MissingMetadata,
    InvalidMetadata,
    InvalidSignature,
    Tampered,
    Expired,
    Replay,
    Rollback,
    ClockRollback,
    TargetNotFound,
    WrongTarget,
    Incompatible,
    CorruptState,
    NewerState,
    StateIo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustError {
    pub code: TrustErrorCode,
}

impl TrustError {
    #[must_use]
    pub const fn new(code: TrustErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for TrustError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "update trust failed: {:?}", self.code)
    }
}

impl std::error::Error for TrustError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    Stable,
    Beta,
    Nightly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTargetName(String);

impl UpdateTargetName {
    pub fn parse(value: impl Into<String>) -> Result<Self, TrustError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || value.starts_with('/')
            || value.split('/').any(|part| matches!(part, "" | "." | ".."))
            || value.contains('\\')
            || value.chars().any(char::is_control)
        {
            return Err(TrustError::new(TrustErrorCode::WrongTarget));
        }
        TargetName::new(value.clone()).map_err(|_| TrustError::new(TrustErrorCode::WrongTarget))?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityRange {
    pub min: u32,
    pub max: u32,
}

impl CompatibilityRange {
    fn contains(self, version: u32) -> bool {
        self.min <= self.max && (self.min..=self.max).contains(&version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRequest {
    pub target: UpdateTargetName,
    pub channel: UpdateChannel,
    pub platform: String,
    pub arch: String,
    pub store_version: u32,
    pub protocol_version: u32,
}

impl VerificationRequest {
    pub fn new(
        target: UpdateTargetName,
        channel: UpdateChannel,
        platform: impl Into<String>,
        arch: impl Into<String>,
        store_version: u32,
        protocol_version: u32,
    ) -> Result<Self, TrustError> {
        let platform = bounded_identifier(platform.into())?;
        let arch = bounded_identifier(arch.into())?;
        Ok(Self {
            target,
            channel,
            platform,
            arch,
            store_version,
            protocol_version,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTarget {
    pub name: UpdateTargetName,
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub length: u64,
    pub hashes: BTreeMap<String, String>,
    pub store_range: CompatibilityRange,
    pub protocol_range: CompatibilityRange,
    pub rollout: u8,
    pub emergency_rollback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedState {
    pub schema_version: u32,
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub observed_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStateInspection {
    Missing,
    Valid(TrustedState),
    Corrupt(Vec<u8>),
    Newer(Vec<u8>),
    Oversized,
}

pub trait Clock: Send + Sync {
    fn unix_seconds(&self) -> i64;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_seconds(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
    }
}

pub trait TrustStateStore: Send + Sync {
    fn load(&self) -> Result<Option<TrustedState>, TrustError>;
    fn commit(&self, state: &TrustedState) -> Result<(), TrustError>;
}

#[derive(Debug, Clone)]
pub struct FileTrustStateStore {
    path: PathBuf,
}

impl FileTrustStateStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn inspect(&self) -> io::Result<TrustStateInspection> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TrustStateInspection::Missing);
            }
            Err(error) => return Err(error),
        };
        if bytes.len() as u64 > MAX_TRUST_STATE_BYTES {
            return Ok(TrustStateInspection::Oversized);
        }
        match serde_json::from_slice::<TrustedState>(&bytes) {
            Ok(state) if state.schema_version == TRUST_STATE_SCHEMA_VERSION => {
                Ok(TrustStateInspection::Valid(state))
            }
            Ok(_) => Ok(TrustStateInspection::Newer(bytes)),
            Err(_) => Ok(TrustStateInspection::Corrupt(bytes)),
        }
    }
}

impl TrustStateStore for FileTrustStateStore {
    fn load(&self) -> Result<Option<TrustedState>, TrustError> {
        match self
            .inspect()
            .map_err(|_| TrustError::new(TrustErrorCode::StateIo))?
        {
            TrustStateInspection::Missing => Ok(None),
            TrustStateInspection::Valid(state) => Ok(Some(state)),
            TrustStateInspection::Newer(_) => Err(TrustError::new(TrustErrorCode::NewerState)),
            TrustStateInspection::Corrupt(_) | TrustStateInspection::Oversized => {
                Err(TrustError::new(TrustErrorCode::CorruptState))
            }
        }
    }

    fn commit(&self, state: &TrustedState) -> Result<(), TrustError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| TrustError::new(TrustErrorCode::StateIo))?;
        fs::create_dir_all(parent).map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        let mut temporary =
            NamedTempFile::new_in(parent).map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        set_private_permissions(temporary.as_file())?;
        serde_json::to_writer(&mut temporary, state)
            .map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        temporary
            .write_all(b"\n")
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        temporary
            .persist(&self.path)
            .map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_permissions(file: &File) -> Result<(), TrustError> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| TrustError::new(TrustErrorCode::StateIo))
}

#[cfg(not(unix))]
fn set_private_permissions(_: &File) -> Result<(), TrustError> {
    Ok(())
}

#[derive(Debug, Clone)]
pub struct RepositorySource {
    pub trusted_root: PathBuf,
    pub metadata_dir: PathBuf,
}

pub async fn verify_and_commit<S: TrustStateStore, C: Clock>(
    source: &RepositorySource,
    request: &VerificationRequest,
    state_store: &S,
    clock: &C,
    cancellation: &CancellationToken,
) -> Result<VerifiedTarget, TrustError> {
    check_cancelled(cancellation)?;
    let previous = state_store.load()?;
    let now_seconds = clock.unix_seconds();
    if previous
        .as_ref()
        .is_some_and(|state| now_seconds < state.observed_unix_seconds)
    {
        return Err(TrustError::new(TrustErrorCode::ClockRollback));
    }
    let now = Timestamp::new(now_seconds, 0)
        .map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?;
    let root = read_bounded(&source.trusted_root, MAX_BOOTSTRAP_ROOT_BYTES)?;
    preflight_metadata(&source.metadata_dir, now, cancellation)?;

    let metadata_url = directory_url(&source.metadata_dir)?;
    let targets_url = metadata_url.clone();
    let staging = tempfile::tempdir().map_err(|_| TrustError::new(TrustErrorCode::StateIo))?;
    let loader = RepositoryLoader::new(&root, metadata_url, targets_url)
        .datastore(staging.path())
        .expiration_enforcement(ExpirationEnforcement::Safe)
        .limits(Limits {
            max_root_size: MAX_BOOTSTRAP_ROOT_BYTES,
            max_targets_size: MAX_ROLE_BYTES,
            max_timestamp_size: MAX_ROLE_BYTES,
            max_snapshot_size: MAX_ROLE_BYTES,
            max_root_updates: 32,
        });
    let repository = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(TrustError::new(TrustErrorCode::Cancelled)),
        result = loader.load() => result.map_err(map_tough_error)?,
    };
    check_cancelled(cancellation)?;

    let next_state = TrustedState {
        schema_version: TRUST_STATE_SCHEMA_VERSION,
        root_version: repository.root().signed.version.get(),
        timestamp_version: repository.timestamp().signed.version.get(),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        observed_unix_seconds: now_seconds,
    };
    validate_monotonic(previous.as_ref(), &next_state)?;

    if repository.all_targets().count() > MAX_TARGETS {
        return Err(TrustError::new(TrustErrorCode::ResourceLimit));
    }
    let target_name = TargetName::new(request.target.as_str())
        .map_err(|_| TrustError::new(TrustErrorCode::WrongTarget))?;
    let target = repository
        .all_targets()
        .find_map(|(name, target)| (name == &target_name).then_some(target))
        .ok_or_else(|| TrustError::new(TrustErrorCode::TargetNotFound))?;
    let verified = parse_target(request, target)?;
    check_cancelled(cancellation)?;
    state_store.commit(&next_state)?;
    Ok(verified)
}

fn validate_monotonic(
    previous: Option<&TrustedState>,
    next: &TrustedState,
) -> Result<(), TrustError> {
    let Some(previous) = previous else {
        return Ok(());
    };
    if next.root_version < previous.root_version
        || next.timestamp_version < previous.timestamp_version
        || next.snapshot_version < previous.snapshot_version
        || next.targets_version < previous.targets_version
    {
        return Err(TrustError::new(TrustErrorCode::Rollback));
    }
    if next.timestamp_version == previous.timestamp_version {
        return Err(TrustError::new(TrustErrorCode::Replay));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TermiRustTargetMetadata {
    schema_version: u32,
    version: String,
    channel: UpdateChannel,
    platform: String,
    arch: String,
    store_range: CompatibilityRange,
    protocol_range: CompatibilityRange,
    rollout: u8,
    emergency_rollback: bool,
}

fn parse_target(
    request: &VerificationRequest,
    target: &tough::schema::Target,
) -> Result<VerifiedTarget, TrustError> {
    if target.custom.len() != 1 {
        return Err(TrustError::new(TrustErrorCode::InvalidMetadata));
    }
    let custom = target
        .custom
        .get("termirust")
        .cloned()
        .ok_or_else(|| TrustError::new(TrustErrorCode::InvalidMetadata))?;
    let metadata: TermiRustTargetMetadata = serde_json::from_value(custom)
        .map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?;
    if metadata.schema_version != 1
        || metadata.version.is_empty()
        || metadata.version.len() > 64
        || metadata.version.chars().any(char::is_control)
        || metadata.rollout > 100
    {
        return Err(TrustError::new(TrustErrorCode::InvalidMetadata));
    }
    if metadata.channel != request.channel
        || metadata.platform != request.platform
        || metadata.arch != request.arch
    {
        return Err(TrustError::new(TrustErrorCode::WrongTarget));
    }
    if !metadata.store_range.contains(request.store_version)
        || !metadata.protocol_range.contains(request.protocol_version)
    {
        return Err(TrustError::new(TrustErrorCode::Incompatible));
    }
    let sha256 = target.hashes.sha256.as_ref();
    if sha256.len() != 32 {
        return Err(TrustError::new(TrustErrorCode::InvalidMetadata));
    }
    let mut hashes = BTreeMap::new();
    hashes.insert("sha256".to_string(), hex::encode(sha256));
    Ok(VerifiedTarget {
        name: request.target.clone(),
        version: metadata.version,
        platform: metadata.platform,
        arch: metadata.arch,
        length: target.length,
        hashes,
        store_range: metadata.store_range,
        protocol_range: metadata.protocol_range,
        rollout: metadata.rollout,
        emergency_rollback: metadata.emergency_rollback,
    })
}

fn preflight_metadata(
    directory: &Path,
    now: Timestamp,
    cancellation: &CancellationToken,
) -> Result<(), TrustError> {
    let entries =
        fs::read_dir(directory).map_err(|_| TrustError::new(TrustErrorCode::MissingMetadata))?;
    let mut files = 0_usize;
    let mut delegated_roles = 0_usize;
    let mut targets = 0_usize;
    let mut top_level = [false; 3];
    for entry in entries {
        check_cancelled(cancellation)?;
        let entry = entry.map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?;
        if !entry
            .file_type()
            .map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?
            .is_file()
        {
            return Err(TrustError::new(TrustErrorCode::InvalidMetadata));
        }
        files += 1;
        if files > MAX_METADATA_FILES {
            return Err(TrustError::new(TrustErrorCode::ResourceLimit));
        }
        let bytes = read_bounded(&entry.path(), MAX_ROLE_BYTES)?;
        let document: Value = serde_json::from_slice(&bytes)
            .map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?;
        let signed = document
            .get("signed")
            .and_then(Value::as_object)
            .ok_or_else(|| TrustError::new(TrustErrorCode::InvalidMetadata))?;
        let role = signed
            .get("_type")
            .and_then(Value::as_str)
            .ok_or_else(|| TrustError::new(TrustErrorCode::InvalidMetadata))?;
        if role == "root" {
            if bytes.len() as u64 > MAX_BOOTSTRAP_ROOT_BYTES {
                return Err(TrustError::new(TrustErrorCode::ResourceLimit));
            }
        } else {
            let expires = signed
                .get("expires")
                .and_then(Value::as_str)
                .ok_or_else(|| TrustError::new(TrustErrorCode::InvalidMetadata))?
                .parse::<Timestamp>()
                .map_err(|_| TrustError::new(TrustErrorCode::InvalidMetadata))?;
            if expires <= now {
                return Err(TrustError::new(TrustErrorCode::Expired));
            }
        }
        match role {
            "root" => {}
            "timestamp" => top_level[0] = true,
            "snapshot" => top_level[1] = true,
            "targets" => {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name == "targets.json" || file_name.ends_with(".targets.json") {
                    top_level[2] = true;
                } else {
                    delegated_roles += 1;
                    if delegated_roles > MAX_DELEGATED_ROLES {
                        return Err(TrustError::new(TrustErrorCode::ResourceLimit));
                    }
                }
                targets = targets.saturating_add(
                    signed
                        .get("targets")
                        .and_then(Value::as_object)
                        .map_or(0, serde_json::Map::len),
                );
                if targets > MAX_TARGETS {
                    return Err(TrustError::new(TrustErrorCode::ResourceLimit));
                }
            }
            _ => return Err(TrustError::new(TrustErrorCode::InvalidMetadata)),
        }
    }
    if top_level.into_iter().all(|present| present) {
        Ok(())
    } else {
        Err(TrustError::new(TrustErrorCode::MissingMetadata))
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, TrustError> {
    let metadata =
        fs::metadata(path).map_err(|_| TrustError::new(TrustErrorCode::MissingMetadata))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(TrustError::new(TrustErrorCode::ResourceLimit));
    }
    fs::read(path).map_err(|_| TrustError::new(TrustErrorCode::MissingMetadata))
}

fn directory_url(path: &Path) -> Result<Url, TrustError> {
    let absolute =
        fs::canonicalize(path).map_err(|_| TrustError::new(TrustErrorCode::MissingMetadata))?;
    Url::from_directory_path(absolute)
        .map_err(|()| TrustError::new(TrustErrorCode::InvalidMetadata))
}

fn bounded_identifier(value: String) -> Result<String, TrustError> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Err(TrustError::new(TrustErrorCode::WrongTarget))
    } else {
        Ok(value)
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), TrustError> {
    if cancellation.is_cancelled() {
        Err(TrustError::new(TrustErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn map_tough_error(error: tough::error::Error) -> TrustError {
    use tough::error::Error;
    let code = match error {
        Error::ExpiredMetadata { .. } => TrustErrorCode::Expired,
        Error::OlderMetadata { .. }
        | Error::OlderSnapshotInTimestamp { .. }
        | Error::SnapshotRoleRollback { .. }
        | Error::SystemTimeSteppedBackward { .. } => TrustErrorCode::Rollback,
        Error::MaxSizeExceeded { .. } | Error::MaxUpdatesExceeded { .. } => {
            TrustErrorCode::ResourceLimit
        }
        Error::HashMismatch { .. }
        | Error::VersionMismatch { .. }
        | Error::TimestampMetaLength { .. } => TrustErrorCode::Tampered,
        Error::VerifyMetadata { .. }
        | Error::VerifyRoleMetadata { .. }
        | Error::VerifyTrustedMetadata { .. }
        | Error::InvalidThreshold { .. }
        | Error::DuplicateKeyid { .. }
        | Error::NoKeys { .. }
        | Error::NoRoleKeysinRoot { .. } => TrustErrorCode::InvalidSignature,
        Error::Transport { source, .. } => match source.kind() {
            tough::TransportErrorKind::FileNotFound => TrustErrorCode::MissingMetadata,
            tough::TransportErrorKind::UnsupportedUrlScheme => TrustErrorCode::InvalidMetadata,
            tough::TransportErrorKind::Other => TrustErrorCode::Tampered,
            _ => TrustErrorCode::InvalidMetadata,
        },
        Error::MetaMissing { .. } => TrustErrorCode::MissingMetadata,
        _ => TrustErrorCode::InvalidMetadata,
    };
    TrustError::new(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tough::schema::{Hashes, Target};

    #[test]
    fn target_name_rejects_ambiguous_paths() {
        for invalid in ["", "/absolute", "a/../b", "a//b", "a\\b"] {
            assert_eq!(
                UpdateTargetName::parse(invalid).unwrap_err().code,
                TrustErrorCode::WrongTarget
            );
        }
        assert_eq!(
            UpdateTargetName::parse("stable/macos/aarch64/termirust.tar.zst")
                .unwrap()
                .as_str(),
            "stable/macos/aarch64/termirust.tar.zst"
        );
    }

    #[test]
    fn compatibility_ranges_are_closed_and_ordered() {
        assert!(CompatibilityRange { min: 2, max: 4 }.contains(2));
        assert!(CompatibilityRange { min: 2, max: 4 }.contains(4));
        assert!(!CompatibilityRange { min: 4, max: 2 }.contains(3));
    }

    #[test]
    fn target_custom_metadata_rejects_unknown_and_malformed_fields() {
        let request = VerificationRequest::new(
            UpdateTargetName::parse("stable/macos/aarch64/update").unwrap(),
            UpdateChannel::Stable,
            "macos",
            "aarch64",
            1,
            1,
        )
        .unwrap();
        let valid = serde_json::json!({
            "schema_version": 1,
            "version": "1.0.0",
            "channel": "stable",
            "platform": "macos",
            "arch": "aarch64",
            "store_range": { "min": 1, "max": 2 },
            "protocol_range": { "min": 1, "max": 2 },
            "rollout": 100,
            "emergency_rollback": false
        });
        let make_target = |custom: Value, hash: Vec<u8>| Target {
            length: 1,
            hashes: Hashes {
                sha256: hash.into(),
                _extra: HashMap::new(),
            },
            custom: HashMap::from([("termirust".to_string(), custom)]),
            _extra: HashMap::new(),
        };
        assert!(parse_target(&request, &make_target(valid.clone(), vec![0; 32])).is_ok());

        let mut unknown = valid.clone();
        unknown["required_future_field"] = Value::Bool(true);
        assert_eq!(
            parse_target(&request, &make_target(unknown, vec![0; 32]))
                .unwrap_err()
                .code,
            TrustErrorCode::InvalidMetadata
        );
        let mut rollout = valid.clone();
        rollout["rollout"] = Value::from(101);
        assert_eq!(
            parse_target(&request, &make_target(rollout, vec![0; 32]))
                .unwrap_err()
                .code,
            TrustErrorCode::InvalidMetadata
        );
        assert_eq!(
            parse_target(&request, &make_target(valid, vec![0; 31]))
                .unwrap_err()
                .code,
            TrustErrorCode::InvalidMetadata
        );
    }
}
