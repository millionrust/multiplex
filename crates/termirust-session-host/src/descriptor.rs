use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{Read, Take};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

use serde::{Deserialize, Serialize};
use termirust_domain::{
    HostInstanceId, HostedSessionId, OccupantGeneration, RuntimeDetectionResult,
    RuntimeDetectionStatus,
};
use termirust_store::JournalLimits;

use crate::{HostError, HostErrorCode};

pub const MAX_DESCRIPTOR_BYTES: u64 = 64 * 1024;
const MAX_ARGUMENTS: usize = 128;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_ENVIRONMENT_ENTRIES: usize = 128;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopDeadlines {
    pub interrupt_millis: u64,
    pub terminate_millis: u64,
    pub total_millis: u64,
}

impl Default for StopDeadlines {
    fn default() -> Self {
        Self {
            interrupt_millis: 2_000,
            terminate_millis: 2_000,
            total_millis: 5_000,
        }
    }
}

impl StopDeadlines {
    pub fn validate(self) -> Result<Self, HostError> {
        if self.interrupt_millis > self.terminate_millis
            || self.terminate_millis > self.total_millis
            || self.total_millis == 0
            || self.total_millis > 5_000
        {
            return Err(HostError::new(HostErrorCode::DescriptorInvalid));
        }
        Ok(self)
    }

    pub fn interrupt(self) -> Duration {
        Duration::from_millis(self.interrupt_millis)
    }

    pub fn terminate(self) -> Duration {
        Duration::from_millis(self.terminate_millis)
    }

    pub fn total(self) -> Duration {
        Duration::from_millis(self.total_millis)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct LaunchDescriptor {
    pub format_version: u16,
    pub session_id: HostedSessionId,
    pub host_instance_id: HostInstanceId,
    #[serde(default)]
    pub expected_occupant_generation: Option<OccupantGeneration>,
    pub runtime_root: PathBuf,
    pub session_dir: PathBuf,
    pub executable: PathBuf,
    #[serde(default)]
    pub runtime_detection: Option<RuntimeDetectionResult>,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub columns: u16,
    pub rows: u16,
    pub journal_limits: JournalLimits,
    pub stop_deadlines: StopDeadlines,
}

impl fmt::Debug for LaunchDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchDescriptor")
            .field("format_version", &self.format_version)
            .field("session_id", &self.session_id)
            .field("host_instance_id", &self.host_instance_id)
            .field(
                "expected_occupant_generation",
                &self.expected_occupant_generation,
            )
            .field("runtime_root", &"[REDACTED]")
            .field("session_dir", &"[REDACTED]")
            .field("executable", &"[REDACTED]")
            .field(
                "runtime_detection",
                &self.runtime_detection.as_ref().map(|detection| {
                    (
                        detection.runtime_id.as_str(),
                        detection.descriptor_version,
                        detection.status,
                    )
                }),
            )
            .field(
                "arguments",
                &format_args!("{} entries", self.arguments.len()),
            )
            .field(
                "environment",
                &format_args!("{} entries", self.environment.len()),
            )
            .field("cwd", &self.cwd.as_ref().map(|_| "[REDACTED]"))
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("journal_limits", &self.journal_limits)
            .field("stop_deadlines", &self.stop_deadlines)
            .finish()
    }
}

impl LaunchDescriptor {
    pub const FORMAT_VERSION: u16 = 1;

