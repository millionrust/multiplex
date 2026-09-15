use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::{
    MAX_ENTRY_BYTES, STAGING_DIR_NAME, current_unix_ms, private_create_new, require_marker,
    set_private_dir,
};
use crate::{Diagnostic, DiagnosticCode, MAX_BUNDLE_BYTES, SCHEMA_VERSION};

const MAX_SOURCE_FILES: usize = 5;
const MAX_ENTRIES: usize = 100_000;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportErrorCode {
    RuntimeUnavailable,
    StorageUnavailable,
    PermissionDenied,
    MalformedEntry,
    RedactionUncertain,
    SourceChanged,
    SizeLimit,
    Cancelled,
    DestinationExists,
    InvalidDestination,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportError {
    pub code: ExportErrorCode,
}

impl ExportError {
    pub(crate) const fn runtime_unavailable() -> Self {
        Self {
            code: ExportErrorCode::RuntimeUnavailable,
        }
    }

    #[doc(hidden)]
    pub const fn runtime_unavailable_for_app() -> Self {
        Self::runtime_unavailable()
    }

    pub(crate) const fn storage_unavailable() -> Self {
        Self {
            code: ExportErrorCode::StorageUnavailable,
        }
    }

    pub(crate) fn from_io(error: io::Error) -> Self {
        let code = match error.kind() {
            io::ErrorKind::PermissionDenied => ExportErrorCode::PermissionDenied,
            io::ErrorKind::InvalidData => ExportErrorCode::MalformedEntry,
            _ => ExportErrorCode::StorageUnavailable,
        };
        Self { code }
    }

    const fn new(code: ExportErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "diagnostic export failed: {:?}", self.code)
    }
}

impl std::error::Error for ExportError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportFileManifest {
    pub logical_name: String,
    pub category: String,
    pub entry_count: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticExportManifest {
    pub schema_version: u16,
    pub created_at_unix_ms: u64,
    pub files: Vec<ExportFileManifest>,
    pub categories: Vec<DiagnosticCode>,
    pub total_entries: u64,
    pub total_bytes: u64,
    pub oldest_unix_ms: Option<u64>,
    pub newest_unix_ms: Option<u64>,
    pub redactions: u64,
    pub snapshot_sha256: String,
    pub included_classes: Vec<String>,
    pub excluded_classes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticBundleFile {
    pub logical_name: String,
    pub entries: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticBundle {
    pub schema_version: u16,
    pub manifest: DiagnosticExportManifest,
    pub files: Vec<DiagnosticBundleFile>,
}

#[derive(Clone, Default)]
pub struct ExportCancellation(Arc<AtomicBool>);

impl ExportCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct PreparedExport {
    staging_path: PathBuf,
    preview_id: String,
    manifest: DiagnosticExportManifest,
    published: bool,
}

impl PreparedExport {
    #[must_use]
    pub fn manifest(&self) -> &DiagnosticExportManifest {
        &self.manifest
    }

    #[must_use]
    pub fn preview_id(&self) -> &str {
        &self.preview_id
    }

    pub fn publish(
        mut self,
        destination: impl AsRef<Path>,
        cancellation: &ExportCancellation,
    ) -> Result<DiagnosticExportManifest, ExportError> {
        let destination = destination.as_ref();
        if cancellation.is_cancelled() {
            return Err(ExportError::new(ExportErrorCode::Cancelled));
        }
        if destination.exists() {
            return Err(ExportError::new(ExportErrorCode::DestinationExists));
        }
        let parent = destination
            .parent()
            .filter(|path| path.is_dir())
            .ok_or_else(|| ExportError::new(ExportErrorCode::InvalidDestination))?;
        let temp = parent.join(format!(".termirust-diagnostics-{}.tmp", self.preview_id));
        let result = copy_restricted(&self.staging_path, &temp, cancellation).and_then(|()| {
            fs::hard_link(&temp, destination).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    ExportError::new(ExportErrorCode::DestinationExists)
                } else {
                    ExportError::from_io(error)
                }
            })
        });
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }

