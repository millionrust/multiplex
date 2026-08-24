use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{
    AddProject, CanonicalPath, LocalizedUserText, PositionKey, Project, ProjectError, ProjectId,
    ProjectService, ProjectSummary, Revision,
};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

pub const CURRENT_FORMAT_VERSION: u16 = 1;
const MINIMUM_READER_VERSION: u16 = 1;
const MAX_FORMAT_BYTES: u64 = 64 * 1024;
const MAX_PROJECTS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PROJECTS: usize = 1_000;
const FORMAT_FILE: &str = "format.json";
const PROJECTS_FILE: &str = "projects.json";
const PROJECTS_BACKUP_FILE: &str = "projects.last-good.json";
const LOCK_FILE: &str = "metadata.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreHealth {
    Healthy,
    RecoveredLastGood,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSnapshot {
    pub revision: Revision,
    pub projects: Vec<ProjectSummary>,
    pub health: StoreHealth,
    pub read_only: bool,
    pub durability: Durability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    UnsafeEntry {
        name: &'static str,
    },
    TooLarge {
        name: &'static str,
        limit: u64,
    },
    Corrupt {
        name: &'static str,
    },
    StoreNewer {
        found: u16,
        supported: u16,
    },
    InvalidInstanceId,
    Domain(ProjectError),
    PresetDomain(termirust_domain::PresetError),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(formatter, "project store {operation} failed ({kind:?})")
            }
            Self::UnsafeEntry { name } => write!(
                formatter,
                "project store entry {name} is not a regular file"
            ),
            Self::TooLarge { name, limit } => write!(
                formatter,
                "project store entry {name} exceeds {limit} bytes"
            ),
            Self::Corrupt { name } => write!(formatter, "project store entry {name} is corrupt"),
            Self::StoreNewer { found, supported } => write!(
                formatter,
                "project store format {found} is newer than supported format {supported}"
            ),
            Self::InvalidInstanceId => formatter.write_str("project store instance ID is invalid"),
            Self::Domain(error) => error.fmt(formatter),
            Self::PresetDomain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<ProjectError> for StoreError {
    fn from(error: ProjectError) -> Self {
        Self::Domain(error)
    }
}

impl From<termirust_domain::PresetError> for StoreError {
    fn from(error: termirust_domain::PresetError) -> Self {
        Self::PresetDomain(error)
    }
}

#[derive(Clone)]
pub struct ProjectRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FormatDocument {
    format_version: u16,
    minimum_reader: u16,
    instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectsDocument {
    revision: Revision,
    projects: Vec<Project>,
}

impl ProjectRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with(
            root,
            uuid::Uuid::new_v4().to_string(),
            Arc::new(SystemAtomicWriter),
        )
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        instance_id: String,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, StoreError> {
        if uuid::Uuid::parse_str(&instance_id).is_err() {
            return Err(StoreError::InvalidInstanceId);
        }
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.acquire_lock()?;
        repository.initialize_locked(&instance_id)?;
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load(&self) -> Result<ProjectSnapshot, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        self.load_locked()
    }

