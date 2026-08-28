use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_FILES, DEFAULT_RETENTION_DAYS, Diagnostic, DiagnosticCode,
    DiagnosticMessageId, RecoveryAction, SafeField, SafeValue, Severity,
};

pub(crate) const MARKER_NAME: &str = ".termirust-diagnostics-v1";
pub(crate) const STAGING_DIR_NAME: &str = ".staging";
pub(crate) const MAX_ENTRY_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPolicy {
    pub enabled: bool,
    pub max_file_bytes: u64,
    pub max_files: u8,
    pub retention_days: u8,
    pub channel_capacity: usize,
}

impl Default for DiagnosticPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_files: DEFAULT_MAX_FILES,
            retention_days: DEFAULT_RETENTION_DAYS,
            channel_capacity: 256,
        }
    }
}

impl DiagnosticPolicy {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.max_file_bytes = self.max_file_bytes.clamp(1024, DEFAULT_MAX_FILE_BYTES);
        self.max_files = self.max_files.clamp(1, DEFAULT_MAX_FILES);
        self.retention_days = self.retention_days.clamp(1, DEFAULT_RETENTION_DAYS);
        self.channel_capacity = self.channel_capacity.clamp(1, 4096);
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticUsage {
    pub files: u8,
    pub bytes: u64,
    pub oldest_unix_ms: Option<u64>,
    pub newest_unix_ms: Option<u64>,
}

pub(crate) struct Writer {
    root: PathBuf,
    policy: DiagnosticPolicy,
    last_retention_check_ms: u64,
}

impl Writer {
    pub(crate) fn open(root: PathBuf, policy: DiagnosticPolicy) -> io::Result<Self> {
        let mut writer = Self {
            root,
            policy: policy.normalized(),
            last_retention_check_ms: 0,
        };
        writer.initialize_root()?;
        writer.cleanup_stale_staging()?;
        writer.enforce_retention(current_unix_ms())?;
        Ok(writer)
    }

    pub(crate) fn policy(&self) -> DiagnosticPolicy {
        self.policy
    }

    pub(crate) fn set_policy(&mut self, policy: DiagnosticPolicy) -> io::Result<()> {
        self.policy = policy.normalized();
        self.enforce_retention(current_unix_ms())
    }