        // The hard link publishes the complete temporary file without a replace race.
        // Once it succeeds the destination is valid; cleanup and directory syncing are
        // best effort so a cleanup failure cannot report a failed export after publish.
        self.published = true;
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&self.staging_path);
        let _ = sync_directory(parent);
        Ok(self.manifest.clone())
    }
}

impl Drop for PreparedExport {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.staging_path);
        }
    }
}

pub(crate) fn prepare_export(
    root: &Path,
    cancellation: &ExportCancellation,
) -> Result<PreparedExport, ExportError> {
    check_cancelled(cancellation)?;
    require_marker(root).map_err(ExportError::from_io)?;
    let staging = root.join(STAGING_DIR_NAME);
    if staging.exists() {
        let metadata = fs::symlink_metadata(&staging).map_err(ExportError::from_io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ExportError::new(ExportErrorCode::PermissionDenied));
        }
    } else {
        fs::create_dir(&staging).map_err(ExportError::from_io)?;
    }
    set_private_dir(&staging).map_err(ExportError::from_io)?;

    let mut files = Vec::new();
    let mut source_hashes = Vec::new();
    let mut total_source_bytes = 0_u64;
    for index in 0..MAX_SOURCE_FILES {
        check_cancelled(cancellation)?;
        let path = root.join(format!("diagnostics-{index}.jsonl"));
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ExportError::from_io(error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ExportError::new(ExportErrorCode::PermissionDenied));
        }
        total_source_bytes = total_source_bytes.saturating_add(metadata.len());
        if total_source_bytes > MAX_BUNDLE_BYTES {
            return Err(ExportError::new(ExportErrorCode::SizeLimit));
        }
        let bytes = fs::read(&path).map_err(ExportError::from_io)?;
        source_hashes.push((path, sha256_hex(&bytes)));
        files.push(parse_source(index, &bytes, cancellation)?);
    }

    for (path, expected) in &source_hashes {
        check_cancelled(cancellation)?;
        let current =
            fs::read(path).map_err(|_| ExportError::new(ExportErrorCode::SourceChanged))?;
        if sha256_hex(&current) != *expected {
            return Err(ExportError::new(ExportErrorCode::SourceChanged));
        }
    }

    let manifest = build_manifest(&files)?;
    let bundle = DiagnosticBundle {
        schema_version: SCHEMA_VERSION,
        manifest: manifest.clone(),
        files,
    };
    let encoded = serde_json::to_vec_pretty(&bundle)
        .map_err(|_| ExportError::new(ExportErrorCode::RedactionUncertain))?;
    if encoded.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(ExportError::new(ExportErrorCode::SizeLimit));
    }
    second_pass_scan(&encoded)?;
    check_cancelled(cancellation)?;

    let preview_id = random_id();
    let staging_path = staging.join(format!("prepared-{preview_id}.json"));
    let mut output = private_create_new(&staging_path).map_err(ExportError::from_io)?;
    output.write_all(&encoded).map_err(ExportError::from_io)?;
    output.sync_all().map_err(ExportError::from_io)?;
    sync_directory(&staging).map_err(ExportError::from_io)?;

    Ok(PreparedExport {
        staging_path,
        preview_id,
        manifest,
        published: false,
    })
}

fn parse_source(
    index: usize,
    bytes: &[u8],
    cancellation: &ExportCancellation,
) -> Result<DiagnosticBundleFile, ExportError> {
    let mut entries = Vec::new();
    for line in BufReader::new(bytes).split(b'\n') {
        check_cancelled(cancellation)?;
        let line = line.map_err(ExportError::from_io)?;
        if line.is_empty() {
            continue;
        }
        if line.len() as u64 > MAX_ENTRY_BYTES || entries.len() >= MAX_ENTRIES {
            return Err(ExportError::new(ExportErrorCode::SizeLimit));
        }
        let diagnostic: Diagnostic = serde_json::from_slice(&line)
            .map_err(|_| ExportError::new(ExportErrorCode::MalformedEntry))?;
        diagnostic
            .validate()
            .map_err(|_| ExportError::new(ExportErrorCode::RedactionUncertain))?;
        entries.push(diagnostic);
    }
    Ok(DiagnosticBundleFile {
        logical_name: format!("diagnostics-{index}.jsonl"),
        entries,
    })
}

