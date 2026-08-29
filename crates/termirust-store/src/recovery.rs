use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use termirust_domain::Revision;
use uuid::Uuid;

use crate::health::MetadataLock;
use crate::presets::validate_preset_metadata_bytes;
use crate::projects::{read_regular_bounded, validate_project_metadata_bytes};
use crate::sessions::validate_session_metadata_bytes;
use crate::{
    AtomicWriter, HealthRepository, IndexRepairKind, RepairCancellation, StoreError,
    SystemAtomicWriter,
};

const FORMAT_FILE: &str = "format.json";
const RECOVERY_DIR: &str = "recovery";
const RECOVERY_MARKER: &str = ".termirust-metadata-recovery-v1";
const ACTIVE_JOURNAL: &str = "active-v1.json";
const MARKER_BYTES: &[u8] = b"termirust-metadata-recovery-v1\n";
const MAX_FORMAT_BYTES: u64 = 64 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    RestoreLastGoodMetadata,
    ReconcileHostLeases,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryResult {
    Restored,
    Reconciled,
    NoChange,
    Ambiguous,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryState {
    Inspecting,
    Planned,
    Confirming,
    BackingUpCurrent,
    Applying,
    Verifying,
    Complete,
    Cancelled,
    Failed,
    RollingBack,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStep {
    LockAndRecheck,
    BackupCurrent,
    VerifyBackup,
    PublishLastGood,
    ReopenAndVerify,
    RebuildDerivedIndexes,
    Rollback,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataFileKind {
    Projects,
    Sessions,
    Presets,
}

impl MetadataFileKind {
    const ALL: [Self; 3] = [Self::Projects, Self::Sessions, Self::Presets];

    fn primary_name(self) -> &'static str {
        match self {
            Self::Projects => "projects.json",
            Self::Sessions => "sessions.json",
            Self::Presets => "presets.json",
        }
    }

    fn last_good_name(self) -> &'static str {
        match self {
            Self::Projects => "projects.last-good.json",
            Self::Sessions => "sessions.last-good.json",
            Self::Presets => "presets.last-good.json",
        }
    }

    fn backup_name(self) -> &'static str {
        match self {
            Self::Projects => "projects.current.json",
            Self::Sessions => "sessions.current.json",
            Self::Presets => "presets.current.json",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RecoveryFilePlan {
    pub kind: MetadataFileKind,
    pub target_path: PathBuf,
    pub current_backup_path: PathBuf,
    pub last_good_path: PathBuf,
    pub current_revision: Option<Revision>,
    pub last_good_revision: Revision,
    pub current_sha256: Option<String>,
    pub current_bytes: u64,
    pub last_good_sha256: String,
    pub last_good_bytes: u64,
    current: Option<Vec<u8>>,
    last_good: Vec<u8>,
}

impl fmt::Debug for RecoveryFilePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoveryFilePlan")
            .field("kind", &self.kind)
            .field("target", &self.target_path.file_name())
            .field("backup", &self.current_backup_path.file_name())
            .field("last_good", &self.last_good_path.file_name())
            .field("current_revision", &self.current_revision)
            .field("last_good_revision", &self.last_good_revision)
            .field("current_sha256", &self.current_sha256)
            .field("current_bytes", &self.current_bytes)
            .field("last_good_sha256", &self.last_good_sha256)
            .field("last_good_bytes", &self.last_good_bytes)
            .finish()
    }
}

pub struct RecoveryPlan {
    pub id: Uuid,
    pub kind: RecoveryKind,
    pub files: Vec<RecoveryFilePlan>,
    pub unchanged_files: Vec<MetadataFileKind>,
    pub steps: Vec<RecoveryStep>,
    pub cancellation_boundary: RecoveryStep,
    pub estimated_backup_bytes: u64,
    pub state: RecoveryState,
}

impl fmt::Debug for RecoveryPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoveryPlan")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("files", &self.files)
            .field("unchanged_files", &self.unchanged_files)
            .field("steps", &self.steps)
            .field("cancellation_boundary", &self.cancellation_boundary)
            .field("estimated_backup_bytes", &self.estimated_backup_bytes)
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReceipt {
    pub id: Uuid,
    pub kind: RecoveryKind,
    pub result: RecoveryResult,
    pub state: RecoveryState,
    pub changed_files: Vec<MetadataFileKind>,
    pub backup_bytes: u64,
}

#[derive(Clone, Default)]
pub struct RecoveryCancellation(Arc<AtomicBool>);

impl RecoveryCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryErrorCode {
    Cancelled,
    NoLastGood,
    CorruptLastGood,
    NewerFormat,
    StaleRevision,
    PermissionDenied,
    UnsafeEntry,
    SizeLimit,
    StorageUnavailable,
    VerificationFailed,
    InjectedCrash,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryError {
    pub code: RecoveryErrorCode,
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "recovery operation failed: {:?}", self.code)
    }
}