    pub fn add_project(&self, request: AddProject) -> Result<Project, StoreError> {
        let canonical_root = CanonicalPath::resolve(&request.root)?;
        let display_name = match request.display_name.as_deref() {
            Some(value) => LocalizedUserText::new(value)?,
            None => canonical_root.display_name()?,
        };

        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(request.expected, document.revision)?;
        if let Some(existing) = document
            .projects
            .iter()
            .find(|project| project.canonical_root.identity() == canonical_root.identity())
        {
            return Err(StoreError::Domain(ProjectError::AlreadyPresent {
                id: existing.id,
            }));
        }
        if document.projects.len() >= MAX_PROJECTS {
            return Err(StoreError::Domain(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS,
            }));
        }
        let revision = next_revision(document.revision)?;
        let position = next_tail_position(&mut document.projects)?;
        let project = Project {
            id: request.id,
            display_name,
            canonical_root,
            position,
            revision,
        };
        document.projects.push(project.clone());
        document.revision = revision;
        sort_projects(&mut document.projects);
        self.write_document_locked(&document)?;
        Ok(project)
    }

    pub fn remove_project(&self, id: ProjectId, expected: Revision) -> Result<Project, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = document
            .projects
            .iter()
            .position(|project| project.id == id)
            .ok_or(StoreError::Domain(ProjectError::Unavailable))?;
        let removed = document.projects.remove(index);
        document.revision = next_revision(document.revision)?;
        self.write_document_locked(&document)?;
        Ok(removed)
    }

    pub fn restore_project(
        &self,
        mut project: Project,
        expected: Revision,
    ) -> Result<Project, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        if document.projects.len() >= MAX_PROJECTS {
            return Err(StoreError::Domain(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS,
            }));
        }
        if let Some(existing) = document.projects.iter().find(|candidate| {
            candidate.id == project.id
                || candidate.canonical_root.identity() == project.canonical_root.identity()
        }) {
            return Err(StoreError::Domain(ProjectError::AlreadyPresent {
                id: existing.id,
            }));
        }
        let revision = next_revision(document.revision)?;
        project.revision = revision;
        project.position = next_tail_position(&mut document.projects)?;
        document.projects.push(project.clone());
        document.revision = revision;
        sort_projects(&mut document.projects);
        self.write_document_locked(&document)?;
        Ok(project)
    }

    pub fn move_project_before(
        &self,
        id: ProjectId,
        before: Option<ProjectId>,
        expected: Revision,
    ) -> Result<Project, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        sort_projects(&mut document.projects);
        let original_order: Vec<_> = document.projects.iter().map(|project| project.id).collect();
        if before == Some(id) {
            return document
                .projects
                .into_iter()
                .find(|project| project.id == id)
                .ok_or(StoreError::Domain(ProjectError::Unavailable));
        }
        let old_index = document
            .projects
            .iter()
            .position(|project| project.id == id)
            .ok_or(StoreError::Domain(ProjectError::Unavailable))?;
        let project = document.projects.remove(old_index);
        let new_index = match before {
            Some(before_id) => document
                .projects
                .iter()
                .position(|candidate| candidate.id == before_id)
                .ok_or(StoreError::Domain(ProjectError::Unavailable))?,
            None => document.projects.len(),
        };
        document.projects.insert(new_index, project);
        let next_order: Vec<_> = document.projects.iter().map(|project| project.id).collect();
        if next_order == original_order {
            return Ok(document.projects[new_index].clone());
        }
        assign_position_at(&mut document.projects, new_index)?;
        let revision = next_revision(document.revision)?;
        document.revision = revision;
        document.projects[new_index].revision = revision;
        let moved = document.projects[new_index].clone();
        self.write_document_locked(&document)?;
        Ok(moved)
    }

    fn ensure_root(&self) -> Result<(), StoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(StoreError::UnsafeEntry {
                    name: "agent-workspace",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.root).map_err(|error| io_error("create root", error))?;
            }
            Err(error) => return Err(io_error("inspect root", error)),
        }
        #[cfg(unix)]
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error("secure root", error))?;
        Ok(())
    }

    fn initialize_locked(&self, instance_id: &str) -> Result<(), StoreError> {
        let format_path = self.root.join(FORMAT_FILE);
        if !format_path.exists() {
            let format = FormatDocument {
                format_version: CURRENT_FORMAT_VERSION,
                minimum_reader: MINIMUM_READER_VERSION,
                instance_id: instance_id.to_string(),
            };
            let bytes = serialize(&format)?;
            self.writer
                .write(&format_path, &bytes)
                .map_err(|error| io_error("write format", error))?;
        }
        self.validate_format_locked()?;

        let projects_path = self.root.join(PROJECTS_FILE);
        if !projects_path.exists() {
            let document = ProjectsDocument {
                revision: Revision::ZERO,
                projects: Vec::new(),
            };
            self.write_document_locked(&document)?;
        }
        Ok(())
    }

    fn validate_format_locked(&self) -> Result<FormatDocument, StoreError> {
        let bytes =
            read_regular_bounded(&self.root.join(FORMAT_FILE), FORMAT_FILE, MAX_FORMAT_BYTES)?;
        let format: FormatDocument = serde_json::from_slice(&bytes)
            .map_err(|_| StoreError::Corrupt { name: FORMAT_FILE })?;
        if format.format_version > CURRENT_FORMAT_VERSION
            || format.minimum_reader > CURRENT_FORMAT_VERSION
        {
            return Err(StoreError::StoreNewer {
                found: format.format_version.max(format.minimum_reader),
                supported: CURRENT_FORMAT_VERSION,
            });
        }
        if format.format_version != CURRENT_FORMAT_VERSION
            || format.minimum_reader != MINIMUM_READER_VERSION
            || uuid::Uuid::parse_str(&format.instance_id).is_err()
        {
            return Err(StoreError::Corrupt { name: FORMAT_FILE });
        }
        Ok(format)
    }

    fn load_locked(&self) -> Result<ProjectSnapshot, StoreError> {
        match self.read_document(PROJECTS_FILE) {
            Ok(document) => Ok(snapshot(
                document,
                StoreHealth::Healthy,
                false,
                Durability::Full,
            )),
            Err(StoreError::Corrupt { .. }) => {
                let backup = self.read_document(PROJECTS_BACKUP_FILE)?;
                Ok(snapshot(
                    backup,
                    StoreHealth::RecoveredLastGood,
                    true,
                    Durability::Full,
                ))
            }
            Err(error) => Err(error),
        }
    }

    fn mutable_document_locked(&self) -> Result<ProjectsDocument, StoreError> {
        match self.read_document(PROJECTS_FILE) {
            Ok(document) => Ok(document),
            Err(StoreError::Corrupt { .. }) => Err(StoreError::Corrupt {
                name: PROJECTS_FILE,
            }),
            Err(error) => Err(error),
        }
    }

    fn read_document(&self, name: &'static str) -> Result<ProjectsDocument, StoreError> {
        let bytes = read_regular_bounded(&self.root.join(name), name, MAX_PROJECTS_BYTES)?;
        let mut document: ProjectsDocument =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt { name })?;
        validate_document(&document).map_err(|_| StoreError::Corrupt { name })?;
        sort_projects(&mut document.projects);
        Ok(document)
    }

    fn write_document_locked(&self, document: &ProjectsDocument) -> Result<Durability, StoreError> {
        validate_document(document).map_err(StoreError::Domain)?;
        let bytes = serialize(document)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PROJECTS_BYTES {
            return Err(StoreError::TooLarge {
                name: PROJECTS_FILE,
                limit: MAX_PROJECTS_BYTES,
            });
        }
        let durability = self
            .writer
            .write(&self.root.join(PROJECTS_FILE), &bytes)
            .map_err(|error| io_error("commit projects", error))?;
        let persisted = self.read_document(PROJECTS_FILE)?;
        if persisted.revision != document.revision || persisted.projects != document.projects {
            return Err(StoreError::Corrupt {
                name: PROJECTS_FILE,
            });
        }
        let _ = self
            .writer
            .write(&self.root.join(PROJECTS_BACKUP_FILE), &bytes);
        Ok(durability)
    }

    fn acquire_lock(&self) -> Result<MetadataLock, StoreError> {
        let path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(path)
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

impl ProjectService for ProjectRepository {
    fn list(&self) -> Result<Vec<ProjectSummary>, ProjectError> {
        self.load()
            .map(|snapshot| snapshot.projects)
            .map_err(store_as_domain)
    }

    fn add(&self, request: AddProject) -> Result<Project, ProjectError> {
        self.add_project(request).map_err(store_as_domain)
    }

    fn remove(&self, id: ProjectId, expected: Revision) -> Result<(), ProjectError> {
        self.remove_project(id, expected)
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

fn read_regular_bounded(
    path: &Path,
    name: &'static str,
    limit: u64,
) -> Result<Vec<u8>, StoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("inspect metadata", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::UnsafeEntry { name });
    }
    if metadata.len() > limit {
        return Err(StoreError::TooLarge { name, limit });
    }
    let capacity = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|error| io_error("read metadata", error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(StoreError::TooLarge { name, limit });
    }
    Ok(bytes)
}

fn serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec_pretty(value).map_err(|_| StoreError::Corrupt {
        name: "serialization",
    })
}