fn check_cancelled(cancellation: &ExportCancellation) -> Result<(), ExportError> {
    if cancellation.is_cancelled() {
        Err(ExportError::new(ExportErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn build_manifest(files: &[DiagnosticBundleFile]) -> Result<DiagnosticExportManifest, ExportError> {
    let mut file_manifests = Vec::new();
    let mut categories = Vec::new();
    let mut total_entries = 0_u64;
    let mut total_bytes = 0_u64;
    let mut oldest = None;
    let mut newest = None;
    let mut snapshot_hasher = Sha256::new();

    for file in files {
        let canonical = serde_json::to_vec(file)
            .map_err(|_| ExportError::new(ExportErrorCode::RedactionUncertain))?;
        snapshot_hasher.update(&canonical);
        let bytes = canonical.len() as u64;
        total_bytes = total_bytes.saturating_add(bytes);
        total_entries = total_entries.saturating_add(file.entries.len() as u64);
        for diagnostic in &file.entries {
            if !categories.contains(&diagnostic.code) {
                categories.push(diagnostic.code);
            }
            oldest = min_option(oldest, Some(diagnostic.occurred_at_unix_ms));
            newest = max_option(newest, Some(diagnostic.occurred_at_unix_ms));
        }
        file_manifests.push(ExportFileManifest {
            logical_name: file.logical_name.clone(),
            category: "redacted_metadata".into(),
            entry_count: file.entries.len() as u64,
            bytes,
            sha256: sha256_hex(&canonical),
        });
    }
    categories.sort_by_key(|code| format!("{code:?}"));
    Ok(DiagnosticExportManifest {
        schema_version: SCHEMA_VERSION,
        created_at_unix_ms: current_unix_ms(),
        files: file_manifests,
        categories,
        total_entries,
        total_bytes,
        oldest_unix_ms: oldest,
        newest_unix_ms: newest,
        redactions: 0,
        snapshot_sha256: format!("{:x}", snapshot_hasher.finalize()),
        included_classes: vec!["allowlisted operational metadata".into()],
        excluded_classes: vec![
            "terminal input and output".into(),
            "prompts and transcripts".into(),
            "credentials and environment".into(),
            "paths, hostnames, usernames, and device names".into(),
            "artifacts and clipboard content".into(),
        ],
    })
}

fn second_pass_scan(bytes: &[u8]) -> Result<(), ExportError> {
    let lower = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let forbidden = [
        "\"terminal_output\":",
        "\"terminal_input\":",
        "\"startup_command\":",
        "\"environment\":",
        "\"hostname\":",
        "\"username\":",
        "\"device_name\":",
        "\"private_key\":",
        "\"password\":",
        "\"authorization\":",
        "bearer ",
        "-----begin ",
        "/users/",
        "/home/",
        "c:\\\\users\\\\",
    ];
    if forbidden.iter().any(|needle| lower.contains(needle)) {
        return Err(ExportError::new(ExportErrorCode::RedactionUncertain));
    }
    let decoded: DiagnosticBundle = serde_json::from_slice(bytes)
        .map_err(|_| ExportError::new(ExportErrorCode::RedactionUncertain))?;
    if decoded.schema_version != SCHEMA_VERSION || decoded.manifest.schema_version != SCHEMA_VERSION
    {
        return Err(ExportError::new(ExportErrorCode::RedactionUncertain));
    }
    Ok(())
}

fn copy_restricted(
    source: &Path,
    destination: &Path,
    cancellation: &ExportCancellation,
) -> Result<(), ExportError> {
    let mut input = File::open(source).map_err(ExportError::from_io)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination).map_err(ExportError::from_io)?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        if cancellation.is_cancelled() {
            return Err(ExportError::new(ExportErrorCode::Cancelled));
        }
        let read = input.read(&mut buffer).map_err(ExportError::from_io)?;
        if read == 0 {
            break;
        }
        copied = copied.saturating_add(read as u64);
        if copied > MAX_BUNDLE_BYTES {
            return Err(ExportError::new(ExportErrorCode::SizeLimit));
        }
        output
            .write_all(&buffer[..read])
            .map_err(ExportError::from_io)?;
    }
    output.sync_all().map_err(ExportError::from_io)
}

fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn min_option(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn max_option(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}