impl std::error::Error for RecoveryError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub enum RecoveryFaultPoint {
    AfterBackup,
    AfterJournal,
    AfterFirstPublish,
    AfterAllPublish,
    AfterVerification,
}

#[derive(Clone)]
pub struct MetadataRecoveryService {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatDocument {
    format_version: u16,
    minimum_reader: u16,
    instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    kind: MetadataFileKind,
    target_name: String,
    backup_name: String,
    current_present: bool,
    current_sha256: Option<String>,
    last_good_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecoveryJournal {
    version: u16,
    plan_id: Uuid,
    entries: Vec<JournalEntry>,
    published: usize,
}

impl MetadataRecoveryService {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, RecoveryError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, RecoveryError> {
        let root = root.into();
        let metadata = fs::symlink_metadata(&root).map_err(map_io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(error(RecoveryErrorCode::UnsafeEntry));
        }
        Ok(Self { root, writer })
    }

    pub fn plan_restore_last_good(&self) -> Result<RecoveryPlan, RecoveryError> {
        validate_format(&self.root)?;
        let _lock = MetadataLock::shared(&self.root).map_err(map_store)?;
        let id = Uuid::new_v4();
        let backup_root = self.recovery_root().join(id.to_string());
        let mut files = Vec::new();
        let mut unchanged_files = Vec::new();
        for kind in MetadataFileKind::ALL {
            let target_path = self.root.join(kind.primary_name());
            let current =
                match read_regular_bounded(&target_path, kind.primary_name(), MAX_METADATA_BYTES) {
                    Ok(bytes) => Some(bytes),
                    Err(StoreError::Io {
                        kind: io::ErrorKind::NotFound,
                        ..
                    }) => None,
                    Err(error_value) => return Err(map_store(error_value)),
                };
            let current_revision = current
                .as_deref()
                .and_then(|bytes| validate_kind(kind, bytes).ok());
            if current_revision.is_some() {
                unchanged_files.push(kind);
                continue;
            }
            let last_good_path = self.root.join(kind.last_good_name());
            let last_good =
                read_regular_bounded(&last_good_path, kind.last_good_name(), MAX_METADATA_BYTES)
                    .map_err(map_last_good)?;
            let last_good_revision = validate_kind(kind, &last_good)
                .map_err(|_| error(RecoveryErrorCode::CorruptLastGood))?;
            files.push(RecoveryFilePlan {
                kind,
                target_path,
                current_backup_path: backup_root.join(kind.backup_name()),
                last_good_path,
                current_revision,
                last_good_revision,
                current_sha256: current.as_deref().map(sha256_hex),
                current_bytes: current.as_ref().map_or(0, |bytes| bytes.len() as u64),
                last_good_sha256: sha256_hex(&last_good),
                last_good_bytes: last_good.len() as u64,
                current,
                last_good,
            });
        }
        let estimated_backup_bytes = files.iter().map(|file| file.current_bytes).sum();
        Ok(RecoveryPlan {
            id,
            kind: RecoveryKind::RestoreLastGoodMetadata,
            files,
            unchanged_files,
            steps: vec![
                RecoveryStep::LockAndRecheck,
                RecoveryStep::BackupCurrent,
                RecoveryStep::VerifyBackup,
                RecoveryStep::PublishLastGood,
                RecoveryStep::ReopenAndVerify,
                RecoveryStep::RebuildDerivedIndexes,
                RecoveryStep::Rollback,
            ],
            cancellation_boundary: RecoveryStep::PublishLastGood,
            estimated_backup_bytes,
            state: RecoveryState::Planned,
        })
    }

