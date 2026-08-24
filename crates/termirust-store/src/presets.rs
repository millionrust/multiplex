use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

use serde::{Deserialize, Serialize};
use termirust_domain::{
    LaunchPreset, MAX_PRESETS, PositionKey, PresetDraft, PresetError, PresetId, PresetService,
    Revision,
};

use crate::{
    AtomicWriter, Durability, ProjectRepository, StoreError, StoreHealth, SystemAtomicWriter,
};

const PRESETS_FILE: &str = "presets.json";
const PRESETS_BACKUP_FILE: &str = "presets.last-good.json";
const LOCK_FILE: &str = "metadata.lock";
const MAX_PRESETS_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetSnapshot {
    pub revision: Revision,
    pub presets: Vec<LaunchPreset>,
    pub health: StoreHealth,
    pub read_only: bool,
    pub durability: Durability,
}

#[derive(Clone)]
pub struct PresetRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PresetsDocument {
    revision: Revision,
    presets: Vec<LaunchPreset>,
}

impl PresetRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        // The project repository owns the shared format marker and root hardening.
        ProjectRepository::open(root.clone())?;
        let repository = Self { root, writer };
        let _lock = repository.acquire_lock()?;
        if !repository.root.join(PRESETS_FILE).exists() {
            repository.write_document_locked(&PresetsDocument {
                revision: Revision::ZERO,
                presets: Vec::new(),
            })?;
        }
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load(&self) -> Result<PresetSnapshot, StoreError> {
        let _lock = self.acquire_lock()?;
        match self.read_document(PRESETS_FILE) {
            Ok(document) => Ok(snapshot(document, StoreHealth::Healthy, false)),
            Err(StoreError::Corrupt { .. }) => {
                let backup = self.read_document(PRESETS_BACKUP_FILE)?;
                Ok(snapshot(backup, StoreHealth::RecoveredLastGood, true))
            }
            Err(error) => Err(error),
        }
    }

    pub fn save_preset(
        &self,
        draft: PresetDraft,
        expected: Revision,
    ) -> Result<LaunchPreset, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(PRESETS_FILE)?;
        sort_presets(&mut document.presets);

        if let Some(existing) = document.presets.iter().find(|preset| preset.id == draft.id) {
            let normalized = draft
                .clone()
                .validate(existing.position, existing.revision)?;
            if same_preset_content(existing, &normalized) {
                return Ok(existing.clone());
            }
        }
        require_revision(expected, document.revision)?;
        let revision = next_revision(document.revision)?;
        let existing_index = document
            .presets
            .iter()
            .position(|preset| preset.id == draft.id);
        let position = match existing_index {
            Some(index) => document.presets[index].position,
            None => {
                if document.presets.len() >= MAX_PRESETS {
                    return Err(PresetError::ResourceLimit { limit: MAX_PRESETS }.into());
                }
                next_tail_position(&mut document.presets)?
            }
        };
        let preset = draft.validate(position, revision)?;
        if preset.favorite {
            for candidate in &mut document.presets {
                if candidate.id != preset.id {
                    candidate.favorite = false;
                    candidate.revision = revision;
                }
            }
        }
        match existing_index {
            Some(index) => document.presets[index] = preset.clone(),
            None => document.presets.push(preset.clone()),
        }
        document.revision = revision;
        sort_presets(&mut document.presets);
        self.write_document_locked(&document)?;
        Ok(preset)
    }

    pub fn remove_preset(
        &self,
        id: PresetId,
        expected: Revision,
    ) -> Result<LaunchPreset, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(PRESETS_FILE)?;
        require_revision(expected, document.revision)?;
        let index = document
            .presets
            .iter()
            .position(|preset| preset.id == id)
            .ok_or(PresetError::Unavailable)?;
        let removed = document.presets.remove(index);
        document.revision = next_revision(document.revision)?;
        self.write_document_locked(&document)?;
        Ok(removed)
    }

    pub fn move_preset_before(
        &self,
        id: PresetId,
        before: Option<PresetId>,
        _expected: Revision,
    ) -> Result<LaunchPreset, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(PRESETS_FILE)?;
        sort_presets(&mut document.presets);
        let original_order: Vec<_> = document.presets.iter().map(|preset| preset.id).collect();
        if before == Some(id) {
            return document
                .presets
                .into_iter()
                .find(|preset| preset.id == id)
                .ok_or(PresetError::Unavailable.into());
        }
        let old_index = document
            .presets
            .iter()
            .position(|preset| preset.id == id)
            .ok_or(PresetError::Unavailable)?;
        let preset = document.presets.remove(old_index);
        let new_index = match before {
            Some(before_id) => document
                .presets
                .iter()
                .position(|candidate| candidate.id == before_id)
                .ok_or(PresetError::Unavailable)?,
            None => document.presets.len(),
        };
        document.presets.insert(new_index, preset);
        let next_order: Vec<_> = document.presets.iter().map(|preset| preset.id).collect();
        if next_order == original_order {
            return Ok(document.presets[new_index].clone());
        }
        assign_position_at(&mut document.presets, new_index)?;
        let revision = next_revision(document.revision)?;
        document.revision = revision;
        document.presets[new_index].revision = revision;
        let moved = document.presets[new_index].clone();
        self.write_document_locked(&document)?;
        Ok(moved)
    }

    fn read_document(&self, name: &'static str) -> Result<PresetsDocument, StoreError> {
        let path = self.root.join(name);
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| io_error("inspect presets", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::UnsafeEntry { name });
        }
        if metadata.len() > MAX_PRESETS_BYTES {
            return Err(StoreError::TooLarge {
                name,
                limit: MAX_PRESETS_BYTES,
            });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(path)
            .and_then(|file| file.take(MAX_PRESETS_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| io_error("read presets", error))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PRESETS_BYTES {
            return Err(StoreError::TooLarge {
                name,
                limit: MAX_PRESETS_BYTES,
            });
        }
        let mut document: PresetsDocument =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt { name })?;
        validate_document(&document).map_err(|_| StoreError::Corrupt { name })?;
        sort_presets(&mut document.presets);
        Ok(document)
    }

    fn write_document_locked(&self, document: &PresetsDocument) -> Result<(), StoreError> {
        validate_document(document)?;
        let bytes = serde_json::to_vec_pretty(document).map_err(|_| StoreError::Corrupt {
            name: "preset serialization",
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PRESETS_BYTES {
            return Err(StoreError::TooLarge {
                name: PRESETS_FILE,
                limit: MAX_PRESETS_BYTES,
            });
        }
        self.writer
            .write(&self.root.join(PRESETS_FILE), &bytes)
            .map_err(|error| io_error("commit presets", error))?;
        let persisted = self.read_document(PRESETS_FILE)?;
        if persisted != *document {
            return Err(StoreError::Corrupt { name: PRESETS_FILE });
        }
        let _ = self
            .writer
            .write(&self.root.join(PRESETS_BACKUP_FILE), &bytes);
        Ok(())
    }

    fn acquire_lock(&self) -> Result<MetadataLock, StoreError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(self.root.join(LOCK_FILE))
            .map_err(|error| io_error("open metadata lock", error))?;
        #[cfg(unix)]
        {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(io_error("lock metadata", io::Error::last_os_error()));
            }
        }
        Ok(MetadataLock { file })
    }
}