fn validate_document(document: &ProjectsDocument) -> Result<(), ProjectError> {
    if document.projects.len() > MAX_PROJECTS {
        return Err(ProjectError::ResourceLimit {
            limit: MAX_PROJECTS,
        });
    }
    let mut ids = HashSet::with_capacity(document.projects.len());
    let mut identities = HashSet::with_capacity(document.projects.len());
    for project in &document.projects {
        if project.revision > document.revision {
            return Err(ProjectError::Store {
                code: "future-project-revision",
            });
        }
        if !ids.insert(project.id) || !identities.insert(project.canonical_root.identity()) {
            return Err(ProjectError::Store {
                code: "duplicate-project",
            });
        }
    }
    Ok(())
}

fn snapshot(
    document: ProjectsDocument,
    health: StoreHealth,
    read_only: bool,
    durability: Durability,
) -> ProjectSnapshot {
    ProjectSnapshot {
        revision: document.revision,
        projects: document
            .projects
            .into_iter()
            .map(ProjectSummary::from)
            .collect(),
        health,
        read_only,
        durability,
    }
}

fn sort_projects(projects: &mut [Project]) {
    projects.sort_by_key(|project| (project.position, project.id));
}

fn next_revision(revision: Revision) -> Result<Revision, StoreError> {
    revision
        .next()
        .ok_or(StoreError::Domain(ProjectError::RevisionOverflow))
}