    pub fn restore(
        &self,
        plan: RecoveryPlan,
        cancellation: &RecoveryCancellation,
    ) -> Result<RecoveryReceipt, RecoveryError> {
        self.restore_with_fault(plan, cancellation, None)
    }

    #[doc(hidden)]
    pub fn restore_with_fault(
        &self,
        plan: RecoveryPlan,
        cancellation: &RecoveryCancellation,
        fault: Option<RecoveryFaultPoint>,
    ) -> Result<RecoveryReceipt, RecoveryError> {
        if plan.kind != RecoveryKind::RestoreLastGoodMetadata {
            return Err(error(RecoveryErrorCode::VerificationFailed));
        }
        if plan.files.is_empty() {
            return Ok(RecoveryReceipt {
                id: plan.id,
                kind: plan.kind,
                result: RecoveryResult::NoChange,
                state: RecoveryState::Complete,
                changed_files: Vec::new(),
                backup_bytes: 0,
            });
        }
        check_cancelled(cancellation)?;
        self.ensure_recovery_root()?;
        let plan_root = self.recovery_root().join(plan.id.to_string());
        create_private_directory(&plan_root)?;
        let lock = MetadataLock::exclusive(&self.root).map_err(map_store)?;
        self.recheck_plan(&plan)?;
        check_cancelled(cancellation)?;
        for file in &plan.files {
            let bytes = file.current.as_deref().unwrap_or_default();
            self.writer
                .write(&file.current_backup_path, bytes)
                .map_err(map_io)?;
            let saved = read_regular_bounded(
                &file.current_backup_path,
                "recovery current backup",
                MAX_METADATA_BYTES,
            )
            .map_err(map_store)?;
            if saved != bytes
                || file.current_sha256.as_deref()
                    != file.current.as_deref().map(sha256_hex).as_deref()
            {
                return Err(error(RecoveryErrorCode::VerificationFailed));
            }
        }
        inject(fault, RecoveryFaultPoint::AfterBackup)?;
        check_cancelled(cancellation)?;
        let mut journal = RecoveryJournal {
            version: 1,
            plan_id: plan.id,
            entries: plan
                .files
                .iter()
                .map(|file| JournalEntry {
                    kind: file.kind,
                    target_name: file.kind.primary_name().to_string(),
                    backup_name: file.kind.backup_name().to_string(),
                    current_present: file.current.is_some(),
                    current_sha256: file.current_sha256.clone(),
                    last_good_sha256: file.last_good_sha256.clone(),
                })
                .collect(),
            published: 0,
        };
        self.write_journal(&journal)?;
        inject(fault, RecoveryFaultPoint::AfterJournal)?;

        for (index, file) in plan.files.iter().enumerate() {
            if let Err(apply_error) = self.writer.write(&file.target_path, &file.last_good) {
                return self.rollback_or_required(&journal, map_io(apply_error));
            }
            journal.published = index + 1;
            if let Err(journal_error) = self.write_journal(&journal) {
                return self.rollback_or_required(&journal, journal_error);
            }
            if index == 0 {
                inject(fault, RecoveryFaultPoint::AfterFirstPublish)?;
            }
        }
        inject(fault, RecoveryFaultPoint::AfterAllPublish)?;
        for file in &plan.files {
            let restored = match read_regular_bounded(
                &file.target_path,
                file.kind.primary_name(),
                MAX_METADATA_BYTES,
            ) {
                Ok(bytes) => bytes,
                Err(value) => return self.rollback_or_required(&journal, map_store(value)),
            };
            if restored != file.last_good
                || sha256_hex(&restored) != file.last_good_sha256
                || validate_kind(file.kind, &restored).is_err()
            {
                return self
                    .rollback_or_required(&journal, error(RecoveryErrorCode::VerificationFailed));
            }
        }
        inject(fault, RecoveryFaultPoint::AfterVerification)?;
        drop(lock);
        if let Err(value) = self.rebuild_indexes() {
            let _lock = MetadataLock::exclusive(&self.root).map_err(map_store)?;
            return self.rollback_or_required(&journal, value);
        }
        self.remove_active_journal()?;
        Ok(RecoveryReceipt {
            id: plan.id,
            kind: plan.kind,
            result: RecoveryResult::Restored,
            state: RecoveryState::Complete,
            changed_files: plan.files.iter().map(|file| file.kind).collect(),
            backup_bytes: plan.estimated_backup_bytes,
        })
    }

