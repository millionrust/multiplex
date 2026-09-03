use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use termirust_domain::{
    DERIVED_INDEX_VERSION, Group, HostedSession, IndexSourceRevisions, LaunchPreset, PaletteIndex,
    Project, ProjectSessionIndex, build_palette_index, build_project_session_index,
};
use uuid::Uuid;

use crate::presets::read_preset_health_source;
use crate::projects::{ProjectHealthSource, read_project_health_source, read_regular_bounded};
use crate::sessions::read_session_health_source;
use crate::{CURRENT_FORMAT_VERSION, StoreError, file_lock};

const FORMAT_FILE: &str = "format.json";
const LOCK_FILE: &str = "metadata.lock";
const INDEX_DIR: &str = "derived-indexes";
const INDEX_MARKER: &str = ".termirust-derived-indexes-v1";
const PROJECT_SESSION_FILE: &str = "project-session-v1.json";
const PALETTE_FILE: &str = "palette-v1.json";
const REPAIR_JOURNAL: &str = "repair-journal-v1.json";
const MAX_FORMAT_BYTES: u64 = 64 * 1024;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HealthCheckId(Uuid);

impl HealthCheckId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCheckKind {
    StoreReadable,
    StoreVersion,
    RecordHashes,
    ProjectSessionIndex,
    PaletteIndex,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthFindingState {
    Healthy,
    Partial,
    Corrupt,
    Newer,
    Permission,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthEvidenceCode {
    Verified,
    IndexMissing,
    IndexStale,
    IndexMalformed,
    StoreMalformed,
    StoreTooLarge,
    StoreUnsafe,
    StoreNewer,
    PermissionDenied,
    IoUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthFinding {
    pub kind: HealthCheckKind,
    pub state: HealthFindingState,
    pub evidence: HealthEvidenceCode,
    pub actual_digest: Option<String>,
    pub expected_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthReport {
    pub id: HealthCheckId,
    pub findings: Vec<HealthFinding>,
    pub source_revisions: Option<IndexSourceRevisions>,
    pub authoritative_records: u64,
}

impl HealthReport {
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.state == HealthFindingState::Healthy)
    }

    #[must_use]
    pub fn finding(&self, kind: HealthCheckKind) -> Option<&HealthFinding> {
        self.findings.iter().find(|finding| finding.kind == kind)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexRepairKind {
    ProjectSessionIndex,
    PaletteIndex,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexRepairStep {
    BuildTemporary,
    VerifyTemporary,
    RecheckSourceRevision,
    Publish,
    ReopenAndVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexRepairState {
    Planned,
    BuildingTemp,
    Verifying,
    Publishing,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceHash {
    pub kind: HealthCheckKind,
    pub sha256: String,
}

pub struct IndexRepairPlan {
    pub id: HealthCheckId,
    pub kind: IndexRepairKind,
    pub source_revisions: IndexSourceRevisions,
    pub source_hashes: Vec<SourceHash>,
    pub target_path: PathBuf,
    pub temporary_path: PathBuf,
    pub estimated_entries: u64,
    pub estimated_bytes: u64,
    pub steps: Vec<IndexRepairStep>,
    pub cancellation_boundary: IndexRepairStep,
    expected_bytes: Vec<u8>,
    expected_digest: String,
}

impl fmt::Debug for IndexRepairPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IndexRepairPlan")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("source_revisions", &self.source_revisions)
            .field("source_hashes", &self.source_hashes)
            .field("target_path", &self.target_path.file_name())
            .field("temporary_path", &self.temporary_path.file_name())
            .field("estimated_entries", &self.estimated_entries)
            .field("estimated_bytes", &self.estimated_bytes)
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default)]
pub struct RepairCancellation(Arc<AtomicBool>);

impl RepairCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexRepairReceipt {
    pub id: HealthCheckId,
    pub kind: IndexRepairKind,
    pub digest: String,
    pub state: IndexRepairState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthErrorCode {
    Cancelled,
    CorruptSource,
    NewerSource,
    PermissionDenied,
    Unavailable,
    StaleSource,
    VerificationFailed,
    UnsafeEntry,
    SizeLimit,
    InjectedCrash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthError {
    pub code: HealthErrorCode,
}

impl fmt::Display for HealthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "health operation failed: {:?}", self.code)
    }
}

impl std::error::Error for HealthError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub enum RepairFaultPoint {
    AfterTempCreated,
    AfterTempWrite,
    AfterTempSync,
    AfterJournalSync,
    AfterPublish,
}

#[derive(Clone)]
pub struct HealthRepository {
    root: PathBuf,
}

struct SourceSnapshot {
    revisions: IndexSourceRevisions,
    projects: Vec<Project>,
    groups: Vec<Group>,
    sessions: Vec<HostedSession>,
    presets: Vec<LaunchPreset>,
    hashes: Vec<SourceHash>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatDocument {
    format_version: u16,
    minimum_reader: u16,
    instance_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepairJournal {
    version: u16,
    kind: IndexRepairKind,
    target_name: String,
    temporary_name: String,
    expected_digest: String,
}

impl HealthRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, HealthError> {
        let repository = Self { root: root.into() };
        repository.validate_root()?;
        Ok(repository)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn scan(&self) -> HealthReport {
        let id = HealthCheckId::new();
        let lock = match MetadataLock::shared(&self.root) {
            Ok(lock) => lock,
            Err(error) => return failed_report(id, classify_error(&error)),
        };
        let source = match self.read_source_locked() {
            Ok(source) => source,
            Err(error) => {
                drop(lock);
                return failed_report(id, error.code);
            }
        };
        let project_session = match self.project_session_bytes(&source) {
            Ok(bytes) => bytes,
            Err(error) => return failed_report(id, error.code),
        };
        let palette = match self.palette_bytes(&source) {
            Ok(bytes) => bytes,
            Err(error) => return failed_report(id, error.code),
        };
        let authoritative_records = source
            .projects
            .len()
            .saturating_add(source.groups.len())
            .saturating_add(source.sessions.len())
            .saturating_add(source.presets.len()) as u64;
        let findings = vec![
            healthy_finding(HealthCheckKind::StoreReadable),
            healthy_finding(HealthCheckKind::StoreVersion),
            healthy_finding(HealthCheckKind::RecordHashes),
            self.inspect_index(
                IndexRepairKind::ProjectSessionIndex,
                &project_session,
                source.revisions,
            ),
            self.inspect_index(IndexRepairKind::PaletteIndex, &palette, source.revisions),
        ];
        drop(lock);
        HealthReport {
            id,
            findings,
            source_revisions: Some(source.revisions),
            authoritative_records,
        }
    }

    pub fn plan_repair(&self, kind: IndexRepairKind) -> Result<IndexRepairPlan, HealthError> {
        let _lock = MetadataLock::shared(&self.root).map_err(map_store_error)?;
        let source = self.read_source_locked()?;
        let expected_bytes = match kind {
            IndexRepairKind::ProjectSessionIndex => self.project_session_bytes(&source)?,
            IndexRepairKind::PaletteIndex => self.palette_bytes(&source)?,
        };
        if expected_bytes.len() as u64 > MAX_INDEX_BYTES {
            return Err(error(HealthErrorCode::SizeLimit));
        }
        let id = HealthCheckId::new();
        let target_path = self.index_root().join(target_name(kind));
        let temporary_path =
            self.index_root()
                .join(format!(".repair-{}-{}.tmp", id.0, kind_slug(kind)));
        let estimated_entries = match kind {
            IndexRepairKind::ProjectSessionIndex => source.sessions.len() + source.projects.len(),
            IndexRepairKind::PaletteIndex => {
                source.projects.len()
                    + source.groups.len()
                    + source.sessions.len()
                    + source.presets.len()
            }
        } as u64;
        Ok(IndexRepairPlan {
            id,
            kind,
            source_revisions: source.revisions,
            source_hashes: source.hashes,
            target_path,
            temporary_path,
            estimated_entries,
            estimated_bytes: expected_bytes.len() as u64,
            steps: vec![
                IndexRepairStep::BuildTemporary,
                IndexRepairStep::VerifyTemporary,
                IndexRepairStep::RecheckSourceRevision,
                IndexRepairStep::Publish,
                IndexRepairStep::ReopenAndVerify,
            ],
            cancellation_boundary: IndexRepairStep::Publish,
            expected_digest: sha256_hex(&expected_bytes),
            expected_bytes,
        })
    }

    pub fn repair(
        &self,
        plan: IndexRepairPlan,
        cancellation: &RepairCancellation,
    ) -> Result<IndexRepairReceipt, HealthError> {
        self.repair_with_fault(plan, cancellation, None)
    }

    /// Completes or removes only marker-owned debris from an interrupted index repair.
    /// Call this during application startup, separately from a read-only health scan.
    pub fn recover_interrupted_repair(&self) -> Result<(), HealthError> {
        self.recover_owned_repair()
    }

    #[doc(hidden)]
    pub fn repair_with_fault(
        &self,
        plan: IndexRepairPlan,
        cancellation: &RepairCancellation,
        fault: Option<RepairFaultPoint>,
    ) -> Result<IndexRepairReceipt, HealthError> {
        self.recover_owned_repair()?;
        check_cancelled(cancellation)?;
        self.ensure_index_root()?;
        reject_unsafe_target(&plan.target_path)?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut temporary = options.open(&plan.temporary_path).map_err(map_io_error)?;
        inject(fault, RepairFaultPoint::AfterTempCreated)?;
        temporary
            .write_all(&plan.expected_bytes)
            .map_err(map_io_error)?;
        inject(fault, RepairFaultPoint::AfterTempWrite)?;
        temporary.sync_all().map_err(map_io_error)?;
        drop(temporary);
        inject(fault, RepairFaultPoint::AfterTempSync)?;
        check_cancelled(cancellation).inspect_err(|_| {
            let _ = fs::remove_file(&plan.temporary_path);
        })?;
        let actual = read_regular_bounded(
            &plan.temporary_path,
            "derived index temporary",
            MAX_INDEX_BYTES,
        )
        .map_err(map_store_error)?;
        if actual != plan.expected_bytes || sha256_hex(&actual) != plan.expected_digest {
            let _ = fs::remove_file(&plan.temporary_path);
            return Err(error(HealthErrorCode::VerificationFailed));
        }

        let _lock = MetadataLock::exclusive(&self.root).map_err(map_store_error)?;
        let current = self.read_source_locked()?;
        if current.revisions != plan.source_revisions || current.hashes != plan.source_hashes {
            let _ = fs::remove_file(&plan.temporary_path);
            return Err(error(HealthErrorCode::StaleSource));
        }
        check_cancelled(cancellation).inspect_err(|_| {
            let _ = fs::remove_file(&plan.temporary_path);
        })?;
        let journal = RepairJournal {
            version: 1,
            kind: plan.kind,
            target_name: target_name(plan.kind).to_string(),
            temporary_name: plan
                .temporary_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| error(HealthErrorCode::UnsafeEntry))?
                .to_string(),
            expected_digest: plan.expected_digest.clone(),
        };
        self.write_journal(&journal)?;
        inject(fault, RepairFaultPoint::AfterJournalSync)?;
        fs::rename(&plan.temporary_path, &plan.target_path).map_err(map_io_error)?;
        sync_directory(&self.index_root()).map_err(map_io_error)?;
        inject(fault, RepairFaultPoint::AfterPublish)?;
        let published = read_regular_bounded(&plan.target_path, "derived index", MAX_INDEX_BYTES)
            .map_err(map_store_error)?;
        if published != plan.expected_bytes || sha256_hex(&published) != plan.expected_digest {
            return Err(error(HealthErrorCode::VerificationFailed));
        }
        let _ = fs::remove_file(self.index_root().join(REPAIR_JOURNAL));
        let _ = sync_directory(&self.index_root());
        Ok(IndexRepairReceipt {
            id: plan.id,
            kind: plan.kind,
            digest: plan.expected_digest,
            state: IndexRepairState::Complete,
        })
    }

    fn validate_root(&self) -> Result<(), HealthError> {
        let metadata = fs::symlink_metadata(&self.root).map_err(map_io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(error(HealthErrorCode::UnsafeEntry));
        }
        Ok(())
    }

    fn read_source_locked(&self) -> Result<SourceSnapshot, HealthError> {
        let format_bytes =
            read_regular_bounded(&self.root.join(FORMAT_FILE), FORMAT_FILE, MAX_FORMAT_BYTES)
                .map_err(map_store_error)?;
        let format: FormatDocument = serde_json::from_slice(&format_bytes)
            .map_err(|_| error(HealthErrorCode::CorruptSource))?;
        if format.format_version > CURRENT_FORMAT_VERSION
            || format.minimum_reader > CURRENT_FORMAT_VERSION
        {
            return Err(error(HealthErrorCode::NewerSource));
        }
        if format.format_version != CURRENT_FORMAT_VERSION
            || format.minimum_reader == 0
            || Uuid::parse_str(&format.instance_id).is_err()
        {
            return Err(error(HealthErrorCode::CorruptSource));
        }
        let (
            project_bytes,
            ProjectHealthSource {
                revision: projects_revision,
                projects,
                groups,
            },
        ) = read_project_health_source(&self.root).map_err(map_store_error)?;
        let (session_bytes, sessions_revision, sessions) =
            read_session_health_source(&self.root).map_err(map_store_error)?;
        let (preset_bytes, presets_revision, presets) =
            read_preset_health_source(&self.root).map_err(map_store_error)?;
        let revisions = IndexSourceRevisions {
            projects: projects_revision,
            sessions: sessions_revision,
            presets: presets_revision,
        };
        Ok(SourceSnapshot {
            revisions,
            projects,
            groups,
            sessions,
            presets,
            hashes: vec![
                SourceHash {
                    kind: HealthCheckKind::StoreVersion,
                    sha256: sha256_hex(&format_bytes),
                },
                SourceHash {
                    kind: HealthCheckKind::ProjectSessionIndex,
                    sha256: sha256_hex(&project_bytes),
                },
                SourceHash {
                    kind: HealthCheckKind::RecordHashes,
                    sha256: sha256_hex(&session_bytes),
                },
                SourceHash {
                    kind: HealthCheckKind::PaletteIndex,
                    sha256: sha256_hex(&preset_bytes),
                },
            ],
        })
    }

    fn project_session_bytes(&self, source: &SourceSnapshot) -> Result<Vec<u8>, HealthError> {
        let index =
            build_project_session_index(source.revisions, &source.projects, &source.sessions)
                .map_err(|_| error(HealthErrorCode::CorruptSource))?;
        serialize_index(&index)
    }

    fn palette_bytes(&self, source: &SourceSnapshot) -> Result<Vec<u8>, HealthError> {
        let index = build_palette_index(
            source.revisions,
            &source.projects,
            &source.groups,
            &source.presets,
            &source.sessions,
        )
        .map_err(|_| error(HealthErrorCode::CorruptSource))?;
        serialize_index(&index)
    }

    fn inspect_index(
        &self,
        kind: IndexRepairKind,
        expected: &[u8],
        revisions: IndexSourceRevisions,
    ) -> HealthFinding {
        let check_kind = check_kind(kind);
        let expected_digest = sha256_hex(expected);
        let path = self.index_root().join(target_name(kind));
        let actual = match read_regular_bounded(&path, "derived index", MAX_INDEX_BYTES) {
            Ok(bytes) => bytes,
            Err(StoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }) => {
                return HealthFinding {
                    kind: check_kind,
                    state: HealthFindingState::Partial,
                    evidence: HealthEvidenceCode::IndexMissing,
                    actual_digest: None,
                    expected_digest: Some(expected_digest),
                };
            }
            Err(error) => {
                let (state, evidence) = finding_for_store_error(&error);
                return HealthFinding {
                    kind: check_kind,
                    state,
                    evidence,
                    actual_digest: None,
                    expected_digest: Some(expected_digest),
                };
            }
        };
        let valid = match kind {
            IndexRepairKind::ProjectSessionIndex => {
                serde_json::from_slice::<ProjectSessionIndex>(&actual).is_ok_and(|index| {
                    index.version == DERIVED_INDEX_VERSION && index.source_revisions == revisions
                })
            }
            IndexRepairKind::PaletteIndex => serde_json::from_slice::<PaletteIndex>(&actual)
                .is_ok_and(|index| {
                    index.version == DERIVED_INDEX_VERSION && index.source_revisions == revisions
                }),
        };
        let actual_digest = sha256_hex(&actual);
        if !valid {
            return HealthFinding {
                kind: check_kind,
                state: HealthFindingState::Corrupt,
                evidence: HealthEvidenceCode::IndexMalformed,
                actual_digest: Some(actual_digest),
                expected_digest: Some(expected_digest),
            };
        }
        if actual != expected {
            return HealthFinding {
                kind: check_kind,
                state: HealthFindingState::Partial,
                evidence: HealthEvidenceCode::IndexStale,
                actual_digest: Some(actual_digest),
                expected_digest: Some(expected_digest),
            };
        }
        HealthFinding {
            kind: check_kind,
            state: HealthFindingState::Healthy,
            evidence: HealthEvidenceCode::Verified,
            actual_digest: Some(actual_digest.clone()),
            expected_digest: Some(actual_digest),
        }
    }

    fn ensure_index_root(&self) -> Result<(), HealthError> {
        let root = self.index_root();
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(error(HealthErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
            Err(error_value) if error_value.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&root).map_err(map_io_error)?;
            }
            Err(error_value) => return Err(map_io_error(error_value)),
        }
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(map_io_error)?;
        let marker = root.join(INDEX_MARKER);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error(HealthErrorCode::UnsafeEntry));
            }
            Ok(_) => {
                if fs::read(&marker).map_err(map_io_error)? != b"termirust-derived-indexes-v1\n" {
                    return Err(error(HealthErrorCode::UnsafeEntry));
                }
            }
            Err(value) if value.kind() == io::ErrorKind::NotFound => {
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                options.mode(0o600);
                let mut file = options.open(marker).map_err(map_io_error)?;
                file.write_all(b"termirust-derived-indexes-v1\n")
                    .map_err(map_io_error)?;
                file.sync_all().map_err(map_io_error)?;
            }
            Err(value) => return Err(map_io_error(value)),
        }
        Ok(())
    }

