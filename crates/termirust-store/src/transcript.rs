use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest as _, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use termirust_domain::{
    ExportManifest, TranscriptCancellation, TranscriptCategorySet, TranscriptKind,
    TranscriptLimits, TranscriptPage, escape_markdown_text,
    render_transcript_entry_markdown_with_label,
};

const MARKDOWN_FILE: &str = "transcript.md";
const MANIFEST_FILE: &str = "manifest.json";
const MAX_EXPORT_LABEL_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptExportSourceSummary {
    pub skipped_count: u64,
    pub redaction_count: u64,
    pub deterministic_source_hash: Option<String>,
}

pub trait TranscriptPageStream {
    fn next_page(
        &mut self,
        cancellation: &TranscriptCancellation,
    ) -> Result<Option<TranscriptPage>, TranscriptExportError>;

    fn summary(&self) -> TranscriptExportSourceSummary;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptExportLabels {
    pub title: String,
    pub categories: BTreeMap<TranscriptKind, String>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TranscriptExportSpec {
    pub destination: PathBuf,
    pub provider_contract: String,
    pub categories: TranscriptCategorySet,
    pub limits: TranscriptLimits,
    pub expected_source_hash: String,
    pub labels: TranscriptExportLabels,
}

impl fmt::Debug for TranscriptExportSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TranscriptExportSpec")
            .field("destination", &"<redacted>")
            .field("provider_contract", &self.provider_contract)
            .field("categories", &self.categories)
            .field("limits", &self.limits)
            .field("expected_source_hash", &"<redacted>")
            .field("labels", &self.labels)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptExportResult {
    pub manifest: ExportManifest,
    pub markdown_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptExportError {
    Cancelled,
    DestinationConflict,
    InvalidEntry,
    InvalidSpec,
    Io(io::ErrorKind),
    ResourceLimit,
    SourceChanged,
    UnsafePath,
}

pub fn export_transcript(
    spec: &TranscriptExportSpec,
    stream: &mut impl TranscriptPageStream,
    cancellation: &TranscriptCancellation,
) -> Result<TranscriptExportResult, TranscriptExportError> {
    export_transcript_with_fault(spec, stream, cancellation, None)
}

fn export_transcript_with_fault(
    spec: &TranscriptExportSpec,
    stream: &mut impl TranscriptPageStream,
    cancellation: &TranscriptCancellation,
    fail_write_after: Option<usize>,
) -> Result<TranscriptExportResult, TranscriptExportError> {
    validate_spec(spec)?;
    cancellation
        .check()
        .map_err(|_| TranscriptExportError::Cancelled)?;
    let parent = spec
        .destination
        .parent()
        .ok_or(TranscriptExportError::UnsafePath)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(map_io_error)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(TranscriptExportError::UnsafePath);
    }
    match fs::symlink_metadata(&spec.destination) {
        Ok(_) => return Err(TranscriptExportError::DestinationConflict),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(map_io_error(error)),
    }
    let staging = staging_path(&spec.destination)?;
    create_private_directory(&staging)?;
    let result = export_into_staging(spec, stream, cancellation, &staging, fail_write_after)
        .and_then(|result| {
            cancellation
                .check()
                .map_err(|_| TranscriptExportError::Cancelled)?;
            rename_no_replace(&staging, &spec.destination).map_err(map_io_error)?;
            sync_directory(parent)?;
            Ok(result)
        });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn export_into_staging(
    spec: &TranscriptExportSpec,
    stream: &mut impl TranscriptPageStream,
    cancellation: &TranscriptCancellation,
    staging: &Path,
    fail_write_after: Option<usize>,
) -> Result<TranscriptExportResult, TranscriptExportError> {
    let markdown_path = staging.join(MARKDOWN_FILE);
    let mut markdown = create_private_file(&markdown_path)?;
    let mut content_hasher = Sha256::new();
    let mut markdown_bytes = 0usize;
    let header = format!("# {}\n\n", escape_markdown_text(&spec.labels.title));
    write_bounded(
        &mut markdown,
        header.as_bytes(),
        &mut content_hasher,
        &mut markdown_bytes,
        spec.limits.output_bytes,
        fail_write_after,
    )?;
    let mut entry_count = 0u64;
    let mut last_sequence = 0u64;
    let mut last_scanned = 0u64;
    while let Some(page) = stream.next_page(cancellation)? {
        cancellation
            .check()
            .map_err(|_| TranscriptExportError::Cancelled)?;
        if page.entries.len() > spec.limits.page_entries {
            return Err(TranscriptExportError::ResourceLimit);
        }
        if page.scanned_count <= last_scanned {
            return Err(TranscriptExportError::InvalidEntry);
        }
        if page.scanned_count > spec.limits.scanned_records as u64 {
            return Err(TranscriptExportError::ResourceLimit);
        }
        last_scanned = page.scanned_count;
        for entry in page.entries {
            if entry.validate().is_err()
                || entry.sequence <= last_sequence
                || !spec.categories.contains(entry.kind)
            {
                return Err(TranscriptExportError::InvalidEntry);
            }
            entry_count = entry_count.saturating_add(1);
            if entry_count > spec.limits.exported_entries as u64 {
                return Err(TranscriptExportError::ResourceLimit);
            }
            last_sequence = entry.sequence;
            let label = spec
                .labels
                .categories
                .get(&entry.kind)
                .ok_or(TranscriptExportError::InvalidSpec)?;
            let rendered = render_transcript_entry_markdown_with_label(&entry, label);
            write_bounded(
                &mut markdown,
                rendered.as_bytes(),
                &mut content_hasher,
                &mut markdown_bytes,
                spec.limits.output_bytes,
                fail_write_after,
            )?;
        }
    }
    let summary = stream.summary();
    if summary.deterministic_source_hash.as_deref() != Some(&spec.expected_source_hash) {
        return Err(TranscriptExportError::SourceChanged);
    }
    markdown.sync_all().map_err(map_io_error)?;
    drop(markdown);
    let manifest = ExportManifest {
        provider_contract: spec.provider_contract.clone(),
        categories: spec.categories.iter().collect(),
        entry_count,
        skipped_count: summary.skipped_count,
        redaction_count: summary.redaction_count,
        deterministic_content_hash: encode_digest(&content_hasher.finalize()),
    };
    manifest
        .validate()
        .map_err(|_| TranscriptExportError::InvalidSpec)?;
    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|_| TranscriptExportError::InvalidSpec)?;
    manifest_bytes.push(b'\n');
    let mut manifest_file = create_private_file(&staging.join(MANIFEST_FILE))?;
    manifest_file
        .write_all(&manifest_bytes)
        .map_err(map_io_error)?;
    manifest_file.sync_all().map_err(map_io_error)?;
    drop(manifest_file);
    sync_directory(staging)?;
    Ok(TranscriptExportResult {
        manifest,
        markdown_bytes: markdown_bytes as u64,
    })
}

fn validate_spec(spec: &TranscriptExportSpec) -> Result<(), TranscriptExportError> {
    spec.limits
        .validate()
        .map_err(|_| TranscriptExportError::InvalidSpec)?;
    if spec.categories.is_empty()
        || spec.expected_source_hash.len() != 64
        || !spec
            .expected_source_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(TranscriptExportError::InvalidSpec);
    }
    if !valid_label(&spec.labels.title)
        || spec.categories.iter().any(|kind| {
            !spec
                .labels
                .categories
                .get(&kind)
                .is_some_and(|label| valid_label(label))
        })
    {
        return Err(TranscriptExportError::InvalidSpec);
    }
    let probe = ExportManifest {
        provider_contract: spec.provider_contract.clone(),
        categories: spec.categories.iter().collect(),
        entry_count: 0,
        skipped_count: 0,
        redaction_count: 0,
        deterministic_content_hash: "0".repeat(64),
    };
    probe
        .validate()
        .map_err(|_| TranscriptExportError::InvalidSpec)
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_EXPORT_LABEL_BYTES
        && !label.chars().any(char::is_control)
}

fn write_bounded(
    file: &mut File,
    bytes: &[u8],
    hasher: &mut Sha256,
    written: &mut usize,
    limit: usize,
    fail_write_after: Option<usize>,
) -> Result<(), TranscriptExportError> {
    let next = written
        .checked_add(bytes.len())
        .ok_or(TranscriptExportError::ResourceLimit)?;
    if next > limit {
        return Err(TranscriptExportError::ResourceLimit);
    }
    if fail_write_after.is_some_and(|threshold| next > threshold) {
        return Err(TranscriptExportError::Io(io::ErrorKind::StorageFull));
    }
    file.write_all(bytes).map_err(map_io_error)?;
    hasher.update(bytes);
    *written = next;
    Ok(())
}

fn staging_path(destination: &Path) -> Result<PathBuf, TranscriptExportError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
    let parent = destination
        .parent()
        .ok_or(TranscriptExportError::UnsafePath)?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(TranscriptExportError::UnsafePath)?;
    let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{name}.tmp-{}-{nonce}", std::process::id())))
}