    pub(crate) fn append(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        if !self.policy.enabled {
            return Ok(());
        }
        let now_ms = current_unix_ms();
        if now_ms.saturating_sub(self.last_retention_check_ms) >= 3_600_000 {
            self.enforce_retention(now_ms)?;
        }
        diagnostic
            .validate()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unsafe diagnostic schema"))?;
        let mut bytes = serde_json::to_vec(diagnostic).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "diagnostic encoding failed")
        })?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic entry exceeds limit",
            ));
        }
        let active = self.log_path(0);
        let current_len = fs::metadata(&active).map_or(0, |metadata| metadata.len());
        if current_len > 0
            && current_len.saturating_add(bytes.len() as u64) > self.policy.max_file_bytes
        {
            self.rotate()?;
        }
        let mut file = private_open_append(&active)?;
        file.write_all(&bytes)?;
        file.flush()
    }

    pub(crate) fn append_dropped(
        &mut self,
        count: u64,
        occurred_at_unix_ms: u64,
    ) -> io::Result<()> {
        if count == 0 {
            return Ok(());
        }
        let mut diagnostic = Diagnostic::new(
            occurred_at_unix_ms,
            DiagnosticCode::EventsDropped,
            Severity::Warning,
            DiagnosticMessageId::DiagnosticsDropping,
        )
        .with_recovery([RecoveryAction::OpenDiagnosticsSettings]);
        diagnostic
            .insert(SafeField::DroppedCount, SafeValue::Count(count))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "dropped count rejected"))?;
        self.append(&diagnostic)
    }

    pub(crate) fn clear(&self) -> io::Result<()> {
        require_marker(&self.root)?;
        for index in 0..DEFAULT_MAX_FILES {
            remove_file_if_present(&self.log_path(index))?;
        }
        self.cleanup_stale_staging()
    }

    fn initialize_root(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        set_private_dir(&self.root)?;
        let marker = self.root.join(MARKER_NAME);
        if !marker.exists() {
            let mut file = private_create_new(&marker)?;
            file.write_all(b"termirust-diagnostics-v1\n")?;
            file.sync_all()?;
        }
        Ok(())
    }

    fn cleanup_stale_staging(&self) -> io::Result<()> {
        require_marker(&self.root)?;
        for index in 0..DEFAULT_MAX_FILES {
            remove_file_if_present(&self.retention_temp_path(index))?;
        }
        let staging = self.root.join(STAGING_DIR_NAME);
        if staging.exists() {
            let metadata = fs::symlink_metadata(&staging)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "invalid diagnostics staging directory",
                ));
            }
            fs::remove_dir_all(&staging)?;
        }
        Ok(())
    }

    fn enforce_retention(&mut self, now_ms: u64) -> io::Result<()> {
        let cutoff = now_ms.saturating_sub(u64::from(self.policy.retention_days) * 86_400_000);
        for index in 0..DEFAULT_MAX_FILES {
            let path = self.log_path(index);
            if !path.exists() {
                continue;
            }
            let entries = read_diagnostics(&path)?;
            let retained: Vec<_> = entries
                .iter()
                .filter(|entry| entry.occurred_at_unix_ms >= cutoff)
                .collect();
            if retained.len() == entries.len() {
                continue;
            }
            if retained.is_empty() {
                fs::remove_file(&path)?;
                continue;
            }
            let temp = self.retention_temp_path(index);
            remove_file_if_present(&temp)?;
            let mut output = private_create_new(&temp)?;
            for diagnostic in retained {
                serde_json::to_writer(&mut output, diagnostic).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "diagnostic encoding failed")
                })?;
                output.write_all(b"\n")?;
            }
            output.sync_all()?;
            fs::rename(&temp, &path)?;
        }
        self.last_retention_check_ms = now_ms;
        Ok(())
    }

    fn rotate(&self) -> io::Result<()> {
        let last = self.policy.max_files.saturating_sub(1);
        remove_file_if_present(&self.log_path(last))?;
        for index in (1..=last).rev() {
            let source = self.log_path(index - 1);
            if source.exists() {
                fs::rename(source, self.log_path(index))?;
            }
        }
        Ok(())
    }

    fn log_path(&self, index: u8) -> PathBuf {
        self.root.join(format!("diagnostics-{index}.jsonl"))
    }

    fn retention_temp_path(&self, index: u8) -> PathBuf {
        self.root
            .join(format!(".diagnostics-{index}.retention.tmp"))
    }
}

pub(crate) fn usage(root: &Path) -> io::Result<DiagnosticUsage> {
    require_marker(root)?;
    let mut result = DiagnosticUsage::default();
    for index in 0..DEFAULT_MAX_FILES {
        let path = root.join(format!("diagnostics-{index}.jsonl"));
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "diagnostic log is not a regular file",
            ));
        }
        result.files = result.files.saturating_add(1);
        result.bytes = result.bytes.saturating_add(metadata.len());
    }
    Ok(result)
}

pub(crate) fn require_marker(root: &Path) -> io::Result<()> {
    let marker = root.join(MARKER_NAME);
    let metadata = fs::symlink_metadata(marker)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "diagnostics ownership marker is invalid",
        ));
    }
    Ok(())
}

fn read_diagnostics(path: &Path) -> io::Result<Vec<Diagnostic>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > DEFAULT_MAX_FILE_BYTES.saturating_add(MAX_ENTRY_BYTES)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic file is invalid or oversized",
        ));
    }
    let file = File::open(path)?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.len() as u64 > MAX_ENTRY_BYTES || entries.len() >= 100_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic entry limit exceeded",
            ));
        }
        let diagnostic: Diagnostic = serde_json::from_str(&line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "malformed entry"))?;
        diagnostic
            .validate()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unsafe entry"))?;
        entries.push(diagnostic);
    }
    Ok(entries)
}

pub(crate) fn private_create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    set_private_file_options(&mut options);
    options.open(path)
}

pub(crate) fn private_open_append(path: &Path) -> io::Result<File> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "diagnostics path is not a regular file",
            ));
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    set_private_file_options(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn set_private_file_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_options(_: &mut OpenOptions) {}

#[cfg(unix)]
pub(crate) fn set_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn set_private_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn current_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