    /// Rolls back only a marker-owned, journaled operation. It never starts a new restore.
    pub fn recover_interrupted_restore(&self) -> Result<Option<RecoveryReceipt>, RecoveryError> {
        let Some(journal) = self.read_active_journal()? else {
            return Ok(None);
        };
        let _lock = MetadataLock::exclusive(&self.root).map_err(map_store)?;
        self.rollback_journal(&journal)?;
        self.remove_active_journal()?;
        Ok(Some(RecoveryReceipt {
            id: journal.plan_id,
            kind: RecoveryKind::RestoreLastGoodMetadata,
            result: RecoveryResult::RolledBack,
            state: RecoveryState::RolledBack,
            changed_files: journal.entries.iter().map(|entry| entry.kind).collect(),
            backup_bytes: journal
                .entries
                .iter()
                .filter_map(|entry| {
                    fs::metadata(
                        self.recovery_root()
                            .join(journal.plan_id.to_string())
                            .join(&entry.backup_name),
                    )
                    .ok()
                    .map(|metadata| metadata.len())
                })
                .sum(),
        }))
    }

    fn recheck_plan(&self, plan: &RecoveryPlan) -> Result<(), RecoveryError> {
        validate_format(&self.root)?;
        for file in &plan.files {
            let current = match read_regular_bounded(
                &file.target_path,
                file.kind.primary_name(),
                MAX_METADATA_BYTES,
            ) {
                Ok(bytes) => Some(bytes),
                Err(StoreError::Io {
                    kind: io::ErrorKind::NotFound,
                    ..
                }) => None,
                Err(value) => return Err(map_store(value)),
            };
            if current.as_deref().map(sha256_hex) != file.current_sha256 {
                return Err(error(RecoveryErrorCode::StaleRevision));
            }
            let last_good = read_regular_bounded(
                &file.last_good_path,
                file.kind.last_good_name(),
                MAX_METADATA_BYTES,
            )
            .map_err(map_last_good)?;
            if sha256_hex(&last_good) != file.last_good_sha256
                || validate_kind(file.kind, &last_good).ok() != Some(file.last_good_revision)
            {
                return Err(error(RecoveryErrorCode::StaleRevision));
            }
        }
        Ok(())
    }

    fn rebuild_indexes(&self) -> Result<(), RecoveryError> {
        let health = HealthRepository::open(&self.root).map_err(map_health)?;
        for kind in [
            IndexRepairKind::ProjectSessionIndex,
            IndexRepairKind::PaletteIndex,
        ] {
            let plan = health.plan_repair(kind).map_err(map_health)?;
            health
                .repair(plan, &RepairCancellation::default())
                .map_err(map_health)?;
        }
        Ok(())
    }

    fn rollback_or_required<T>(
        &self,
        journal: &RecoveryJournal,
        original: RecoveryError,
    ) -> Result<T, RecoveryError> {
        match self.rollback_journal(journal) {
            Ok(()) => {
                let _ = self.remove_active_journal();
                Err(original)
            }
            Err(_) => Err(error(RecoveryErrorCode::RecoveryRequired)),
        }
    }