    pub fn validate(&self) -> Result<(), HostError> {
        if self.format_version != Self::FORMAT_VERSION
            || self.columns == 0
            || self.rows == 0
            || self.columns > 1_000
            || self.rows > 1_000
            || self
                .expected_occupant_generation
                .is_some_and(|generation| generation == OccupantGeneration::ZERO)
            || self.arguments.len() > MAX_ARGUMENTS
            || self.environment.len() > MAX_ENVIRONMENT_ENTRIES
            || !self.runtime_root.is_absolute()
            || !self.session_dir.is_absolute()
            || !self.executable.is_absolute()
            || self.cwd.as_ref().is_some_and(|path| !path.is_absolute())
            || self.runtime_detection.as_ref().is_some_and(|detection| {
                detection.descriptor_version == 0
                    || detection.status != RuntimeDetectionStatus::Available
                    || detection.fingerprint.is_none()
                    || detection.safe_version.as_ref().is_none_or(|version| {
                        version.is_empty()
                            || version.len() > termirust_domain::MAX_RUNTIME_VERSION_BYTES
                            || !version
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || byte == b'.')
                    })
                    || detection.capabilities.is_empty()
                    || detection.capabilities.len() > 8
                    || detection.diagnostic_code.is_some()
            })
        {
            return Err(HostError::new(HostErrorCode::DescriptorInvalid));
        }
        self.journal_limits
            .validate()
            .map_err(|_| HostError::new(HostErrorCode::DescriptorInvalid))?;
        self.stop_deadlines.validate()?;
        let argument_bytes = self.arguments.iter().try_fold(0_usize, |total, argument| {
            if argument.as_bytes().contains(&0) || argument.len() > MAX_ARGUMENT_BYTES {
                return None;
            }
            total.checked_add(argument.len())
        });
        if argument_bytes.is_none_or(|value| value > MAX_DESCRIPTOR_BYTES as usize) {
            return Err(HostError::new(HostErrorCode::DescriptorInvalid));
        }
        let environment_bytes =
            self.environment
                .iter()
                .try_fold(0_usize, |total, (name, value)| {
                    if !valid_environment_name(name)
                        || value.as_bytes().contains(&0)
                        || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
                    {
                        return None;
                    }
                    total.checked_add(name.len())?.checked_add(value.len())
                });
        if environment_bytes.is_none_or(|value| value > MAX_DESCRIPTOR_BYTES as usize) {
            return Err(HostError::new(HostErrorCode::DescriptorInvalid));
        }
        validate_directory_parent(&self.runtime_root)?;
        validate_directory_parent(&self.session_dir)?;
        validate_executable(&self.executable)?;
        if let Some(cwd) = &self.cwd {
            let metadata = fs::symlink_metadata(cwd).map_err(HostError::io)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(HostError::new(HostErrorCode::DescriptorInvalid));
            }
        }
        Ok(())
    }

    pub fn read(reader: impl Read) -> Result<Self, HostError> {
        let mut bytes = Vec::new();
        let mut bounded: Take<_> = reader.take(MAX_DESCRIPTOR_BYTES + 1);
        bounded.read_to_end(&mut bytes).map_err(HostError::io)?;
        if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
            return Err(HostError::new(HostErrorCode::DescriptorTooLarge));
        }
        let descriptor: Self = serde_json::from_slice(&bytes)
            .map_err(|_| HostError::new(HostErrorCode::DescriptorInvalid))?;
        descriptor.validate()?;
        Ok(descriptor)
    }
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|value| value == b'_' || value.is_ascii_alphabetic())
        && bytes.all(|value| value == b'_' || value.is_ascii_alphanumeric())
}

fn validate_directory_parent(path: &Path) -> Result<(), HostError> {
    let parent = path
        .parent()
        .ok_or_else(|| HostError::new(HostErrorCode::DescriptorInvalid))?;
    let metadata = fs::symlink_metadata(parent).map_err(HostError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(HostError::new(HostErrorCode::DescriptorInvalid));
    }
    Ok(())
}

fn validate_executable(path: &Path) -> Result<(), HostError> {
    let metadata = fs::symlink_metadata(path).map_err(HostError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(HostError::new(HostErrorCode::DescriptorInvalid));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o111 == 0 || metadata.uid() == u32::MAX {
        return Err(HostError::new(HostErrorCode::PermissionDenied));
    }
    Ok(())
}

#[cfg(unix)]
pub fn stdin_is_pipe() -> Result<bool, HostError> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstat(libc::STDIN_FILENO, stat.as_mut_ptr()) };
    if result != 0 {
        return Err(HostError::io(std::io::Error::last_os_error()));
    }
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_mode & libc::S_IFMT) == libc::S_IFIFO)
}

#[cfg(not(unix))]
pub fn stdin_is_pipe() -> Result<bool, HostError> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_debug_redacts_paths_arguments_and_environment() {
        let fixture = tempfile::tempdir().unwrap();
        let descriptor = LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id: HostedSessionId::new(),
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: fixture.path().join("runtime"),
            session_dir: fixture.path().join("session"),
            executable: PathBuf::from("/bin/sh"),
            runtime_detection: None,
            arguments: vec!["canary-argument".to_string()],
            environment: BTreeMap::from([("TOKEN".to_string(), "canary-secret".to_string())]),
            cwd: Some(fixture.path().to_path_buf()),
            columns: 80,
            rows: 24,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        };
        let debug = format!("{descriptor:?}");
        assert!(!debug.contains("canary"));
        assert!(!debug.contains(fixture.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn descriptor_reader_rejects_oversize_and_invalid_environment() {
        assert_eq!(
            LaunchDescriptor::read(vec![b' '; MAX_DESCRIPTOR_BYTES as usize + 1].as_slice())
                .unwrap_err()
                .code,
            HostErrorCode::DescriptorTooLarge
        );
        assert!(!valid_environment_name("1BAD"));
        assert!(valid_environment_name("TERMIRUST_TEST"));
    }
}