    fn write_journal(&self, journal: &RepairJournal) -> Result<(), HealthError> {
        let path = self.index_root().join(REPAIR_JOURNAL);
        let temporary = self.index_root().join(".repair-journal-v1.tmp");
        let _ = fs::remove_file(&temporary);
        let bytes =
            serde_json::to_vec(journal).map_err(|_| error(HealthErrorCode::VerificationFailed))?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).map_err(map_io_error)?;
        file.write_all(&bytes).map_err(map_io_error)?;
        file.sync_all().map_err(map_io_error)?;
        drop(file);
        fs::rename(&temporary, path).map_err(map_io_error)?;
        sync_directory(&self.index_root()).map_err(map_io_error)
    }

    fn recover_owned_repair(&self) -> Result<(), HealthError> {
        let root = self.index_root();
        let marker = root.join(INDEX_MARKER);
        let root_metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(value) if value.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(value) => return Err(map_io_error(value)),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(error(HealthErrorCode::UnsafeEntry));
        }
        let marker_metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) => metadata,
            Err(value) if value.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(value) => return Err(map_io_error(value)),
        };
        if marker_metadata.file_type().is_symlink()
            || !marker_metadata.is_file()
            || fs::read(&marker).map_err(map_io_error)? != b"termirust-derived-indexes-v1\n"
        {
            return Err(error(HealthErrorCode::UnsafeEntry));
        }
        let journal_path = root.join(REPAIR_JOURNAL);
        if journal_path.exists() {
            let bytes = read_regular_bounded(&journal_path, "repair journal", 64 * 1024)
                .map_err(map_store_error)?;
            let journal: RepairJournal = serde_json::from_slice(&bytes)
                .map_err(|_| error(HealthErrorCode::VerificationFailed))?;
            if journal.version != 1
                || journal.target_name != target_name(journal.kind)
                || !valid_owned_temp_name(&journal.temporary_name)
            {
                return Err(error(HealthErrorCode::UnsafeEntry));
            }
            let target = root.join(&journal.target_name);
            if let Ok(published) = read_regular_bounded(&target, "derived index", MAX_INDEX_BYTES)
                && sha256_hex(&published) != journal.expected_digest
            {
                return Err(error(HealthErrorCode::VerificationFailed));
            }
            let _ = fs::remove_file(root.join(&journal.temporary_name));
            let _ = fs::remove_file(&journal_path);
        }
        for entry in fs::read_dir(&root).map_err(map_io_error)? {
            let entry = entry.map_err(map_io_error)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if valid_owned_temp_name(name) {
                let metadata = entry.metadata().map_err(map_io_error)?;
                if metadata.is_file() {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }

    fn index_root(&self) -> PathBuf {
        self.root.join(INDEX_DIR)
    }
}

pub(crate) struct MetadataLock {
    file: File,
}

impl MetadataLock {
    pub(crate) fn shared(root: &Path) -> Result<Self, StoreError> {
        Self::acquire(root, false)
    }

    pub(crate) fn exclusive(root: &Path) -> Result<Self, StoreError> {
        Self::acquire(root, true)
    }

    fn acquire(root: &Path, exclusive: bool) -> Result<Self, StoreError> {
        let lock_path = root.join(LOCK_FILE);
        let metadata = fs::symlink_metadata(&lock_path).map_err(|error| StoreError::Io {
            operation: "inspect health metadata lock",
            kind: error.kind(),
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::UnsafeEntry {
                name: "metadata lock",
            });
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(lock_path).map_err(|error| StoreError::Io {
            operation: "open health metadata lock",
            kind: error.kind(),
        })?;
        let lock_result = if exclusive {
            file_lock::exclusive(&file)
        } else {
            file_lock::shared(&file)
        };
        lock_result.map_err(|error| StoreError::Io {
            operation: "lock health metadata",
            kind: error.kind(),
        })?;
        Ok(Self { file })
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        file_lock::release(&self.file);
    }
}

fn serialize_index<T: Serialize>(index: &T) -> Result<Vec<u8>, HealthError> {
    let bytes = serde_json::to_vec(index).map_err(|_| error(HealthErrorCode::CorruptSource))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(error(HealthErrorCode::SizeLimit));
    }
    Ok(bytes)
}

fn failed_report(id: HealthCheckId, code: HealthErrorCode) -> HealthReport {
    let (state, evidence) = finding_for_health_error(code);
    let kinds = [
        HealthCheckKind::StoreReadable,
        HealthCheckKind::StoreVersion,
        HealthCheckKind::RecordHashes,
        HealthCheckKind::ProjectSessionIndex,
        HealthCheckKind::PaletteIndex,
    ];
    HealthReport {
        id,
        findings: kinds
            .into_iter()
            .map(|kind| HealthFinding {
                kind,
                state: if code == HealthErrorCode::NewerSource
                    && kind == HealthCheckKind::StoreReadable
                {
                    HealthFindingState::Healthy
                } else if kind == HealthCheckKind::StoreReadable {
                    state
                } else if code == HealthErrorCode::NewerSource
                    && kind == HealthCheckKind::StoreVersion
                {
                    HealthFindingState::Newer
                } else {
                    HealthFindingState::Unavailable
                },
                evidence: if code == HealthErrorCode::NewerSource
                    && kind == HealthCheckKind::StoreReadable
                {
                    HealthEvidenceCode::Verified
                } else {
                    evidence
                },
                actual_digest: None,
                expected_digest: None,
            })
            .collect(),
        source_revisions: None,
        authoritative_records: 0,
    }
}

fn healthy_finding(kind: HealthCheckKind) -> HealthFinding {
    HealthFinding {
        kind,
        state: HealthFindingState::Healthy,
        evidence: HealthEvidenceCode::Verified,
        actual_digest: None,
        expected_digest: None,
    }
}

fn finding_for_health_error(code: HealthErrorCode) -> (HealthFindingState, HealthEvidenceCode) {
    match code {
        HealthErrorCode::NewerSource => (HealthFindingState::Newer, HealthEvidenceCode::StoreNewer),
        HealthErrorCode::PermissionDenied => (
            HealthFindingState::Permission,
            HealthEvidenceCode::PermissionDenied,
        ),
        HealthErrorCode::CorruptSource | HealthErrorCode::VerificationFailed => (
            HealthFindingState::Corrupt,
            HealthEvidenceCode::StoreMalformed,
        ),
        HealthErrorCode::UnsafeEntry => {
            (HealthFindingState::Corrupt, HealthEvidenceCode::StoreUnsafe)
        }
        HealthErrorCode::SizeLimit => (
            HealthFindingState::Corrupt,
            HealthEvidenceCode::StoreTooLarge,
        ),
        HealthErrorCode::Cancelled
        | HealthErrorCode::Unavailable
        | HealthErrorCode::StaleSource
        | HealthErrorCode::InjectedCrash => (
            HealthFindingState::Unavailable,
            HealthEvidenceCode::IoUnavailable,
        ),
    }
}

fn finding_for_store_error(error: &StoreError) -> (HealthFindingState, HealthEvidenceCode) {
    match error {
        StoreError::UnsafeEntry { .. } => {
            (HealthFindingState::Corrupt, HealthEvidenceCode::StoreUnsafe)
        }
        StoreError::TooLarge { .. } => (
            HealthFindingState::Corrupt,
            HealthEvidenceCode::StoreTooLarge,
        ),
        StoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        } => (
            HealthFindingState::Permission,
            HealthEvidenceCode::PermissionDenied,
        ),
        StoreError::Io { .. } => (
            HealthFindingState::Unavailable,
            HealthEvidenceCode::IoUnavailable,
        ),
        _ => (
            HealthFindingState::Corrupt,
            HealthEvidenceCode::IndexMalformed,
        ),
    }
}