impl PresetService for PresetRepository {
    fn list(&self) -> Result<Vec<LaunchPreset>, PresetError> {
        self.load()
            .map(|snapshot| snapshot.presets)
            .map_err(store_as_domain)
    }

    fn save(&self, draft: PresetDraft, expected: Revision) -> Result<LaunchPreset, PresetError> {
        self.save_preset(draft, expected).map_err(store_as_domain)
    }

    fn remove(&self, id: PresetId, expected: Revision) -> Result<(), PresetError> {
        self.remove_preset(id, expected)
            .map(|_| ())
            .map_err(store_as_domain)
    }
}

struct MetadataLock {
    file: File,
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn snapshot(document: PresetsDocument, health: StoreHealth, read_only: bool) -> PresetSnapshot {
    PresetSnapshot {
        revision: document.revision,
        presets: document.presets,
        health,
        read_only,
        durability: Durability::Full,
    }
}

fn validate_document(document: &PresetsDocument) -> Result<(), PresetError> {
    if document.presets.len() > MAX_PRESETS {
        return Err(PresetError::ResourceLimit { limit: MAX_PRESETS });
    }
    let mut ids = HashSet::with_capacity(document.presets.len());
    let mut positions = HashSet::with_capacity(document.presets.len());
    let mut favorites = 0;
    for preset in &document.presets {
        if preset.revision > document.revision {
            return Err(PresetError::Store {
                code: "future-preset-revision",
            });
        }
        if !ids.insert(preset.id) || !positions.insert(preset.position) {
            return Err(PresetError::Store {
                code: "duplicate-preset",
            });
        }
        if preset.favorite {
            favorites += 1;
        }
        let normalized = preset
            .to_draft()
            .validate(preset.position, preset.revision)?;
        if !same_preset_content(preset, &normalized) {
            return Err(PresetError::Store {
                code: "invalid-preset",
            });
        }
    }
    if favorites > 1 {
        return Err(PresetError::Store {
            code: "multiple-favorites",
        });
    }
    Ok(())
}

fn same_preset_content(left: &LaunchPreset, right: &LaunchPreset) -> bool {
    left.id == right.id
        && left.label == right.label
        && left.executable == right.executable
        && left.args == right.args
        && left.working_directory == right.working_directory
        && left.runtime == right.runtime
        && left.enabled == right.enabled
        && left.favorite == right.favorite
        && left.position == right.position
        && left.permission_policy == right.permission_policy
        && left.origin == right.origin
        && left.risk == right.risk
}

fn sort_presets(presets: &mut [LaunchPreset]) {
    presets.sort_by_key(|preset| (preset.position, preset.id));
}

fn require_revision(expected: Revision, actual: Revision) -> Result<(), StoreError> {
    if expected != actual {
        return Err(PresetError::StaleRevision { expected, actual }.into());
    }
    Ok(())
}

fn next_revision(revision: Revision) -> Result<Revision, StoreError> {
    revision.next().ok_or(PresetError::RevisionOverflow.into())
}

fn next_tail_position(presets: &mut [LaunchPreset]) -> Result<PositionKey, StoreError> {
    sort_presets(presets);
    match presets.last() {
        None => Ok(PositionKey::FIRST),
        Some(preset) => preset.position.after().map_err(|_| {
            PresetError::Store {
                code: "position-overflow",
            }
            .into()
        }),
    }
}

fn assign_position_at(presets: &mut [LaunchPreset], index: usize) -> Result<(), StoreError> {
    let candidate = match (index.checked_sub(1), presets.get(index + 1)) {
        (None, Some(right)) if right.position.get() > 1 => {
            Some(PositionKey::new(right.position.get() / 2))
        }
        (Some(left_index), Some(right)) => {
            PositionKey::between(presets[left_index].position, right.position).ok()
        }
        (Some(left_index), None) => presets[left_index].position.after().ok(),
        (None, None) => Some(PositionKey::FIRST),
        _ => None,
    };
    if let Some(position) = candidate {
        presets[index].position = position;
        return Ok(());
    }
    for (position_index, preset) in presets.iter_mut().enumerate() {
        preset.position =
            PositionKey::rebalanced(position_index).map_err(|_| PresetError::Store {
                code: "position-overflow",
            })?;
    }
    Ok(())
}

fn store_as_domain(error: StoreError) -> PresetError {
    match error {
        StoreError::PresetDomain(error) => error,
        StoreError::StoreNewer { .. } => PresetError::Store {
            code: "newer-format",
        },
        StoreError::Corrupt { .. }
        | StoreError::UnsafeEntry { .. }
        | StoreError::TooLarge { .. } => PresetError::Store { code: "corrupt" },
        StoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        } => PresetError::Store {
            code: "permission-denied",
        },
        StoreError::Io { .. } | StoreError::InvalidInstanceId | StoreError::Domain(_) => {
            PresetError::Store {
                code: "unavailable",
            }
        }
    }
}

