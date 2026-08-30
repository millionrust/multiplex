use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{
    GroupId, HostedSession, HostedSessionId, MAX_SESSIONS_PER_PROJECT, PositionKey, ProjectId,
    Revision, SessionMutation, SessionStateError, SessionTitle, reduce_session,
};

use crate::{
    AtomicWriter, Durability, ProjectRepository, StoreError, StoreHealth, SystemAtomicWriter,
};

const SESSIONS_FILE: &str = "sessions.json";
const SESSIONS_BACKUP_FILE: &str = "sessions.last-good.json";
const LOCK_FILE: &str = "metadata.lock";
const MAX_SESSIONS_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REMOVAL_ENTRIES: usize = 10_000;
const INTERACTIVE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub revision: Revision,
    pub sessions: Vec<HostedSession>,
    pub health: StoreHealth,
    pub read_only: bool,
    pub durability: Durability,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionRemovalManifest {
    pub metadata_bytes: u64,
    pub journal_bytes: u64,
    pub transcript_bytes: u64,
    pub artifact_bytes: u64,
    pub file_count: usize,
}

impl SessionRemovalManifest {
    pub fn total_bytes(self) -> u64 {
        self.metadata_bytes
            .saturating_add(self.journal_bytes)
            .saturating_add(self.transcript_bytes)
            .saturating_add(self.artifact_bytes)
    }

    pub fn requires_title_confirmation(self) -> bool {
        self.transcript_bytes > 0 || self.artifact_bytes > 0
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SessionRemovalPlan {
    pub session_id: HostedSessionId,
    pub expected_revision: Revision,
    pub title: SessionTitle,
    pub manifest: SessionRemovalManifest,
}

impl std::fmt::Debug for SessionRemovalPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionRemovalPlan")
            .field("session_id", &self.session_id)
            .field("expected_revision", &self.expected_revision)
            .field("title", &"<redacted>")
            .field("manifest", &self.manifest)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedSession {
    pub session: HostedSession,
    pub manifest: SessionRemovalManifest,
    pub data_quarantined: bool,
}

#[derive(Clone)]
pub struct SessionRepository {
    root: PathBuf,
    data_root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionsDocument {
    revision: Revision,
    sessions: Vec<HostedSession>,
}

pub(crate) fn read_session_health_source(
    root: &Path,
) -> Result<(Vec<u8>, Revision, Vec<HostedSession>), StoreError> {
    let bytes = crate::projects::read_regular_bounded(
        &root.join(SESSIONS_FILE),
        SESSIONS_FILE,
        MAX_SESSIONS_BYTES,
    )?;
    let mut document: SessionsDocument =
        serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt {
            name: SESSIONS_FILE,
        })?;
    validate_document(&document)?;
    sort_sessions(&mut document.sessions);
    Ok((bytes, document.revision, document.sessions))
}

pub(crate) fn validate_session_metadata_bytes(bytes: &[u8]) -> Result<Revision, StoreError> {
    let document: SessionsDocument =
        serde_json::from_slice(bytes).map_err(|_| StoreError::Corrupt {
            name: SESSIONS_FILE,
        })?;
    validate_document(&document)?;
    Ok(document.revision)
}

impl SessionRepository {
    pub(crate) fn load_existing_read_only(
        root: impl Into<PathBuf>,
    ) -> Result<SessionSnapshot, StoreError> {
        let repository = Self {
            root: root.into(),
            data_root: PathBuf::new(),
            writer: Arc::new(SystemAtomicWriter),
        };
        match repository.read_document(SESSIONS_FILE) {
            Ok(document) => Ok(snapshot(document, StoreHealth::Healthy, false)),
            Err(StoreError::Corrupt { .. })
            | Err(StoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }) => {
                let backup = repository.read_document(SESSIONS_BACKUP_FILE)?;
                Ok(snapshot(backup, StoreHealth::RecoveredLastGood, true))
            }
            Err(error) => Err(error),
        }
    }

    pub fn open(
        root: impl Into<PathBuf>,
        data_root: impl Into<PathBuf>,
    ) -> Result<Self, StoreError> {
        Self::open_with(root, data_root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        data_root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        let data_root = data_root.into();
        ProjectRepository::open(root.clone())?;
        create_user_only_directory(&data_root)?;
        let repository = Self {
            root,
            data_root,
            writer,
        };
        let _lock = repository.acquire_lock()?;
        if !repository.root.join(SESSIONS_FILE).exists()
            && !repository.root.join(SESSIONS_BACKUP_FILE).exists()
        {
            repository.write_document_locked(&SessionsDocument {
                revision: Revision::ZERO,
                sessions: Vec::new(),
            })?;
        }
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_data_path(&self, id: HostedSessionId) -> PathBuf {
        self.data_root.join(id.to_string())
    }

    pub fn load(&self) -> Result<SessionSnapshot, StoreError> {
        let _lock = self.acquire_lock()?;
        match self.read_document(SESSIONS_FILE) {
            Ok(document) => Ok(snapshot(document, StoreHealth::Healthy, false)),
            Err(StoreError::Corrupt { .. })
            | Err(StoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }) => {
                let backup = self.read_document(SESSIONS_BACKUP_FILE)?;
                Ok(snapshot(backup, StoreHealth::RecoveredLastGood, true))
            }
            Err(error) => Err(error),
        }
    }

    pub fn create_session(
        &self,
        mut session: HostedSession,
        expected: Revision,
    ) -> Result<HostedSession, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(SESSIONS_FILE)?;
        if let Some(existing) = document
            .sessions
            .iter()
            .find(|existing| existing.id == session.id)
        {
            return Ok(existing.clone());
        }
        require_revision(expected, document.revision)?;
        let project_count = document
            .sessions
            .iter()
            .filter(|candidate| candidate.project_id == session.project_id)
            .count();
        if project_count >= MAX_SESSIONS_PER_PROJECT {
            return Err(SessionStateError::ResourceLimit {
                limit: MAX_SESSIONS_PER_PROJECT,
            }
            .into());
        }
        session.position =
            next_tail_position(&document.sessions, session.project_id, session.group_id)?;
        let revision = next_revision(document.revision)?;
        session.revision = revision;
        document.revision = revision;
        document.sessions.push(session.clone());
        sort_sessions(&mut document.sessions);
        self.write_document_locked(&document)?;
        Ok(session)
    }

    pub fn mutate_session(
        &self,
        id: HostedSessionId,
        expected: Revision,
        mutation: SessionMutation,
        updated_at: u64,
    ) -> Result<HostedSession, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(SESSIONS_FILE)?;
        let index = document
            .sessions
            .iter()
            .position(|session| session.id == id)
            .ok_or(SessionStateError::Unavailable)?;
        let mut candidate = document.sessions[index].clone();
        let changed = reduce_session(&mut candidate, mutation)?;
        if !changed {
            return Ok(candidate);
        }
        require_revision(expected, document.revision)?;
        let revision = next_revision(document.revision)?;
        candidate.revision = revision;
        candidate.updated_at = updated_at;
        document.revision = revision;
        document.sessions[index] = candidate.clone();
        sort_sessions(&mut document.sessions);
        self.write_document_locked(&document)?;
        Ok(candidate)
    }

