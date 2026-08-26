// Candidate parsing remains intentionally dormant in release builds until D02 approves an exact
// provider/version contract. Tests exercise the complete boundary without advertising support.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::{self, BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use termirust_domain::{
    MAX_TRANSCRIPT_OUTPUT_BYTES, ProviderRecordRef, RuntimeId, RuntimeVersion,
    TranscriptCancellation, TranscriptEntry, TranscriptError, TranscriptKind, TranscriptPage,
    TranscriptRequest, normalize_transcript_content,
};

const TRANSCRIPT_FILE_NAME: &str = "records.jsonl";
const MAX_TRANSCRIPT_SOURCE_BYTES: u64 = (MAX_TRANSCRIPT_OUTPUT_BYTES as u64) * 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateTranscriptContract {
    pub id: &'static str,
    pub runtime_id: RuntimeId,
    pub version: RuntimeVersion,
    pub release_enabled: bool,
    pub record_format: &'static str,
}

pub fn sanitized_candidate_transcript_contract() -> CandidateTranscriptContract {
    CandidateTranscriptContract {
        id: "termirust-sanitized-v1",
        runtime_id: RuntimeId::new("fixture").expect("fixture runtime ID is bounded"),
        version: RuntimeVersion::new(1, 0, 0),
        release_enabled: false,
        record_format: "bounded-json-lines-v1",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptReadSummary {
    pub scanned_count: u64,
    pub skipped_count: u64,
    pub redaction_count: u64,
    pub category_counts: BTreeMap<TranscriptKind, u64>,
    pub deterministic_source_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptSourceError {
    Cancelled,
    ContainmentViolation,
    MalformedContract,
    PermissionDenied,
    ResourceLimit,
    SourceChanged,
    SourceMissing,
    UnavailableContract,
}

impl From<TranscriptError> for TranscriptSourceError {
    fn from(error: TranscriptError) -> Self {
        match error {
            TranscriptError::Cancelled => Self::Cancelled,
            TranscriptError::RecordTooLarge | TranscriptError::ResourceLimit => Self::ResourceLimit,
            TranscriptError::EmptyCategories
            | TranscriptError::InvalidContent
            | TranscriptError::InvalidProviderContract
            | TranscriptError::InvalidProviderReference
            | TranscriptError::InvalidRange
            | TranscriptError::InvalidSequence => Self::MalformedContract,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct SourceIdentity {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl fmt::Debug for SourceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceIdentity(<redacted>)")
    }
}

impl SourceIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

pub struct TranscriptReader {
    source_path: PathBuf,
    source_identity: SourceIdentity,
    reader: BufReader<File>,
    source_hasher: Sha256,
    ordinal: u64,
    scanned_count: u64,
    skipped_count: u64,
    redaction_count: u64,
    emitted_count: u64,
    category_counts: BTreeMap<TranscriptKind, u64>,
    complete_hash: Option<String>,
}

impl fmt::Debug for TranscriptReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TranscriptReader")
            .field("source", &"<redacted>")
            .field("identity", &self.source_identity)
            .field("scanned_count", &self.scanned_count)
            .field("skipped_count", &self.skipped_count)
            .finish_non_exhaustive()
    }
}

impl TranscriptReader {
    pub fn open(
        contract: &CandidateTranscriptContract,
        root: &Path,
    ) -> Result<Self, TranscriptSourceError> {
        if contract != &sanitized_candidate_transcript_contract() {
            return Err(TranscriptSourceError::UnavailableContract);
        }
        let root_metadata = fs::symlink_metadata(root).map_err(map_io_error)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(TranscriptSourceError::ContainmentViolation);
        }
        let canonical_root = root.canonicalize().map_err(map_io_error)?;
        let source_path = canonical_root.join(TRANSCRIPT_FILE_NAME);
        let source_metadata = fs::symlink_metadata(&source_path).map_err(map_io_error)?;
        if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
            return Err(TranscriptSourceError::ContainmentViolation);
        }
        if source_metadata.len() > MAX_TRANSCRIPT_SOURCE_BYTES {
            return Err(TranscriptSourceError::ResourceLimit);
        }
        let canonical_source = source_path.canonicalize().map_err(map_io_error)?;
        if canonical_source.parent() != Some(canonical_root.as_path()) {
            return Err(TranscriptSourceError::ContainmentViolation);
        }
        let source_identity = SourceIdentity::from_metadata(&source_metadata);
        let file = File::open(&canonical_source).map_err(map_io_error)?;
        Ok(Self {
            source_path: canonical_source,
            source_identity,
            reader: BufReader::new(file),
            source_hasher: Sha256::new(),
            ordinal: 0,
            scanned_count: 0,
            skipped_count: 0,
            redaction_count: 0,
            emitted_count: 0,
            category_counts: BTreeMap::new(),
            complete_hash: None,
        })
    }

    pub fn next_page(
        &mut self,
        request: &TranscriptRequest,
        cancellation: &TranscriptCancellation,
    ) -> Result<Option<TranscriptPage>, TranscriptSourceError> {
        request.validate()?;
        cancellation.check()?;
        self.verify_source_identity()?;
        if self.complete_hash.is_some() {
            return Ok(None);
        }
        let mut entries = Vec::with_capacity(request.limits.page_entries);
        let starting_skipped = self.skipped_count;
        let starting_redactions = self.redaction_count;
        while entries.len() < request.limits.page_entries
            && self.scanned_count < request.limits.scanned_records as u64
        {
            cancellation.check()?;
            let Some(record) = read_bounded_record(
                &mut self.reader,
                request.limits.record_bytes,
                &mut self.source_hasher,
                cancellation,
            )?
            else {
                self.finish_hash();
                break;
            };
            self.ordinal = self.ordinal.saturating_add(1);
            self.scanned_count = self.scanned_count.saturating_add(1);
            if record.oversize {
                self.skipped_count = self.skipped_count.saturating_add(1);
                continue;
            }
            let Some(raw) = serde_json::from_slice::<RawTranscriptRecord>(&record.bytes).ok()
            else {
                self.skipped_count = self.skipped_count.saturating_add(1);
                continue;
            };
            *self.category_counts.entry(raw.kind).or_default() += 1;
            if !request.categories.contains(raw.kind) || !request.range.includes(self.ordinal) {
                continue;
            }
            let normalized = match normalize_transcript_content(&raw.content, cancellation) {
                Ok(value) => value,
                Err(TranscriptError::Cancelled) => return Err(TranscriptSourceError::Cancelled),
                Err(_) => {
                    self.skipped_count = self.skipped_count.saturating_add(1);
                    continue;
                }
            };
            self.redaction_count = self
                .redaction_count
                .saturating_add(normalized.redaction_count);
            let entry = TranscriptEntry {
                sequence: self.ordinal,
                occurred_at: raw.occurred_at,
                kind: raw.kind,
                content: normalized.content,
                provenance: ProviderRecordRef::new(format!("record-{}", self.ordinal))?,
            };
            entries.push(entry);
            self.emitted_count = self.emitted_count.saturating_add(1);
            if self.emitted_count >= request.limits.exported_entries as u64 {
                break;
            }
        }
        if self.scanned_count >= request.limits.scanned_records as u64
            || self.emitted_count >= request.limits.exported_entries as u64
        {
            let available = self.reader.fill_buf().map_err(map_io_error)?;
            if available.is_empty() {
                self.finish_hash();
            }
        }
        if self.scanned_count >= request.limits.scanned_records as u64
            && self.complete_hash.is_none()
        {
            return Err(TranscriptSourceError::ResourceLimit);
        }
        if self.emitted_count >= request.limits.exported_entries as u64
            && self.complete_hash.is_none()
        {
            return Err(TranscriptSourceError::ResourceLimit);
        }
        let next_record = self
            .complete_hash
            .is_none()
            .then_some(self.ordinal.saturating_add(1));
        if entries.is_empty() && next_record.is_none() {
            return Ok(None);
        }
        Ok(Some(TranscriptPage {
            entries,
            next_record,
            scanned_count: self.scanned_count,
            skipped_count: self.skipped_count.saturating_sub(starting_skipped),
            redaction_count: self.redaction_count.saturating_sub(starting_redactions),
        }))
    }

    pub fn summary(&self) -> TranscriptReadSummary {
        TranscriptReadSummary {
            scanned_count: self.scanned_count,
            skipped_count: self.skipped_count,
            redaction_count: self.redaction_count,
            category_counts: self.category_counts.clone(),
            deterministic_source_hash: self.complete_hash.clone(),
        }
    }

    fn verify_source_identity(&self) -> Result<(), TranscriptSourceError> {
        let metadata = fs::symlink_metadata(&self.source_path).map_err(map_io_error)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || SourceIdentity::from_metadata(&metadata) != self.source_identity
        {
            return Err(TranscriptSourceError::SourceChanged);
        }
        Ok(())
    }

    fn finish_hash(&mut self) {
        let digest = self.source_hasher.clone().finalize();
        self.complete_hash = Some(encode_digest(&digest));
    }
}

fn encode_digest(digest: &[u8]) -> String {
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[derive(Deserialize)]
struct RawTranscriptRecord {
    kind: TranscriptKind,
    #[serde(default)]
    occurred_at: Option<i64>,
    content: String,
}

struct BoundedRecord {
    bytes: Vec<u8>,
    oversize: bool,
}

fn read_bounded_record(
    reader: &mut BufReader<File>,
    limit: usize,
    hasher: &mut Sha256,
    cancellation: &TranscriptCancellation,
) -> Result<Option<BoundedRecord>, TranscriptSourceError> {
    let mut bytes = Vec::new();
    let mut oversize = false;
    let mut consumed_any = false;
    loop {
        cancellation.check()?;
        let available = reader.fill_buf().map_err(map_io_error)?;
        if available.is_empty() {
            return if consumed_any {
                Ok(Some(BoundedRecord { bytes, oversize }))
            } else {
                Ok(None)
            };
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let chunk = &available[..consumed];
        hasher.update(chunk);
        consumed_any = true;
        let content = chunk.strip_suffix(b"\n").unwrap_or(chunk);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if !oversize && bytes.len().saturating_add(content.len()) <= limit {
            bytes.extend_from_slice(content);
        } else {
            oversize = true;
            bytes.clear();
        }
        let ended = chunk.ends_with(b"\n");
        reader.consume(consumed);
        if ended {
            return Ok(Some(BoundedRecord { bytes, oversize }));
        }
    }
}

fn map_io_error(error: io::Error) -> TranscriptSourceError {
    match error.kind() {
        io::ErrorKind::NotFound => TranscriptSourceError::SourceMissing,
        io::ErrorKind::PermissionDenied => TranscriptSourceError::PermissionDenied,
        _ => TranscriptSourceError::ContainmentViolation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_store::{
        TranscriptExportError, TranscriptExportLabels, TranscriptExportSourceSummary,
        TranscriptExportSpec, TranscriptPageStream, export_transcript,
    };

    struct FixtureExportStream {
        reader: TranscriptReader,
        request: TranscriptRequest,
    }

    impl TranscriptPageStream for FixtureExportStream {
        fn next_page(
            &mut self,
            cancellation: &TranscriptCancellation,
        ) -> Result<Option<TranscriptPage>, TranscriptExportError> {
            self.reader
                .next_page(&self.request, cancellation)
                .map_err(|error| match error {
                    TranscriptSourceError::Cancelled => TranscriptExportError::Cancelled,
                    TranscriptSourceError::ResourceLimit => TranscriptExportError::ResourceLimit,
                    TranscriptSourceError::SourceChanged => TranscriptExportError::SourceChanged,
                    TranscriptSourceError::PermissionDenied => {
                        TranscriptExportError::Io(io::ErrorKind::PermissionDenied)
                    }
                    TranscriptSourceError::ContainmentViolation
                    | TranscriptSourceError::MalformedContract
                    | TranscriptSourceError::SourceMissing
                    | TranscriptSourceError::UnavailableContract => {
                        TranscriptExportError::InvalidEntry
                    }
                })
        }

        fn summary(&self) -> TranscriptExportSourceSummary {
            let summary = self.reader.summary();
            TranscriptExportSourceSummary {
                skipped_count: summary.skipped_count,
                redaction_count: summary.redaction_count,
                deterministic_source_hash: summary.deterministic_source_hash,
            }
        }
    }

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runtimes/candidate/transcripts")
    }

    fn collect(
        root: &Path,
        request: &TranscriptRequest,
    ) -> Result<(Vec<TranscriptEntry>, TranscriptReadSummary), TranscriptSourceError> {
        let mut reader = TranscriptReader::open(&sanitized_candidate_transcript_contract(), root)?;
        let cancellation = TranscriptCancellation::default();
        let mut entries = Vec::new();
        while let Some(page) = reader.next_page(request, &cancellation)? {
            entries.extend(page.entries);
        }
        Ok((entries, reader.summary()))
    }

    #[test]
    fn transcript_contracts_are_exact_and_disabled_pending_client_approval() {
        let contract = sanitized_candidate_transcript_contract();
        assert_eq!(contract.runtime_id.as_str(), "fixture");
        assert_eq!(contract.version, RuntimeVersion::new(1, 0, 0));
        assert!(!contract.release_enabled);
        assert!(
            termirust_domain::compiled_runtime_descriptors()
                .iter()
                .all(
                    |descriptor| descriptor.version_rules.iter().all(|rule| !rule
                        .capabilities
                        .contains(termirust_domain::RuntimeCapability::TranscriptExport))
                )
        );
    }

    #[test]
    fn transcript_contracts_fixture_exports_deterministically_after_preview() {
        let contract = sanitized_candidate_transcript_contract();
        let request = TranscriptRequest::default();
        let (_, preview) = collect(&fixture_root(), &request).unwrap();
        let source_hash = preview.deterministic_source_hash.unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let export_once = |name: &str| {
            let reader = TranscriptReader::open(&contract, &fixture_root()).unwrap();
            let mut stream = FixtureExportStream {
                reader,
                request: request.clone(),
            };
            let spec = TranscriptExportSpec {
                destination: fixture.path().join(name),
                provider_contract: contract.id.to_string(),
                categories: request.categories.clone(),
                limits: request.limits,
                expected_source_hash: source_hash.clone(),
                labels: TranscriptExportLabels {
                    title: "Sanitized transcript".to_string(),
                    categories: TranscriptKind::ALL
                        .into_iter()
                        .map(|kind| (kind, kind.label().to_string()))
                        .collect(),
                },
            };
            export_transcript(&spec, &mut stream, &TranscriptCancellation::default()).unwrap()
        };
        let first = export_once("first");
        let second = export_once("second");
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(first.manifest.entry_count, 4);
        assert_eq!(first.manifest.redaction_count, 2);
        let first_markdown =
            fs::read_to_string(fixture.path().join("first/transcript.md")).unwrap();
        let second_markdown =
            fs::read_to_string(fixture.path().join("second/transcript.md")).unwrap();
        assert_eq!(first_markdown, second_markdown);
        assert!(!first_markdown.contains("Sensitive reasoning"));
        assert!(!first_markdown.contains("cat README"));
        assert!(!first_markdown.contains("canary-secret"));
        assert!(first_markdown.contains("\u{0928}\u{092e}\u{0938}\u{094d}\u{0924}\u{0947}"));
    }

    #[test]
    fn transcript_reader_preserves_provider_order_and_default_consent() {
        let (entries, summary) = collect(&fixture_root(), &TranscriptRequest::default()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 7, 8]
        );
        assert!(
            entries.iter().all(|entry| matches!(
                entry.kind,
                TranscriptKind::User | TranscriptKind::Assistant
            ))
        );
        assert_eq!(summary.scanned_count, 8);
        assert_eq!(summary.skipped_count, 0);
        assert_eq!(summary.redaction_count, 2);
        assert_eq!(
            summary.deterministic_source_hash,
            Some(termirust_domain::deterministic_content_hash(
                &fs::read(fixture_root().join(TRANSCRIPT_FILE_NAME)).unwrap()
            ))
        );
    }

    #[test]
    fn transcript_reader_skips_malformed_without_reordering_valid_records() {
        let fixture = tempfile::tempdir().unwrap();
        fs::copy(
            fixture_root().join("malformed.jsonl"),
            fixture.path().join(TRANSCRIPT_FILE_NAME),
        )
        .unwrap();
        let (entries, summary) = collect(fixture.path(), &TranscriptRequest::default()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
        assert_eq!(summary.skipped_count, 2);
    }

    #[test]
    fn transcript_reader_skips_oversize_record_without_large_record_allocation() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join(TRANSCRIPT_FILE_NAME);
        fs::write(
            &source,
            format!(
                "{{\"kind\":\"user\",\"content\":\"{}\"}}\n{{\"kind\":\"assistant\",\"content\":\"safe\"}}\n",
                "x".repeat(1024)
            ),
        )
        .unwrap();
        let request = TranscriptRequest {
            limits: termirust_domain::TranscriptLimits {
                record_bytes: 128,
                ..termirust_domain::TranscriptLimits::default()
            },
            ..TranscriptRequest::default()
        };
        let (entries, summary) = collect(fixture.path(), &request).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence, 2);
        assert_eq!(summary.skipped_count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn transcript_reader_rejects_symlink_and_detects_source_change() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let outside = fixture.path().join("outside.jsonl");
        fs::write(&outside, b"{}\n").unwrap();
        let root = fixture.path().join("root");
        fs::create_dir(&root).unwrap();
        symlink(&outside, root.join(TRANSCRIPT_FILE_NAME)).unwrap();
        assert_eq!(
            TranscriptReader::open(&sanitized_candidate_transcript_contract(), &root).unwrap_err(),
            TranscriptSourceError::ContainmentViolation
        );

        fs::remove_file(root.join(TRANSCRIPT_FILE_NAME)).unwrap();
        fs::copy(
            fixture_root().join(TRANSCRIPT_FILE_NAME),
            root.join(TRANSCRIPT_FILE_NAME),
        )
        .unwrap();
        let mut reader =
            TranscriptReader::open(&sanitized_candidate_transcript_contract(), &root).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(root.join(TRANSCRIPT_FILE_NAME))
            .unwrap();
        fs::write(root.join(TRANSCRIPT_FILE_NAME), b"changed\n").unwrap();
        assert_eq!(
            reader
                .next_page(
                    &TranscriptRequest::default(),
                    &TranscriptCancellation::default()
                )
                .unwrap_err(),
            TranscriptSourceError::SourceChanged
        );
    }

    #[test]
    fn transcript_reader_cancellation_has_no_content_in_debug() {
        let mut reader =
            TranscriptReader::open(&sanitized_candidate_transcript_contract(), &fixture_root())
                .unwrap();
        assert!(!format!("{reader:?}").contains("candidate"));
        let cancellation = TranscriptCancellation::default();
        cancellation.cancel();
        assert_eq!(
            reader
                .next_page(&TranscriptRequest::default(), &cancellation)
                .unwrap_err(),
            TranscriptSourceError::Cancelled
        );
    }
}