fn create_private_directory(path: &Path) -> Result<(), TranscriptExportError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path).map_err(map_io_error)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_io_error)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path).map_err(map_io_error)
    }
}

fn create_private_file(path: &Path) -> Result<File, TranscriptExportError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).map_err(map_io_error)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(map_io_error)?;
    Ok(file)
}

fn sync_directory(path: &Path) -> Result<(), TranscriptExportError> {
    match File::open(path).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Unsupported
                    | io::ErrorKind::InvalidInput
                    | io::ErrorKind::PermissionDenied
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(map_io_error(error)),
    }
}

#[cfg(target_os = "macos")]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: both pointers come from live NUL-terminated C strings for the duration of the call.
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: both pointers come from live NUL-terminated C strings for the duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "transcript export destination already exists",
        ));
    }
    fs::rename(source, destination)
}

fn encode_digest(digest: &[u8]) -> String {
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn map_io_error(error: io::Error) -> TranscriptExportError {
    match error.kind() {
        io::ErrorKind::AlreadyExists => TranscriptExportError::DestinationConflict,
        io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory => {
            TranscriptExportError::UnsafePath
        }
        kind => TranscriptExportError::Io(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{ProviderRecordRef, TranscriptContent, TranscriptEntry};

    struct FakeStream {
        pages: std::collections::VecDeque<TranscriptPage>,
        summary: TranscriptExportSourceSummary,
        cancel_after_page: bool,
    }

    impl TranscriptPageStream for FakeStream {
        fn next_page(
            &mut self,
            cancellation: &TranscriptCancellation,
        ) -> Result<Option<TranscriptPage>, TranscriptExportError> {
            cancellation
                .check()
                .map_err(|_| TranscriptExportError::Cancelled)?;
            let page = self.pages.pop_front();
            if page.is_some() && self.cancel_after_page {
                cancellation.cancel();
            }
            Ok(page)
        }

        fn summary(&self) -> TranscriptExportSourceSummary {
            self.summary.clone()
        }
    }

    fn entry(sequence: u64, kind: TranscriptKind, content: &str) -> TranscriptEntry {
        TranscriptEntry {
            sequence,
            occurred_at: None,
            kind,
            content: TranscriptContent::new(content.to_string()).unwrap(),
            provenance: ProviderRecordRef::new(format!("record-{sequence}")).unwrap(),
        }
    }

    fn stream(source_hash: &str) -> FakeStream {
        FakeStream {
            pages: std::collections::VecDeque::from([TranscriptPage {
                entries: vec![
                    entry(1, TranscriptKind::User, "hello **world**"),
                    entry(2, TranscriptKind::Assistant, "safe response"),
                ],
                next_record: None,
                scanned_count: 2,
                skipped_count: 1,
                redaction_count: 2,
            }]),
            summary: TranscriptExportSourceSummary {
                skipped_count: 1,
                redaction_count: 2,
                deterministic_source_hash: Some(source_hash.to_string()),
            },
            cancel_after_page: false,
        }
    }

    fn spec(root: &Path, name: &str, source_hash: &str) -> TranscriptExportSpec {
        TranscriptExportSpec {
            destination: root.join(name),
            provider_contract: "termirust-sanitized-v1".to_string(),
            categories: TranscriptCategorySet::default(),
            limits: TranscriptLimits::default(),
            expected_source_hash: source_hash.to_string(),
            labels: TranscriptExportLabels {
                title: "Localized transcript".to_string(),
                categories: TranscriptKind::ALL
                    .into_iter()
                    .map(|kind| (kind, format!("Localized {}", kind.label())))
                    .collect(),
            },
        }
    }

    #[test]
    fn transcript_export_is_atomic_restricted_and_deterministic() {
        let fixture = tempfile::tempdir().unwrap();
        let source_hash = "a".repeat(64);
        let first = export_transcript(
            &spec(fixture.path(), "first", &source_hash),
            &mut stream(&source_hash),
            &TranscriptCancellation::default(),
        )
        .unwrap();
        let second = export_transcript(
            &spec(fixture.path(), "second", &source_hash),
            &mut stream(&source_hash),
            &TranscriptCancellation::default(),
        )
        .unwrap();
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(
            fs::read(fixture.path().join("first/transcript.md")).unwrap(),
            fs::read(fixture.path().join("second/transcript.md")).unwrap()
        );
        let markdown = fs::read_to_string(fixture.path().join("first/transcript.md")).unwrap();
        assert!(markdown.starts_with("# Localized transcript"));
        assert!(markdown.contains("## Localized User"));
        assert!(markdown.contains("\\*\\*world\\*\\*"));
        assert_eq!(first.manifest.entry_count, 2);
        assert_eq!(first.manifest.skipped_count, 1);
        assert_eq!(first.manifest.redaction_count, 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(fixture.path().join("first"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(fixture.path().join("first/transcript.md"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn transcript_export_cancel_source_change_and_limit_leave_no_partial_output() {
        let fixture = tempfile::tempdir().unwrap();
        let source_hash = "b".repeat(64);
        let cancellation = TranscriptCancellation::default();
        let mut cancelled_stream = stream(&source_hash);
        cancelled_stream.cancel_after_page = true;
        assert_eq!(
            export_transcript(
                &spec(fixture.path(), "cancelled", &source_hash),
                &mut cancelled_stream,
                &cancellation,
            ),
            Err(TranscriptExportError::Cancelled)
        );
        assert!(!fixture.path().join("cancelled").exists());

        assert_eq!(
            export_transcript(
                &spec(fixture.path(), "changed", &source_hash),
                &mut stream(&"c".repeat(64)),
                &TranscriptCancellation::default(),
            ),
            Err(TranscriptExportError::SourceChanged)
        );
        assert!(!fixture.path().join("changed").exists());

        let mut limited = spec(fixture.path(), "limited", &source_hash);
        limited.limits.output_bytes = "# Localized transcript\n\n".len();
        assert_eq!(
            export_transcript(
                &limited,
                &mut stream(&source_hash),
                &TranscriptCancellation::default(),
            ),
            Err(TranscriptExportError::ResourceLimit)
        );
        assert!(!fixture.path().join("limited").exists());

        let disk_full = spec(fixture.path(), "disk-full", &source_hash);
        assert_eq!(
            export_transcript_with_fault(
                &disk_full,
                &mut stream(&source_hash),
                &TranscriptCancellation::default(),
                Some("# Localized transcript\n\n".len()),
            ),
            Err(TranscriptExportError::Io(io::ErrorKind::StorageFull))
        );
        assert!(!fixture.path().join("disk-full").exists());

        let mut stagnant = stream(&source_hash);
        stagnant.pages.front_mut().unwrap().scanned_count = 0;
        assert_eq!(
            export_transcript(
                &spec(fixture.path(), "stagnant", &source_hash),
                &mut stagnant,
                &TranscriptCancellation::default(),
            ),
            Err(TranscriptExportError::InvalidEntry)
        );
        assert!(!fixture.path().join("stagnant").exists());
    }

    #[cfg(unix)]
    #[test]
    fn transcript_export_rejects_existing_and_symlink_destinations_without_mutation() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let source_hash = "d".repeat(64);
        let existing = fixture.path().join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("sentinel"), b"preserve").unwrap();
        assert_eq!(
            export_transcript(
                &spec(fixture.path(), "existing", &source_hash),
                &mut stream(&source_hash),
                &TranscriptCancellation::default(),
            ),
            Err(TranscriptExportError::DestinationConflict)
        );
        assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"preserve");

        let outside = fixture.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, fixture.path().join("linked")).unwrap();
        assert_eq!(
            export_transcript(
                &spec(fixture.path(), "linked", &source_hash),
                &mut stream(&source_hash),
                &TranscriptCancellation::default(),
            ),
            Err(TranscriptExportError::DestinationConflict)
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        let racing_source = fixture.path().join("racing-source");
        let racing_destination = fixture.path().join("racing-destination");
        fs::create_dir(&racing_source).unwrap();
        fs::create_dir(&racing_destination).unwrap();
        fs::write(racing_destination.join("sentinel"), b"preserve").unwrap();
        assert_eq!(
            rename_no_replace(&racing_source, &racing_destination)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(racing_source.exists());
        assert_eq!(
            fs::read(racing_destination.join("sentinel")).unwrap(),
            b"preserve"
        );
    }
}