    fn rollback_journal(&self, journal: &RecoveryJournal) -> Result<(), RecoveryError> {
        validate_journal(journal)?;
        let plan_root = self.recovery_root().join(journal.plan_id.to_string());
        for entry in &journal.entries {
            let target = self.root.join(&entry.target_name);
            let backup = plan_root.join(&entry.backup_name);
            let bytes =
                read_regular_bounded(&backup, "recovery current backup", MAX_METADATA_BYTES)
                    .map_err(map_store)?;
            if entry.current_sha256.as_deref() != Some(sha256_hex(&bytes).as_str())
                && entry.current_present
            {
                return Err(error(RecoveryErrorCode::VerificationFailed));
            }
            if entry.current_present {
                self.writer.write(&target, &bytes).map_err(map_io)?;
                let restored =
                    read_regular_bounded(&target, entry.kind.primary_name(), MAX_METADATA_BYTES)
                        .map_err(map_store)?;
                if restored != bytes {
                    return Err(error(RecoveryErrorCode::VerificationFailed));
                }
            } else {
                match fs::symlink_metadata(&target) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                        return Err(error(RecoveryErrorCode::UnsafeEntry));
                    }
                    Ok(_) => fs::remove_file(&target).map_err(map_io)?,
                    Err(value) if value.kind() == io::ErrorKind::NotFound => {}
                    Err(value) => return Err(map_io(value)),
                }
            }
        }
        Ok(())
    }

    fn ensure_recovery_root(&self) -> Result<(), RecoveryError> {
        let root = self.recovery_root();
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(error(RecoveryErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
            Err(value) if value.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&root)?;
            }
            Err(value) => return Err(map_io(value)),
        }
        let marker = root.join(RECOVERY_MARKER);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error(RecoveryErrorCode::UnsafeEntry));
            }
            Ok(_) => {
                if fs::read(&marker).map_err(map_io)? != MARKER_BYTES {
                    return Err(error(RecoveryErrorCode::UnsafeEntry));
                }
            }
            Err(value) if value.kind() == io::ErrorKind::NotFound => {
                create_private_file(&marker, MARKER_BYTES)?;
            }
            Err(value) => return Err(map_io(value)),
        }
        Ok(())
    }

    fn write_journal(&self, journal: &RecoveryJournal) -> Result<(), RecoveryError> {
        let bytes = serde_json::to_vec(journal)
            .map_err(|_| error(RecoveryErrorCode::VerificationFailed))?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(error(RecoveryErrorCode::SizeLimit));
        }
        self.writer
            .write(&self.recovery_root().join(ACTIVE_JOURNAL), &bytes)
            .map_err(map_io)?;
        Ok(())
    }

    fn read_active_journal(&self) -> Result<Option<RecoveryJournal>, RecoveryError> {
        let root = self.recovery_root();
        match fs::symlink_metadata(&root) {
            Err(value) if value.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(value) => return Err(map_io(value)),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(error(RecoveryErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
        }
        let marker = read_regular_bounded(&root.join(RECOVERY_MARKER), "recovery marker", 1024)
            .map_err(map_store)?;
        if marker != MARKER_BYTES {
            return Err(error(RecoveryErrorCode::UnsafeEntry));
        }
        let bytes = match read_regular_bounded(
            &root.join(ACTIVE_JOURNAL),
            "recovery journal",
            MAX_JOURNAL_BYTES,
        ) {
            Ok(bytes) => bytes,
            Err(StoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }) => return Ok(None),
            Err(value) => return Err(map_store(value)),
        };
        let journal: RecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|_| error(RecoveryErrorCode::VerificationFailed))?;
        validate_journal(&journal)?;
        Ok(Some(journal))
    }

    fn remove_active_journal(&self) -> Result<(), RecoveryError> {
        match fs::remove_file(self.recovery_root().join(ACTIVE_JOURNAL)) {
            Ok(()) => Ok(()),
            Err(value) if value.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(value) => Err(map_io(value)),
        }
    }

    fn recovery_root(&self) -> PathBuf {
        self.root.join(RECOVERY_DIR)
    }
}

fn validate_format(root: &Path) -> Result<(), RecoveryError> {
    let bytes = read_regular_bounded(&root.join(FORMAT_FILE), FORMAT_FILE, MAX_FORMAT_BYTES)
        .map_err(map_store)?;
    let format: FormatDocument =
        serde_json::from_slice(&bytes).map_err(|_| error(RecoveryErrorCode::VerificationFailed))?;
    if format.format_version > crate::CURRENT_FORMAT_VERSION
        || format.minimum_reader > crate::CURRENT_FORMAT_VERSION
    {
        return Err(error(RecoveryErrorCode::NewerFormat));
    }
    if format.format_version != crate::CURRENT_FORMAT_VERSION
        || format.minimum_reader == 0
        || Uuid::parse_str(&format.instance_id).is_err()
    {
        return Err(error(RecoveryErrorCode::VerificationFailed));
    }
    Ok(())
}