fn require_revision(expected: Revision, actual: Revision) -> Result<(), StoreError> {
    if expected != actual {
        return Err(StoreError::Domain(ProjectError::StaleRevision {
            expected,
            actual,
        }));
    }
    Ok(())
}

fn next_tail_position(projects: &mut [Project]) -> Result<PositionKey, StoreError> {
    sort_projects(projects);
    match projects.last() {
        None => Ok(PositionKey::FIRST),
        Some(project) => project.position.after().map_err(|_| {
            StoreError::Domain(ProjectError::Store {
                code: "position-overflow",
            })
        }),
    }
}

fn assign_position_at(projects: &mut [Project], index: usize) -> Result<(), StoreError> {
    let candidate = match (index.checked_sub(1), projects.get(index + 1)) {
        (None, Some(right)) if right.position.get() > 1 => {
            Some(PositionKey::new(right.position.get() / 2))
        }
        (Some(left_index), Some(right)) => {
            PositionKey::between(projects[left_index].position, right.position).ok()
        }
        (Some(left_index), None) => projects[left_index].position.after().ok(),
        (None, None) => Some(PositionKey::FIRST),
        _ => None,
    };
    if let Some(position) = candidate {
        projects[index].position = position;
        return Ok(());
    }
    for (project_index, project) in projects.iter_mut().enumerate() {
        project.position = PositionKey::rebalanced(project_index).map_err(|_| {
            StoreError::Domain(ProjectError::Store {
                code: "position-overflow",
            })
        })?;
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> StoreError {
    StoreError::Io {
        operation,
        kind: error.kind(),
    }
}

fn store_as_domain(error: StoreError) -> ProjectError {
    match error {
        StoreError::Domain(error) => error,
        StoreError::StoreNewer { .. } => ProjectError::Store {
            code: "newer-format",
        },
        StoreError::Corrupt { .. } => ProjectError::Store { code: "corrupt" },
        StoreError::Io { .. } => ProjectError::Store { code: "io" },
        StoreError::UnsafeEntry { .. } => ProjectError::Store {
            code: "unsafe-entry",
        },
        StoreError::TooLarge { .. } => ProjectError::Store { code: "too-large" },
        StoreError::InvalidInstanceId => ProjectError::Store {
            code: "invalid-instance",
        },
        StoreError::PresetDomain(_) => ProjectError::Store {
            code: "preset-domain",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use uuid::Uuid;

    const INSTANCE_ID: &str = "00000000-0000-0000-0000-000000000001";

    fn id(value: u128) -> ProjectId {
        ProjectId::from_uuid(Uuid::from_u128(value))
    }

    fn repository(root: &Path) -> ProjectRepository {
        ProjectRepository::open_with(root, INSTANCE_ID.to_string(), Arc::new(SystemAtomicWriter))
            .unwrap()
    }

    fn add_request(id: ProjectId, root: &Path, expected: Revision) -> AddProject {
        AddProject {
            id,
            root: root.to_path_buf(),
            display_name: None,
            expected,
        }
    }

    #[test]
    fn projects_persist_restart_and_removal_never_touches_folder_or_legacy_state() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("config/agent-workspace");
        let project_root = fixture.path().join("user-project");
        fs::create_dir(&project_root).unwrap();
        let sentinel = project_root.join("KEEP.txt");
        fs::write(&sentinel, b"do-not-delete").unwrap();
        let legacy = fixture.path().join("config/state.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, b"legacy-sentinel").unwrap();

        let repo = repository(&store_root);
        let added = repo
            .add_project(add_request(id(10), &project_root, Revision::ZERO))
            .unwrap();
        drop(repo);

        let reopened = repository(&store_root);
        let snapshot = reopened.load().unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].project.id, added.id);
        reopened
            .remove_project(added.id, snapshot.revision)
            .unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"do-not-delete");
        assert_eq!(fs::read(&legacy).unwrap(), b"legacy-sentinel");
        assert!(reopened.load().unwrap().projects.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_duplicate_returns_existing_id_without_alias() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("real");
        fs::create_dir(&project_root).unwrap();
        let alias = fixture.path().join("alias");
        symlink(&project_root, &alias).unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        let error = repo
            .add_project(add_request(id(2), &alias, Revision::new(1)))
            .unwrap_err();
        assert_eq!(
            error,
            StoreError::Domain(ProjectError::AlreadyPresent { id: id(1) })
        );
        assert_eq!(repo.load().unwrap().projects.len(), 1);
    }

    #[test]
    fn stale_concurrent_revision_commits_exactly_once() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let first_root = fixture.path().join("first");
        let second_root = fixture.path().join("second");
        fs::create_dir(&first_root).unwrap();
        fs::create_dir(&second_root).unwrap();
        let repo = repository(&store_root);
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = [(id(1), first_root), (id(2), second_root)]
            .into_iter()
            .map(|(project_id, root)| {
                let repo = repo.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    repo.add_project(add_request(project_id, &root, Revision::ZERO))
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Domain(ProjectError::StaleRevision { .. }))
                ))
                .count(),
            1
        );
        assert_eq!(repo.load().unwrap().projects.len(), 1);
    }

    #[test]
    fn corrupt_primary_loads_last_good_read_only_and_is_not_overwritten() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        let projects_path = repo.root().join(PROJECTS_FILE);
        fs::write(&projects_path, b"{broken").unwrap();
        let corrupt_bytes = fs::read(&projects_path).unwrap();

        let recovered = repo.load().unwrap();
        assert_eq!(recovered.health, StoreHealth::RecoveredLastGood);
        assert!(recovered.read_only);
        assert_eq!(recovered.projects.len(), 1);
        assert!(matches!(
            repo.remove_project(id(1), recovered.revision),
            Err(StoreError::Corrupt { .. })
        ));
        assert_eq!(fs::read(projects_path).unwrap(), corrupt_bytes);
    }

    #[test]
    fn newer_format_is_read_only_and_never_rewritten() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let format_path = repo.root().join(FORMAT_FILE);
        let future = br#"{"format_version":99,"minimum_reader":99,"instance_id":"00000000-0000-0000-0000-000000000001"}"#;
        fs::write(&format_path, future).unwrap();
        assert_eq!(
            repo.load().unwrap_err(),
            StoreError::StoreNewer {
                found: 99,
                supported: CURRENT_FORMAT_VERSION
            }
        );
        assert_eq!(fs::read(format_path).unwrap(), future);
    }

    #[derive(Debug)]
    struct DiskFullWriter;

    impl AtomicWriter for DiskFullWriter {
        fn write(&self, _target: &Path, _bytes: &[u8]) -> io::Result<Durability> {
            Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "fixture disk full",
            ))
        }
    }

    #[test]
    fn disk_full_before_rename_preserves_prior_bytes() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let normal = repository(&store_root);
        let prior = fs::read(normal.root().join(PROJECTS_FILE)).unwrap();
        let failing = ProjectRepository::open_with(
            &store_root,
            INSTANCE_ID.to_string(),
            Arc::new(DiskFullWriter),
        )
        .unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        assert!(matches!(
            failing.add_project(add_request(id(1), &project_root, Revision::ZERO)),
            Err(StoreError::Io {
                kind: io::ErrorKind::StorageFull,
                ..
            })
        ));
        assert_eq!(fs::read(normal.root().join(PROJECTS_FILE)).unwrap(), prior);
    }

    #[test]
    fn move_rebalances_adjacent_positions_deterministically() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        for value in 1..=3 {
            let root = fixture.path().join(format!("project-{value}"));
            fs::create_dir(&root).unwrap();
            repo.add_project(add_request(
                id(value),
                &root,
                Revision::new(value as u64 - 1),
            ))
            .unwrap();
        }
        repo.move_project_before(id(3), Some(id(1)), Revision::new(3))
            .unwrap();
        let ids: Vec<_> = repo
            .load()
            .unwrap()
            .projects
            .into_iter()
            .map(|summary| summary.project.id)
            .collect();
        assert_eq!(ids, [id(3), id(1), id(2)]);

        let revision = repo.load().unwrap().revision;
        repo.move_project_before(id(1), Some(id(2)), revision)
            .unwrap();
        assert_eq!(repo.load().unwrap().revision, revision);
        repo.move_project_before(id(1), Some(id(1)), revision)
            .unwrap();
        assert_eq!(repo.load().unwrap().revision, revision);
    }

    #[cfg(unix)]
    #[test]
    fn store_directory_and_metadata_are_user_only() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let root_mode = fs::metadata(repo.root()).unwrap().permissions().mode() & 0o777;
        let projects_mode = fs::metadata(repo.root().join(PROJECTS_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(root_mode, 0o700);
        assert_eq!(projects_mode, 0o600);
    }

    #[test]
    fn oversized_metadata_is_rejected_before_deserialization() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let oversized = vec![b'x'; usize::try_from(MAX_PROJECTS_BYTES + 1).unwrap()];
        fs::write(repo.root().join(PROJECTS_FILE), oversized).unwrap();
        assert!(matches!(repo.load(), Err(StoreError::TooLarge { .. })));
    }

    #[test]
    fn duplicate_identity_and_project_count_limit_fail_validation() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        let mut duplicate = project.clone();
        duplicate.id = id(2);
        let duplicate_document = ProjectsDocument {
            revision: Revision::new(1),
            projects: vec![project.clone(), duplicate],
        };
        assert_eq!(
            validate_document(&duplicate_document),
            Err(ProjectError::Store {
                code: "duplicate-project"
            })
        );

        let over_limit = ProjectsDocument {
            revision: Revision::new(1),
            projects: vec![project; MAX_PROJECTS + 1],
        };
        assert_eq!(
            validate_document(&over_limit),
            Err(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS
            })
        );
    }
}