fn classify_error(error: &StoreError) -> HealthErrorCode {
    map_store_error(error.clone()).code
}

fn map_store_error(store_error: StoreError) -> HealthError {
    match store_error {
        StoreError::StoreNewer { .. } => error(HealthErrorCode::NewerSource),
        StoreError::Corrupt { .. }
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_)
        | StoreError::InvalidInstanceId => error(HealthErrorCode::CorruptSource),
        StoreError::UnsafeEntry { .. } => error(HealthErrorCode::UnsafeEntry),
        StoreError::TooLarge { .. } => error(HealthErrorCode::SizeLimit),
        StoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        } => error(HealthErrorCode::PermissionDenied),
        StoreError::Io { .. } => error(HealthErrorCode::Unavailable),
    }
}

fn map_io_error(value: io::Error) -> HealthError {
    match value.kind() {
        io::ErrorKind::PermissionDenied => error(HealthErrorCode::PermissionDenied),
        io::ErrorKind::InvalidInput => error(HealthErrorCode::UnsafeEntry),
        _ => error(HealthErrorCode::Unavailable),
    }
}

fn error(code: HealthErrorCode) -> HealthError {
    HealthError { code }
}

fn check_cancelled(cancellation: &RepairCancellation) -> Result<(), HealthError> {
    if cancellation.is_cancelled() {
        Err(error(HealthErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn inject(
    selected: Option<RepairFaultPoint>,
    current: RepairFaultPoint,
) -> Result<(), HealthError> {
    if selected == Some(current) {
        Err(error(HealthErrorCode::InjectedCrash))
    } else {
        Ok(())
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), HealthError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(error(HealthErrorCode::UnsafeEntry))
        }
        Ok(_) => Ok(()),
        Err(value) if value.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(value) => Err(map_io_error(value)),
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn target_name(kind: IndexRepairKind) -> &'static str {
    match kind {
        IndexRepairKind::ProjectSessionIndex => PROJECT_SESSION_FILE,
        IndexRepairKind::PaletteIndex => PALETTE_FILE,
    }
}

fn kind_slug(kind: IndexRepairKind) -> &'static str {
    match kind {
        IndexRepairKind::ProjectSessionIndex => "project-session",
        IndexRepairKind::PaletteIndex => "palette",
    }
}

fn check_kind(kind: IndexRepairKind) -> HealthCheckKind {
    match kind {
        IndexRepairKind::ProjectSessionIndex => HealthCheckKind::ProjectSessionIndex,
        IndexRepairKind::PaletteIndex => HealthCheckKind::PaletteIndex,
    }
}

fn valid_owned_temp_name(name: &str) -> bool {
    name.starts_with(".repair-") && name.ends_with(".tmp") && !name.contains('/')
}