fn validate_kind(kind: MetadataFileKind, bytes: &[u8]) -> Result<Revision, StoreError> {
    match kind {
        MetadataFileKind::Projects => validate_project_metadata_bytes(bytes),
        MetadataFileKind::Sessions => validate_session_metadata_bytes(bytes),
        MetadataFileKind::Presets => validate_preset_metadata_bytes(bytes),
    }
}

fn validate_journal(journal: &RecoveryJournal) -> Result<(), RecoveryError> {
    if journal.version != 1
        || journal.entries.is_empty()
        || journal.entries.len() > MetadataFileKind::ALL.len()
        || journal.published > journal.entries.len()
        || journal.entries.iter().any(|entry| {
            entry.target_name != entry.kind.primary_name()
                || entry.backup_name != entry.kind.backup_name()
                || (entry.current_present && entry.current_sha256.is_none())
        })
    {
        return Err(error(RecoveryErrorCode::UnsafeEntry));
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), RecoveryError> {
    fs::create_dir(path).map_err(map_io)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_io)?;
    Ok(())
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<(), RecoveryError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    use std::io::Write as _;
    let mut file = options.open(path).map_err(map_io)?;
    file.write_all(bytes).map_err(map_io)?;
    file.sync_all().map_err(map_io)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn check_cancelled(cancellation: &RecoveryCancellation) -> Result<(), RecoveryError> {
    if cancellation.is_cancelled() {
        Err(error(RecoveryErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn inject(
    selected: Option<RecoveryFaultPoint>,
    current: RecoveryFaultPoint,
) -> Result<(), RecoveryError> {
    if selected == Some(current) {
        Err(error(RecoveryErrorCode::InjectedCrash))
    } else {
        Ok(())
    }
}

fn map_last_good(value: StoreError) -> RecoveryError {
    match value {
        StoreError::Io {
            kind: io::ErrorKind::NotFound,
            ..
        } => error(RecoveryErrorCode::NoLastGood),
        StoreError::Corrupt { .. } => error(RecoveryErrorCode::CorruptLastGood),
        other => map_store(other),
    }
}

fn map_store(value: StoreError) -> RecoveryError {
    match value {
        StoreError::UnsafeEntry { .. } => error(RecoveryErrorCode::UnsafeEntry),
        StoreError::TooLarge { .. } => error(RecoveryErrorCode::SizeLimit),
        StoreError::StoreNewer { .. } => error(RecoveryErrorCode::NewerFormat),
        StoreError::Corrupt { .. }
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_)
        | StoreError::InvalidInstanceId => error(RecoveryErrorCode::VerificationFailed),
        StoreError::Io { kind, .. } => map_io(io::Error::from(kind)),
    }
}

fn map_health(value: crate::HealthError) -> RecoveryError {
    use crate::HealthErrorCode;
    error(match value.code {
        HealthErrorCode::Cancelled => RecoveryErrorCode::Cancelled,
        HealthErrorCode::NewerSource => RecoveryErrorCode::NewerFormat,
        HealthErrorCode::PermissionDenied => RecoveryErrorCode::PermissionDenied,
        HealthErrorCode::UnsafeEntry => RecoveryErrorCode::UnsafeEntry,
        HealthErrorCode::SizeLimit => RecoveryErrorCode::SizeLimit,
        HealthErrorCode::StaleSource => RecoveryErrorCode::StaleRevision,
        HealthErrorCode::CorruptSource | HealthErrorCode::VerificationFailed => {
            RecoveryErrorCode::VerificationFailed
        }
        HealthErrorCode::Unavailable | HealthErrorCode::InjectedCrash => {
            RecoveryErrorCode::StorageUnavailable
        }
    })
}

fn map_io(value: io::Error) -> RecoveryError {
    error(match value.kind() {
        io::ErrorKind::PermissionDenied => RecoveryErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput => RecoveryErrorCode::UnsafeEntry,
        _ => RecoveryErrorCode::StorageUnavailable,
    })
}

const fn error(code: RecoveryErrorCode) -> RecoveryError {
    RecoveryError { code }
}