    pub fn move_session_before(
        &self,
        id: HostedSessionId,
        group_id: Option<GroupId>,
        before: Option<HostedSessionId>,
        expected: Revision,
        updated_at: u64,
    ) -> Result<HostedSession, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(SESSIONS_FILE)?;
        require_revision(expected, document.revision)?;
        sort_sessions(&mut document.sessions);
        let moving_index = document
            .sessions
            .iter()
            .position(|session| session.id == id)
            .ok_or(SessionStateError::Unavailable)?;
        if before == Some(id) {
            return Ok(document.sessions[moving_index].clone());
        }
        let project_id = document.sessions[moving_index].project_id;
        let old_order = destination_ids(&document.sessions, project_id, group_id);
        let moving = document.sessions.remove(moving_index);
        let mut destination = destination_ids(&document.sessions, project_id, group_id);
        let insert_at = match before {
            Some(before_id) => destination
                .iter()
                .position(|candidate| *candidate == before_id)
                .ok_or(SessionStateError::Unavailable)?,
            None => destination.len(),
        };
        destination.insert(insert_at, id);
        if moving.group_id == group_id && destination == old_order {
            return Ok(moving);
        }
        document.sessions.push(moving);
        rebalance_destination(&mut document.sessions, project_id, group_id, &destination)?;
        let revision = next_revision(document.revision)?;
        document.revision = revision;
        let moved = document
            .sessions
            .iter_mut()
            .find(|session| session.id == id)
            .ok_or(SessionStateError::Unavailable)?;
        moved.group_id = group_id;
        moved.revision = revision;
        moved.updated_at = updated_at;
        let moved = moved.clone();
        sort_sessions(&mut document.sessions);
        self.write_document_locked(&document)?;
        Ok(moved)
    }

    pub fn removal_plan(&self, id: HostedSessionId) -> Result<SessionRemovalPlan, StoreError> {
        let _lock = self.acquire_lock()?;
        let document = self.read_document(SESSIONS_FILE)?;
        let session = document
            .sessions
            .iter()
            .find(|session| session.id == id)
            .ok_or(SessionStateError::Unavailable)?;
        if !session.can_remove() {
            return Err(SessionStateError::RemoveRequiresExitedArchive.into());
        }
        let manifest = self.scan_removal_manifest(session)?;
        Ok(SessionRemovalPlan {
            session_id: id,
            expected_revision: document.revision,
            title: session.title.clone(),
            manifest,
        })
    }

    pub fn apply_placements(
        &self,
        placements: &[(HostedSessionId, Option<GroupId>, PositionKey)],
        expected: Revision,
        updated_at: u64,
    ) -> Result<Vec<HostedSession>, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(SESSIONS_FILE)?;
        require_revision(expected, document.revision)?;
        let mut seen = HashSet::with_capacity(placements.len());
        if placements.iter().any(|(id, _, _)| !seen.insert(*id)) {
            return Err(SessionStateError::Store {
                code: "duplicate-placement",
            }
            .into());
        }
        for (id, _, _) in placements {
            if !document.sessions.iter().any(|session| session.id == *id) {
                return Err(SessionStateError::Unavailable.into());
            }
        }
        let changed = placements.iter().any(|(id, group_id, position)| {
            document.sessions.iter().any(|session| {
                session.id == *id
                    && (session.group_id != *group_id || session.position != *position)
            })
        });
        if !changed {
            return Ok(placements
                .iter()
                .filter_map(|(id, _, _)| {
                    document
                        .sessions
                        .iter()
                        .find(|session| session.id == *id)
                        .cloned()
                })
                .collect());
        }
        let revision = next_revision(document.revision)?;
        for (id, group_id, position) in placements {
            let session = document
                .sessions
                .iter_mut()
                .find(|session| session.id == *id)
                .ok_or(SessionStateError::Unavailable)?;
            session.group_id = *group_id;
            session.position = *position;
            session.revision = revision;
            session.updated_at = updated_at;
        }
        document.revision = revision;
        sort_sessions(&mut document.sessions);
        self.write_document_locked(&document)?;
        Ok(placements
            .iter()
            .filter_map(|(id, _, _)| {
                document
                    .sessions
                    .iter()
                    .find(|session| session.id == *id)
                    .cloned()
            })
            .collect())
    }

    pub fn remove_session(
        &self,
        plan: &SessionRemovalPlan,
        expected: Revision,
    ) -> Result<QuarantinedSession, StoreError> {
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document(SESSIONS_FILE)?;
        require_revision(expected, document.revision)?;
        if plan.expected_revision != expected {
            return Err(SessionStateError::StaleRevision {
                expected: plan.expected_revision,
                actual: expected,
            }
            .into());
        }
        let index = document
            .sessions
            .iter()
            .position(|session| session.id == plan.session_id)
            .ok_or(SessionStateError::Unavailable)?;
        let session = document.sessions[index].clone();
        if !session.can_remove() {
            return Err(SessionStateError::RemoveRequiresExitedArchive.into());
        }
        let manifest = self.scan_removal_manifest(&session)?;
        if manifest != plan.manifest {
            return Err(SessionStateError::Store {
                code: "removal-plan-changed",
            }
            .into());
        }

        let source = self.session_data_path(session.id);
        let source_exists = match fs::symlink_metadata(&source) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(StoreError::UnsafeEntry {
                        name: "session data",
                    });
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(io_error("inspect session data", error)),
        };
        let quarantine_root = self.data_root.with_file_name("durable-session-quarantine");
        let quarantine = quarantine_root.join(session.id.to_string());
        if source_exists {
            create_user_only_directory(&quarantine_root)?;
            if quarantine.exists() {
                return Err(SessionStateError::Store {
                    code: "quarantine-conflict",
                }
                .into());
            }
            fs::rename(&source, &quarantine)
                .map_err(|error| io_error("quarantine session data", error))?;
        }

        document.sessions.remove(index);
        document.revision = next_revision(document.revision)?;
        if let Err(error) = self.write_document_locked(&document) {
            if source_exists {
                let _ = fs::rename(&quarantine, &source);
            }
            return Err(error);
        }
        Ok(QuarantinedSession {
            session,
            manifest,
            data_quarantined: source_exists,
        })
    }

    fn scan_removal_manifest(
        &self,
        session: &HostedSession,
    ) -> Result<SessionRemovalManifest, StoreError> {
        let mut manifest = SessionRemovalManifest {
            metadata_bytes: u64::try_from(
                serde_json::to_vec(session)
                    .map_err(|_| StoreError::Corrupt {
                        name: "session serialization",
                    })?
                    .len(),
            )
            .unwrap_or(u64::MAX),
            ..SessionRemovalManifest::default()
        };
        let root = self.session_data_path(session.id);
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(manifest),
            Err(error) => return Err(io_error("inspect session data", error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StoreError::UnsafeEntry {
                name: "session data",
            });
        }
        let mut pending = vec![(root.clone(), PathBuf::new())];
        while let Some((directory, relative)) = pending.pop() {
            for entry in
                fs::read_dir(&directory).map_err(|error| io_error("read session data", error))?
            {
                let entry = entry.map_err(|error| io_error("read session data", error))?;
                manifest.file_count = manifest.file_count.saturating_add(1);
                if manifest.file_count > MAX_REMOVAL_ENTRIES {
                    return Err(SessionStateError::ResourceLimit {
                        limit: MAX_REMOVAL_ENTRIES,
                    }
                    .into());
                }
                let entry_relative = relative.join(entry.file_name());
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|error| io_error("inspect session data", error))?;
                if metadata.file_type().is_symlink() {
                    return Err(StoreError::UnsafeEntry {
                        name: "session data",
                    });
                }
                if metadata.is_dir() {
                    pending.push((entry.path(), entry_relative));
                } else if metadata.is_file() {
                    add_manifest_bytes(&mut manifest, &entry_relative, metadata.len())?;
                } else {
                    return Err(StoreError::UnsafeEntry {
                        name: "session data",
                    });
                }
            }
        }
        Ok(manifest)
    }

    fn read_document(&self, name: &'static str) -> Result<SessionsDocument, StoreError> {
        let path = self.root.join(name);
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| io_error("inspect sessions", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::UnsafeEntry { name });
        }
        if metadata.len() > MAX_SESSIONS_BYTES {
            return Err(StoreError::TooLarge {
                name,
                limit: MAX_SESSIONS_BYTES,
            });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(path)
            .and_then(|file| file.take(MAX_SESSIONS_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| io_error("read sessions", error))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSIONS_BYTES {
            return Err(StoreError::TooLarge {
                name,
                limit: MAX_SESSIONS_BYTES,
            });
        }
        let mut document: SessionsDocument =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt { name })?;
        validate_document(&document)?;
        sort_sessions(&mut document.sessions);
        Ok(document)
    }

    fn write_document_locked(&self, document: &SessionsDocument) -> Result<(), StoreError> {
        validate_document(document)?;
        let bytes = serde_json::to_vec_pretty(document).map_err(|_| StoreError::Corrupt {
            name: "session serialization",
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSIONS_BYTES {
            return Err(StoreError::TooLarge {
                name: SESSIONS_FILE,
                limit: MAX_SESSIONS_BYTES,
            });
        }
        self.writer
            .write(&self.root.join(SESSIONS_FILE), &bytes)
            .map_err(|error| io_error("commit sessions", error))?;
        let persisted = self.read_document(SESSIONS_FILE)?;
        if persisted != *document {
            return Err(StoreError::Corrupt {
                name: SESSIONS_FILE,
            });
        }
        let _ = self
            .writer
            .write(&self.root.join(SESSIONS_BACKUP_FILE), &bytes);
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
            let deadline = Instant::now() + INTERACTIVE_LOCK_TIMEOUT;
            loop {
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                let retryable = error
                    .raw_os_error()
                    .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
                if !retryable || Instant::now() >= deadline {
                    return Err(io_error("lock session metadata", error));
                }
                thread::sleep(LOCK_RETRY_INTERVAL);
            }
        }
        Ok(MetadataLock { file })
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

fn snapshot(document: SessionsDocument, health: StoreHealth, read_only: bool) -> SessionSnapshot {
    SessionSnapshot {
        revision: document.revision,
        sessions: document.sessions,
        health,
        read_only,
        durability: Durability::Full,
    }
}

fn validate_document(document: &SessionsDocument) -> Result<(), StoreError> {
    let mut ids = HashSet::with_capacity(document.sessions.len());
    let mut positions = HashSet::with_capacity(document.sessions.len());
    let mut project_counts = HashMap::<ProjectId, usize>::new();
    for session in &document.sessions {
        if session.revision > document.revision {
            return Err(SessionStateError::Store {
                code: "future-session-revision",
            }
            .into());
        }
        if !ids.insert(session.id)
            || !positions.insert((session.project_id, session.group_id, session.position))
        {
            return Err(SessionStateError::Store {
                code: "duplicate-session",
            }
            .into());
        }
        let count = project_counts.entry(session.project_id).or_default();
        *count = count.saturating_add(1);
        if *count > MAX_SESSIONS_PER_PROJECT {
            return Err(SessionStateError::ResourceLimit {
                limit: MAX_SESSIONS_PER_PROJECT,
            }
            .into());
        }
        if SessionTitle::new(session.title.as_str())? != session.title
            || session.activity.validate().is_err()
            || session.read_through_sequence > session.last_output_sequence
            || session
                .unread_sequence
                .is_some_and(|sequence| sequence > session.last_output_sequence)
            || (session.archived_at.is_some() && !session.lifecycle.is_exited())
        {
            return Err(SessionStateError::Store {
                code: "invalid-session",
            }
            .into());
        }
    }
    Ok(())
}

fn sort_sessions(sessions: &mut [HostedSession]) {
    sessions.sort_by_key(|session| {
        (
            session.project_id,
            session.group_id,
            session.position,
            session.id,
        )
    });
}

fn destination_ids(
    sessions: &[HostedSession],
    project_id: ProjectId,
    group_id: Option<GroupId>,
) -> Vec<HostedSessionId> {
    let mut sessions = sessions
        .iter()
        .filter(|session| session.project_id == project_id && session.group_id == group_id)
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| (session.position, session.id));
    sessions.into_iter().map(|session| session.id).collect()
}

fn next_tail_position(
    sessions: &[HostedSession],
    project_id: ProjectId,
    group_id: Option<GroupId>,
) -> Result<PositionKey, StoreError> {
    let last = sessions
        .iter()
        .filter(|session| session.project_id == project_id && session.group_id == group_id)
        .max_by_key(|session| (session.position, session.id));
    match last {
        None => Ok(PositionKey::FIRST),
        Some(session) => session.position.after().map_err(|_| {
            SessionStateError::Store {
                code: "position-overflow",
            }
            .into()
        }),
    }
}

fn rebalance_destination(
    sessions: &mut [HostedSession],
    project_id: ProjectId,
    group_id: Option<GroupId>,
    ordered_ids: &[HostedSessionId],
) -> Result<(), StoreError> {
    for (index, id) in ordered_ids.iter().enumerate() {
        let session = sessions
            .iter_mut()
            .find(|session| session.id == *id)
            .ok_or(SessionStateError::Unavailable)?;
        if session.project_id != project_id {
            return Err(SessionStateError::Store {
                code: "cross-project-move",
            }
            .into());
        }
        session.group_id = group_id;
        session.position =
            PositionKey::rebalanced(index).map_err(|_| SessionStateError::Store {
                code: "position-overflow",
            })?;
    }
    Ok(())
}

fn add_manifest_bytes(
    manifest: &mut SessionRemovalManifest,
    relative: &Path,
    bytes: u64,
) -> Result<(), StoreError> {
    let first = relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .unwrap_or_default();
    let file_name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let target = if first == "transcripts" {
        &mut manifest.transcript_bytes
    } else if first == "artifacts" {
        &mut manifest.artifact_bytes
    } else if file_name.starts_with("journal-") || file_name.ends_with(".trj") {
        &mut manifest.journal_bytes
    } else {
        &mut manifest.metadata_bytes
    };
    *target = target.checked_add(bytes).ok_or(SessionStateError::Store {
        code: "removal-size-overflow",
    })?;
    Ok(())
}

fn require_revision(expected: Revision, actual: Revision) -> Result<(), StoreError> {
    if expected != actual {
        return Err(SessionStateError::StaleRevision { expected, actual }.into());
    }
    Ok(())
}

fn next_revision(revision: Revision) -> Result<Revision, StoreError> {
    revision
        .next()
        .ok_or(SessionStateError::RevisionOverflow.into())
}

fn io_error(operation: &'static str, error: io::Error) -> StoreError {
    StoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(unix)]
fn create_user_only_directory(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(StoreError::UnsafeEntry {
                name: "session directory",
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| io_error("create session directory", error))?
        }
        Err(error) => return Err(io_error("inspect session directory", error)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("secure session directory", error))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_user_only_directory(path: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(path).map_err(|error| io_error("create session directory", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use termirust_domain::{
        HostedSessionState, OutputSequence, PresetId, SessionTitle, TitleSource,
    };
    use uuid::Uuid;

    fn session(project: u128, id: u128, state: HostedSessionState) -> HostedSession {
        HostedSession {
            id: HostedSessionId::from_uuid(Uuid::from_u128(id)),
            project_id: ProjectId::from_uuid(Uuid::from_u128(project)),
            group_id: None,
            preset_id: Some(PresetId::from_uuid(Uuid::from_u128(99))),
            title: SessionTitle::new(&format!("Session {id}")).unwrap(),
            title_source: TitleSource::Default,
            lifecycle: state,
            activity: termirust_domain::ActivityAggregate::default(),
            pinned: false,
            position: PositionKey::FIRST,
            last_output_sequence: OutputSequence::ZERO,
            read_through_sequence: OutputSequence::ZERO,
            unread_sequence: None,
            archived_at: None,
            created_at: 1,
            updated_at: 1,
            revision: Revision::ZERO,
        }
    }

    fn repository() -> (tempfile::TempDir, SessionRepository) {
        let fixture = tempfile::tempdir().unwrap();
        let repository = SessionRepository::open(
            fixture.path().join("metadata"),
            fixture.path().join("durable-sessions"),
        )
        .unwrap();
        (fixture, repository)
    }

    #[test]
    fn sessions_revisioned_mutations_survive_restart_and_reject_stale_writers() {
        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 10, HostedSessionState::Live), Revision::ZERO)
            .unwrap();
        assert_eq!(created.revision, Revision::new(1));
        let renamed = repository
            .mutate_session(
                created.id,
                Revision::new(1),
                SessionMutation::Rename(SessionTitle::new("Manual").unwrap()),
                2,
            )
            .unwrap();
        assert_eq!(renamed.revision, Revision::new(2));
        assert!(matches!(
            repository.mutate_session(
                created.id,
                Revision::new(1),
                SessionMutation::SetPinned(true),
                3,
            ),
            Err(StoreError::SessionDomain(
                SessionStateError::StaleRevision { .. }
            ))
        ));
        let reopened = SessionRepository::open(
            fixture.path().join("metadata"),
            fixture.path().join("durable-sessions"),
        )
        .unwrap();
        let snapshot = reopened.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(2));
        assert_eq!(snapshot.sessions[0].title.as_str(), "Manual");
    }

    #[test]
    fn activity_replay_persists_attention_and_exact_read_watermarks_across_restart() {
        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 20, HostedSessionState::Live), Revision::ZERO)
            .unwrap();
        let observed = repository
            .mutate_session(
                created.id,
                created.revision,
                SessionMutation::ObserveOutput {
                    through: OutputSequence::new(5),
                },
                2,
            )
            .unwrap();
        assert!(!observed.unread(), "ordinary output is not attention");
        let needs_input = termirust_domain::ActivityAggregate {
            state: termirust_domain::ActivityState::NeedsInput,
            confidence: termirust_domain::ActivityConfidence::Verified,
            effective_sequence: termirust_domain::HostSequence::new(1),
            source_kind: termirust_domain::ActivitySourceKind::Approval,
            source_id: "store-test".to_string(),
            stale: false,
            attention_reason: Some(termirust_domain::AttentionReason::Approval),
            attention_sequence: Some(OutputSequence::new(5)),
            ..termirust_domain::ActivityAggregate::default()
        };
        let attention = repository
            .mutate_session(
                created.id,
                observed.revision,
                SessionMutation::ApplyActivity {
                    activity: needs_input,
                    visible_through: None,
                },
                3,
            )
            .unwrap();
        assert_eq!(attention.unread_sequence, Some(OutputSequence::new(5)));

        let reopened = SessionRepository::open(
            fixture.path().join("metadata"),
            fixture.path().join("durable-sessions"),
        )
        .unwrap();
        let restored = reopened.load().unwrap().sessions.remove(0);
        assert_eq!(
            restored.activity.state,
            termirust_domain::ActivityState::NeedsInput
        );
        assert!(restored.unread());
        let read = reopened
            .mutate_session(
                restored.id,
                restored.revision,
                SessionMutation::MarkRead {
                    through: OutputSequence::new(5),
                },
                4,
            )
            .unwrap();
        assert!(!read.unread());
        assert_eq!(read.read_through_sequence, OutputSequence::new(5));
    }

    #[test]
    fn sessions_concurrent_writers_commit_once_and_report_stale_revision() {
        let (_fixture, repository) = repository();
        let repository = Arc::new(repository);
        let barrier = Arc::new(Barrier::new(3));
        let handles = [10_u128, 11_u128]
            .into_iter()
            .map(|id| {
                let repository = Arc::clone(&repository);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    repository
                        .create_session(session(1, id, HostedSessionState::Exited), Revision::ZERO)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::SessionDomain(
                        SessionStateError::StaleRevision { .. }
                    ))
                ))
                .count(),
            1
        );
        assert_eq!(repository.load().unwrap().sessions.len(), 1);
    }

    #[test]
    fn sessions_corrupt_primary_recovers_last_good_read_only_without_overwrite() {
        let (_fixture, repository) = repository();
        repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        let path = repository.root.join(SESSIONS_FILE);
        fs::write(&path, b"{broken").unwrap();
        let corrupt = fs::read(&path).unwrap();

        let recovered = repository.load().unwrap();
        assert_eq!(recovered.health, StoreHealth::RecoveredLastGood);
        assert!(recovered.read_only);
        assert_eq!(recovered.sessions.len(), 1);
        assert!(matches!(
            repository.mutate_session(
                recovered.sessions[0].id,
                recovered.revision,
                SessionMutation::SetPinned(true),
                3,
            ),
            Err(StoreError::Corrupt { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), corrupt);

        fs::remove_file(&path).unwrap();
        let missing_primary = repository.load().unwrap();
        assert_eq!(missing_primary.health, StoreHealth::RecoveredLastGood);
        assert!(missing_primary.read_only);
        assert_eq!(missing_primary.sessions.len(), 1);
        assert!(!path.exists());
    }

    #[test]
    fn sessions_ordering_is_stable_and_move_is_idempotent() {
        let (_fixture, repository) = repository();
        let first = repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        let second = repository
            .create_session(session(1, 11, HostedSessionState::Exited), Revision::new(1))
            .unwrap();
        let moved = repository
            .move_session_before(second.id, None, Some(first.id), Revision::new(2), 5)
            .unwrap();
        assert_eq!(moved.position, PositionKey::FIRST);
        let snapshot = repository.load().unwrap();
        assert_eq!(snapshot.sessions[0].id, second.id);
        let same = repository
            .move_session_before(second.id, None, Some(first.id), snapshot.revision, 6)
            .unwrap();
        assert_eq!(same.id, second.id);
        assert_eq!(repository.load().unwrap().revision, snapshot.revision);
    }

    #[test]
    fn sessions_remove_requires_exited_archive_and_quarantines_only_owned_data() {
        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        assert!(matches!(
            repository.removal_plan(created.id),
            Err(StoreError::SessionDomain(
                SessionStateError::RemoveRequiresExitedArchive
            ))
        ));
        let archived = repository
            .mutate_session(
                created.id,
                Revision::new(1),
                SessionMutation::Archive { at: 20 },
                20,
            )
            .unwrap();
        let data = fixture
            .path()
            .join("durable-sessions")
            .join(archived.id.to_string());
        fs::create_dir_all(data.join("transcripts")).unwrap();
        fs::write(data.join("journal-000001.trj"), b"journal").unwrap();
        fs::write(data.join("transcripts/one.json"), b"transcript").unwrap();
        let sentinel = fixture.path().join("project-sentinel");
        fs::write(&sentinel, b"keep").unwrap();

        let plan = repository.removal_plan(archived.id).unwrap();
        assert_eq!(plan.manifest.journal_bytes, 7);
        assert_eq!(plan.manifest.transcript_bytes, 10);
        assert!(plan.manifest.requires_title_confirmation());
        let removed = repository
            .remove_session(&plan, plan.expected_revision)
            .unwrap();
        assert!(removed.data_quarantined);
        assert!(!data.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        assert!(repository.load().unwrap().sessions.is_empty());
        assert!(
            fixture
                .path()
                .join("durable-session-quarantine")
                .join(archived.id.to_string())
                .is_dir()
        );
    }

    #[test]
    fn sessions_removal_rejects_changed_manifest_and_preserves_evidence() {
        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        let archived = repository
            .mutate_session(
                created.id,
                created.revision,
                SessionMutation::Archive { at: 20 },
                20,
            )
            .unwrap();
        let data = fixture
            .path()
            .join("durable-sessions")
            .join(archived.id.to_string());
        fs::create_dir_all(data.join("artifacts")).unwrap();
        let plan = repository.removal_plan(archived.id).unwrap();
        fs::write(data.join("artifacts/new.txt"), b"changed").unwrap();

        assert!(matches!(
            repository.remove_session(&plan, plan.expected_revision),
            Err(StoreError::SessionDomain(SessionStateError::Store {
                code: "removal-plan-changed"
            }))
        ));
        assert!(data.join("artifacts/new.txt").is_file());
        assert_eq!(repository.load().unwrap().sessions.len(), 1);
    }

    #[test]
    fn sessions_enforce_ten_thousand_records_per_project() {
        let (_fixture, repository) = repository();
        let mut sessions = Vec::with_capacity(MAX_SESSIONS_PER_PROJECT);
        for index in 0..MAX_SESSIONS_PER_PROJECT {
            let mut value = session(
                1,
                u128::try_from(index).unwrap() + 1,
                HostedSessionState::Exited,
            );
            value.position = PositionKey::rebalanced(index).unwrap();
            sessions.push(value);
        }
        repository
            .write_document_locked(&SessionsDocument {
                revision: Revision::new(1),
                sessions,
            })
            .unwrap();
        assert!(matches!(
            repository.create_session(
                session(1, 20_000, HostedSessionState::Exited),
                Revision::new(1),
            ),
            Err(StoreError::SessionDomain(
                SessionStateError::ResourceLimit {
                    limit: MAX_SESSIONS_PER_PROJECT
                }
            ))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sessions_removal_rejects_symlink_evidence_without_mutation() {
        use std::os::unix::fs::symlink;

        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        repository
            .mutate_session(
                created.id,
                Revision::new(1),
                SessionMutation::Archive { at: 20 },
                20,
            )
            .unwrap();
        let data = fixture
            .path()
            .join("durable-sessions")
            .join(created.id.to_string());
        fs::create_dir_all(&data).unwrap();
        let outside = fixture.path().join("outside");
        fs::write(&outside, b"sentinel").unwrap();
        symlink(&outside, data.join("unsafe-link")).unwrap();
        assert!(matches!(
            repository.removal_plan(created.id),
            Err(StoreError::UnsafeEntry { .. })
        ));
        assert_eq!(fs::read(&outside).unwrap(), b"sentinel");
        assert_eq!(repository.load().unwrap().sessions.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn sessions_removal_rejects_broken_session_data_symlink_without_mutation() {
        use std::os::unix::fs::symlink;

        let (fixture, repository) = repository();
        let created = repository
            .create_session(session(1, 10, HostedSessionState::Exited), Revision::ZERO)
            .unwrap();
        repository
            .mutate_session(
                created.id,
                Revision::new(1),
                SessionMutation::Archive { at: 20 },
                20,
            )
            .unwrap();
        symlink(
            fixture.path().join("missing-session-data"),
            fixture
                .path()
                .join("durable-sessions")
                .join(created.id.to_string()),
        )
        .unwrap();

        assert!(matches!(
            repository.removal_plan(created.id),
            Err(StoreError::UnsafeEntry {
                name: "session data"
            })
        ));
        assert_eq!(repository.load().unwrap().sessions.len(), 1);
    }
}