fn io_error(operation: &'static str, error: io::Error) -> StoreError {
    StoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use termirust_domain::{PermissionPolicy, PresetOrigin, WorkingDirectoryRule};
    use uuid::Uuid;

    fn id(value: u128) -> PresetId {
        PresetId::from_uuid(Uuid::from_u128(value))
    }

    fn draft(value: u128, label: &str) -> PresetDraft {
        PresetDraft {
            id: id(value),
            label: label.to_string(),
            executable: "codex".to_string(),
            args: vec!["--model".to_string(), format!("literal-{value}")],
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: Some("codex".to_string()),
            enabled: true,
            favorite: false,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::User,
            confirm_risky_favorite: false,
        }
    }

    fn repository(root: &Path) -> PresetRepository {
        PresetRepository::open(root).unwrap()
    }

    #[test]
    fn presets_crud_order_and_restart_are_deterministic() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(fixture.path());
        repo.save_preset(draft(1, "One"), Revision::ZERO).unwrap();
        repo.save_preset(draft(2, "Two"), Revision::new(1)).unwrap();
        repo.move_preset_before(id(2), Some(id(1)), Revision::new(2))
            .unwrap();
        drop(repo);

        let reopened = repository(fixture.path());
        let snapshot = reopened.load().unwrap();
        assert_eq!(
            snapshot
                .presets
                .iter()
                .map(|preset| preset.id)
                .collect::<Vec<_>>(),
            vec![id(2), id(1)]
        );
        reopened.remove_preset(id(1), snapshot.revision).unwrap();
        assert_eq!(reopened.load().unwrap().presets.len(), 1);
    }

    #[test]
    fn identical_stale_save_is_idempotent_but_conflict_requires_reload() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(fixture.path());
        let original = draft(1, "One");
        repo.save_preset(original.clone(), Revision::ZERO).unwrap();
        assert_eq!(
            repo.save_preset(original, Revision::ZERO).unwrap().revision,
            Revision::new(1)
        );
        assert!(matches!(
            repo.save_preset(draft(1, "Changed"), Revision::ZERO),
            Err(StoreError::PresetDomain(PresetError::StaleRevision { .. }))
        ));
    }

    #[test]
    fn concurrent_stale_writers_commit_once() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = Arc::new(repository(fixture.path()));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = [draft(1, "One"), draft(2, "Two")]
            .into_iter()
            .map(|draft| {
                let repo = repo.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    repo.save_preset(draft, Revision::ZERO)
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(repo.load().unwrap().presets.len(), 1);
    }

    #[test]
    fn favorite_is_unique_and_risky_favorite_requires_confirmation() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(fixture.path());
        let mut first = draft(1, "One");
        first.favorite = true;
        repo.save_preset(first, Revision::ZERO).unwrap();
        let mut second = draft(2, "Two");
        second.favorite = true;
        repo.save_preset(second, Revision::new(1)).unwrap();
        let snapshot = repo.load().unwrap();
        assert_eq!(
            snapshot
                .presets
                .iter()
                .filter(|preset| preset.favorite)
                .count(),
            1
        );
        assert!(
            snapshot
                .presets
                .iter()
                .find(|preset| preset.id == id(2))
                .unwrap()
                .favorite
        );

        let mut risky = draft(3, "Risky");
        risky.args = vec!["--dangerously-bypass-approvals-and-sandbox".to_string()];
        risky.favorite = true;
        assert!(matches!(
            repo.save_preset(risky, snapshot.revision),
            Err(StoreError::PresetDomain(
                PresetError::RiskConfirmationRequired
            ))
        ));
    }

    #[test]
    fn corrupt_primary_recovers_last_good_read_only() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(fixture.path());
        repo.save_preset(draft(1, "One"), Revision::ZERO).unwrap();
        fs::write(repo.root.join(PRESETS_FILE), b"{broken").unwrap();
        let snapshot = repo.load().unwrap();
        assert!(snapshot.read_only);
        assert_eq!(snapshot.health, StoreHealth::RecoveredLastGood);
        assert_eq!(snapshot.presets.len(), 1);
    }
}
