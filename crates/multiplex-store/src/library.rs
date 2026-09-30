use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use multiplex_domain::{
    CanonicalPath, Group, GroupDestination, GroupError, GroupId, GroupInverseCommand,
    GroupMutation, GroupName, LocalizedUserText, MAX_GROUPS, MAX_WORKTREE_REGISTRATIONS,
    ManagedWorktreeId, PathError, PositionKey, Revision, WorktreeError, WorktreeIntent,
    WorktreeIntentState, WorktreeRegistration, validate_group_set,
};
use serde::{Deserialize, Serialize};

use crate::{AtomicWriter, Durability, SystemAtomicWriter, file_lock};

pub const CURRENT_FORMAT_VERSION: u16 = 1;
const MINIMUM_READER_VERSION: u16 = 1;
const MAX_FORMAT_BYTES: u64 = 64 * 1024;
const MAX_LIBRARY_BYTES: u64 = 4 * 1024 * 1024;
const FORMAT_FILE: &str = "format.json";
pub(crate) const LIBRARY_FILE: &str = "library.json";
const LIBRARY_BACKUP_FILE: &str = "library.last-good.json";
const LOCK_FILE: &str = "metadata.lock";
const INTERACTIVE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreHealth {
    Healthy,
    RecoveredLastGood,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibrarySnapshot {
    pub revision: Revision,
    pub groups: Vec<Group>,
    pub worktree_intents: Vec<WorktreeIntent>,
    pub worktrees: Vec<WorktreeRegistration>,
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
    /// The caller's revision is not the one on disk, so it read a version that has since moved.
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    RevisionOverflow,
    Domain(PathError),
    GroupDomain(GroupError),
    PresetDomain(multiplex_domain::PresetError),
    SessionDomain(multiplex_domain::SessionStateError),
    WorktreeDomain(WorktreeError),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(formatter, "library store {operation} failed ({kind:?})")
            }
            Self::UnsafeEntry { name } => write!(
                formatter,
                "library store entry {name} is not a regular file"
            ),
            Self::TooLarge { name, limit } => write!(
                formatter,
                "library store entry {name} exceeds {limit} bytes"
            ),
            Self::Corrupt { name } => write!(formatter, "library store entry {name} is corrupt"),
            Self::StoreNewer { found, supported } => write!(
                formatter,
                "library store format {found} is newer than supported format {supported}"
            ),
            Self::InvalidInstanceId => formatter.write_str("library store instance ID is invalid"),
            Self::StaleRevision { .. } => {
                formatter.write_str("the library changed; reload required")
            }
            Self::RevisionOverflow => formatter.write_str("library revision exhausted"),
            Self::Domain(error) => error.fmt(formatter),
            Self::GroupDomain(error) => error.fmt(formatter),
            Self::PresetDomain(error) => error.fmt(formatter),
            Self::SessionDomain(error) => error.fmt(formatter),
            Self::WorktreeDomain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<PathError> for StoreError {
    fn from(error: PathError) -> Self {
        Self::Domain(error)
    }
}

impl From<GroupError> for StoreError {
    fn from(error: GroupError) -> Self {
        Self::GroupDomain(error)
    }
}

impl From<multiplex_domain::PresetError> for StoreError {
    fn from(error: multiplex_domain::PresetError) -> Self {
        Self::PresetDomain(error)
    }
}

impl From<multiplex_domain::SessionStateError> for StoreError {
    fn from(error: multiplex_domain::SessionStateError) -> Self {
        Self::SessionDomain(error)
    }
}

impl From<WorktreeError> for StoreError {
    fn from(error: WorktreeError) -> Self {
        Self::WorktreeDomain(error)
    }
}

#[derive(Clone)]
pub struct LibraryRepository {
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
struct LibraryDocument {
    revision: Revision,
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    worktree_intents: Vec<WorktreeIntent>,
    #[serde(default)]
    worktrees: Vec<WorktreeRegistration>,
}

pub(crate) struct LibraryHealthSource {
    pub groups: Vec<Group>,
}

pub(crate) fn read_library_health_source(
    root: &Path,
) -> Result<(Vec<u8>, LibraryHealthSource), StoreError> {
    let bytes = read_regular_bounded(&root.join(LIBRARY_FILE), LIBRARY_FILE, MAX_LIBRARY_BYTES)?;
    let mut document: LibraryDocument =
        serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt { name: LIBRARY_FILE })?;
    validate_document(&document).map_err(|_| StoreError::Corrupt { name: LIBRARY_FILE })?;
    sort_groups(&mut document.groups);
    Ok((
        bytes,
        LibraryHealthSource {
            groups: document.groups,
        },
    ))
}

pub(crate) fn validate_library_metadata_bytes(bytes: &[u8]) -> Result<Revision, StoreError> {
    let document: LibraryDocument =
        serde_json::from_slice(bytes).map_err(|_| StoreError::Corrupt { name: LIBRARY_FILE })?;
    validate_document(&document).map_err(|_| StoreError::Corrupt { name: LIBRARY_FILE })?;
    Ok(document.revision)
}

impl LibraryRepository {
    pub(crate) fn load_existing_read_only(
        root: impl Into<PathBuf>,
    ) -> Result<LibrarySnapshot, StoreError> {
        let repository = Self {
            root: root.into(),
            writer: Arc::new(SystemAtomicWriter),
        };
        repository.validate_format_locked()?;
        // A store nobody has opened for writing since Projects were removed still has its groups
        // in `projects.json`; read them from there rather than report the library missing.
        if !repository.root.join(LIBRARY_FILE).exists()
            && let Some(legacy) = crate::legacy_projects::read_legacy(&repository.root)
        {
            let mut document: LibraryDocument =
                serde_json::from_value(crate::legacy_projects::library_from_legacy(&legacy))
                    .map_err(|_| StoreError::Corrupt {
                        name: crate::legacy_projects::LEGACY_PROJECTS_FILE,
                    })?;
            validate_document(&document).map_err(|_| StoreError::Corrupt {
                name: crate::legacy_projects::LEGACY_PROJECTS_FILE,
            })?;
            sort_groups(&mut document.groups);
            return Ok(snapshot(
                document,
                StoreHealth::Healthy,
                true,
                Durability::Full,
            ));
        }
        repository.load_locked()
    }

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

    pub fn load(&self) -> Result<LibrarySnapshot, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        self.load_locked()
    }

    pub fn begin_worktree_intent(
        &self,
        mut intent: WorktreeIntent,
        expected: Revision,
    ) -> Result<WorktreeIntent, StoreError> {
        intent.plan.validate()?;
        let _ = LocalizedUserText::new(&intent.child_display_name)?;
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        if document.worktrees.len() + document.worktree_intents.len() >= MAX_WORKTREE_REGISTRATIONS
        {
            return Err(WorktreeError::ResourceLimit {
                limit: MAX_WORKTREE_REGISTRATIONS,
            }
            .into());
        }
        if document
            .worktree_intents
            .iter()
            .any(|candidate| worktree_plan_conflicts(&candidate.plan, &intent.plan))
            || document.worktrees.iter().any(|candidate| {
                candidate.id == intent.plan.id
                    || candidate.branch == intent.plan.generated_branch
                    || candidate.managed_path.as_path() == intent.plan.managed_path.as_path()
            })
        {
            return Err(WorktreeError::RegistrationConflict.into());
        }
        let revision = next_revision(document.revision)?;
        intent.revision = revision;
        intent.state = WorktreeIntentState::Planned;
        document.worktree_intents.push(intent.clone());
        document.revision = revision;
        self.write_document_locked(&document)?;
        Ok(intent)
    }

    pub fn mark_worktree_intent_needs_inspection(
        &self,
        id: ManagedWorktreeId,
        expected: Revision,
    ) -> Result<WorktreeIntent, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = document
            .worktree_intents
            .iter()
            .position(|intent| intent.plan.id == id)
            .ok_or(StoreError::WorktreeDomain(
                WorktreeError::RegistrationConflict,
            ))?;
        let revision = next_revision(document.revision)?;
        document.worktree_intents[index].state = WorktreeIntentState::NeedsInspection;
        document.worktree_intents[index].revision = revision;
        document.revision = revision;
        let intent = document.worktree_intents[index].clone();
        self.write_document_locked(&document)?;
        Ok(intent)
    }

    pub fn cancel_worktree_intent(
        &self,
        id: ManagedWorktreeId,
        expected: Revision,
    ) -> Result<(), StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = document
            .worktree_intents
            .iter()
            .position(|intent| intent.plan.id == id)
            .ok_or(StoreError::WorktreeDomain(
                WorktreeError::RegistrationConflict,
            ))?;
        document.worktree_intents.remove(index);
        document.revision = next_revision(document.revision)?;
        self.write_document_locked(&document)?;
        Ok(())
    }

    pub fn register_worktree_child(
        &self,
        id: ManagedWorktreeId,
        expected: Revision,
    ) -> Result<WorktreeRegistration, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let intent_index = document
            .worktree_intents
            .iter()
            .position(|intent| intent.plan.id == id)
            .ok_or(StoreError::WorktreeDomain(
                WorktreeError::RegistrationConflict,
            ))?;
        let intent = document.worktree_intents[intent_index].clone();
        intent.plan.validate()?;
        let canonical_path = CanonicalPath::resolve(intent.plan.managed_path.as_path())?;
        if canonical_path.as_path() != intent.plan.managed_path.as_path()
            || canonical_path.as_path() == intent.plan.managed_root.as_path()
            || !canonical_path
                .as_path()
                .starts_with(intent.plan.managed_root.as_path())
        {
            return Err(WorktreeError::SymlinkSwap.into());
        }
        if document.worktrees.iter().any(|registration| {
            registration.id == id
                || registration.managed_path.identity() == canonical_path.identity()
                || registration.branch == intent.plan.generated_branch
        }) {
            return Err(WorktreeError::RegistrationConflict.into());
        }
        let revision = next_revision(document.revision)?;
        let registration = WorktreeRegistration {
            id,
            repository_root: intent.plan.repository_root,
            managed_root: intent.plan.managed_root,
            managed_path: canonical_path,
            base: intent.plan.selected_base,
            branch: intent.plan.generated_branch,
            revision,
        };
        registration.validate()?;
        document.worktree_intents.remove(intent_index);
        document.worktrees.push(registration.clone());
        document.revision = revision;
        self.write_document_locked(&document)?;
        Ok(registration)
    }

    pub fn create_group(
        &self,
        id: GroupId,
        name: &str,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let name = GroupName::new(name)?;
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let group_count = document.groups.len();
        if group_count >= MAX_GROUPS {
            return Err(GroupError::ResourceLimit { limit: MAX_GROUPS }.into());
        }
        require_unique_group_name(&document.groups, None, &name)?;
        if document.groups.iter().any(|group| group.id == id) {
            return Err(GroupError::Store {
                code: "duplicate-group-id",
            }
            .into());
        }
        let revision = next_group_revision(document.revision)?;
        let position = next_group_tail_position(&mut document.groups)?;
        let group = Group {
            id,
            name,
            position,
            collapsed: false,
            revision,
        };
        document.groups.push(group.clone());
        document.revision = revision;
        sort_groups(&mut document.groups);
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: group,
            inverse: GroupInverseCommand::RemoveCreated { group_id: id },
        })
    }

    pub fn rename_group(
        &self,
        id: GroupId,
        name: &str,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let name = GroupName::new(name)?;
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = group_index(&document.groups, id)?;
        require_unique_group_name(&document.groups, Some(id), &name)?;
        if document.groups[index].name == name {
            return Ok(GroupMutation {
                value: document.groups[index].clone(),
                inverse: GroupInverseCommand::Rename { group_id: id, name },
            });
        }
        let previous = document.groups[index].name.clone();
        let revision = next_group_revision(document.revision)?;
        document.groups[index].name = name;
        document.groups[index].revision = revision;
        document.revision = revision;
        let group = document.groups[index].clone();
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: group,
            inverse: GroupInverseCommand::Rename {
                group_id: id,
                name: previous,
            },
        })
    }

    pub fn set_group_collapsed(
        &self,
        id: GroupId,
        collapsed: bool,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = group_index(&document.groups, id)?;
        let previous = document.groups[index].collapsed;
        if previous == collapsed {
            return Ok(GroupMutation {
                value: document.groups[index].clone(),
                inverse: GroupInverseCommand::SetCollapsed {
                    group_id: id,
                    collapsed: previous,
                },
            });
        }
        let revision = next_group_revision(document.revision)?;
        document.groups[index].collapsed = collapsed;
        document.groups[index].revision = revision;
        document.revision = revision;
        let group = document.groups[index].clone();
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: group,
            inverse: GroupInverseCommand::SetCollapsed {
                group_id: id,
                collapsed: previous,
            },
        })
    }

    pub fn move_group_before(
        &self,
        id: GroupId,
        before: Option<GroupId>,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        sort_groups(&mut document.groups);
        let index = group_index(&document.groups, id)?;
        if let Some(before_id) = before
            && !document.groups.iter().any(|group| group.id == before_id)
        {
            return Err(GroupError::DestinationNotFound.into());
        }
        let mut order = group_ids(&document.groups);
        let old_index = order
            .iter()
            .position(|candidate| *candidate == id)
            .ok_or(StoreError::GroupDomain(GroupError::NotFound))?;
        let previous_before = order.get(old_index + 1).copied();
        if before == Some(id) {
            return Ok(GroupMutation {
                value: document.groups[index].clone(),
                inverse: GroupInverseCommand::MoveBefore {
                    group_id: id,
                    before: previous_before,
                },
            });
        }
        order.remove(old_index);
        let new_index = match before {
            Some(before_id) => order
                .iter()
                .position(|candidate| *candidate == before_id)
                .ok_or(StoreError::GroupDomain(GroupError::DestinationNotFound))?,
            None => order.len(),
        };
        order.insert(new_index, id);
        let current_order = group_ids(&document.groups);
        if order == current_order {
            return Ok(GroupMutation {
                value: document.groups[index].clone(),
                inverse: GroupInverseCommand::MoveBefore {
                    group_id: id,
                    before: previous_before,
                },
            });
        }
        assign_group_position(&mut document.groups, &order, new_index)?;
        let revision = next_group_revision(document.revision)?;
        let index = group_index(&document.groups, id)?;
        document.groups[index].revision = revision;
        document.revision = revision;
        let moved = document.groups[index].clone();
        sort_groups(&mut document.groups);
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: moved,
            inverse: GroupInverseCommand::MoveBefore {
                group_id: id,
                before: previous_before,
            },
        })
    }

    pub fn remove_group(
        &self,
        id: GroupId,
        destination: Option<GroupDestination>,
        has_sessions: bool,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = group_index(&document.groups, id)?;
        let destination = match (has_sessions, destination) {
            (true, None) => return Err(GroupError::NonEmptyDestinationRequired.into()),
            (_, Some(destination)) => destination,
            (false, None) => GroupDestination::Ungrouped,
        };
        if destination.group_id() == Some(id) {
            return Err(GroupError::DestinationIsSource.into());
        }
        if let Some(destination_id) = destination.group_id()
            && !document
                .groups
                .iter()
                .any(|group| group.id == destination_id)
        {
            return Err(GroupError::DestinationNotFound.into());
        }
        let removed = document.groups.remove(index);
        document.revision = next_group_revision(document.revision)?;
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: removed.clone(),
            inverse: GroupInverseCommand::RestoreRemoved {
                group: removed,
                moved_sessions_to: destination,
            },
        })
    }

    pub fn restore_group(
        &self,
        mut group: Group,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        if document
            .groups
            .iter()
            .any(|candidate| candidate.id == group.id)
        {
            return Err(GroupError::Store {
                code: "duplicate-group-id",
            }
            .into());
        }
        let group_count = document.groups.len();
        if group_count >= MAX_GROUPS {
            return Err(GroupError::ResourceLimit { limit: MAX_GROUPS }.into());
        }
        require_unique_group_name(&document.groups, None, &group.name)?;
        let revision = next_group_revision(document.revision)?;
        group.revision = revision;
        document.groups.push(group.clone());
        document.revision = revision;
        sort_groups(&mut document.groups);
        self.write_document_locked(&document)?;
        Ok(GroupMutation {
            value: group.clone(),
            inverse: GroupInverseCommand::RemoveCreated { group_id: group.id },
        })
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
        crate::legacy_projects::migrate(&self.root, self.writer.as_ref())?;

        let library_path = self.root.join(LIBRARY_FILE);
        if !library_path.exists() {
            let document = LibraryDocument {
                revision: Revision::ZERO,
                groups: Vec::new(),
                worktree_intents: Vec::new(),
                worktrees: Vec::new(),
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

    fn load_locked(&self) -> Result<LibrarySnapshot, StoreError> {
        match self.read_document(LIBRARY_FILE) {
            Ok(document) => Ok(snapshot(
                document,
                StoreHealth::Healthy,
                false,
                Durability::Full,
            )),
            Err(StoreError::Corrupt { .. }) => {
                let backup = self.read_document(LIBRARY_BACKUP_FILE)?;
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

    fn mutable_document_locked(&self) -> Result<LibraryDocument, StoreError> {
        match self.read_document(LIBRARY_FILE) {
            Ok(document) => Ok(document),
            Err(StoreError::Corrupt { .. }) => Err(StoreError::Corrupt { name: LIBRARY_FILE }),
            Err(error) => Err(error),
        }
    }

    fn read_document(&self, name: &'static str) -> Result<LibraryDocument, StoreError> {
        let bytes = read_regular_bounded(&self.root.join(name), name, MAX_LIBRARY_BYTES)?;
        let mut document: LibraryDocument =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt { name })?;
        validate_document(&document).map_err(|_| StoreError::Corrupt { name })?;
        sort_groups(&mut document.groups);
        Ok(document)
    }

    fn write_document_locked(&self, document: &LibraryDocument) -> Result<Durability, StoreError> {
        validate_document(document).map_err(|name| StoreError::Corrupt { name })?;
        let bytes = serialize(document)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_LIBRARY_BYTES {
            return Err(StoreError::TooLarge {
                name: LIBRARY_FILE,
                limit: MAX_LIBRARY_BYTES,
            });
        }
        let durability = self
            .writer
            .write(&self.root.join(LIBRARY_FILE), &bytes)
            .map_err(|error| io_error("commit library", error))?;
        let persisted = self.read_document(LIBRARY_FILE)?;
        if persisted.revision != document.revision
            || persisted.groups != document.groups
            || persisted.worktree_intents != document.worktree_intents
            || persisted.worktrees != document.worktrees
        {
            return Err(StoreError::Corrupt { name: LIBRARY_FILE });
        }
        let _ = self
            .writer
            .write(&self.root.join(LIBRARY_BACKUP_FILE), &bytes);
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
        file_lock::exclusive_with_timeout(&file, INTERACTIVE_LOCK_TIMEOUT, LOCK_RETRY_INTERVAL)
            .map_err(|error| io_error("lock library metadata", error))?;
        Ok(MetadataLock { file })
    }
}

struct MetadataLock {
    file: File,
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        file_lock::release(&self.file);
    }
}

pub(crate) fn read_regular_bounded(
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

fn validate_document(document: &LibraryDocument) -> Result<(), &'static str> {
    validate_group_set(&document.groups).map_err(|error| match error {
        GroupError::DuplicateName => "duplicate-group-name",
        GroupError::ResourceLimit { .. } => "group-limit",
        GroupError::Store { code } => code,
        _ => "invalid-group",
    })?;
    if document
        .groups
        .iter()
        .any(|group| group.revision > document.revision)
    {
        return Err("future-group-revision");
    }
    if document.worktree_intents.len() + document.worktrees.len() > MAX_WORKTREE_REGISTRATIONS {
        return Err("worktree-limit");
    }
    let mut worktree_ids = HashSet::new();
    let mut worktree_paths = HashSet::new();
    let mut worktree_branches = HashSet::new();
    for intent in &document.worktree_intents {
        intent
            .plan
            .validate()
            .map_err(|_| "invalid-worktree-intent")?;
        LocalizedUserText::new(&intent.child_display_name).map_err(|_| "invalid-worktree-label")?;
        if intent.revision > document.revision
            || !worktree_ids.insert(intent.plan.id)
            || !worktree_paths.insert(intent.plan.managed_path.as_path())
            || !worktree_branches.insert(&intent.plan.generated_branch)
        {
            return Err("duplicate-worktree-intent");
        }
    }
    for registration in &document.worktrees {
        registration
            .validate()
            .map_err(|_| "invalid-worktree-registration")?;
        if registration.revision > document.revision
            || !worktree_ids.insert(registration.id)
            || !worktree_paths.insert(registration.managed_path.as_path())
            || !worktree_branches.insert(&registration.branch)
        {
            return Err("duplicate-worktree-registration");
        }
    }
    Ok(())
}

fn snapshot(
    document: LibraryDocument,
    health: StoreHealth,
    read_only: bool,
    durability: Durability,
) -> LibrarySnapshot {
    LibrarySnapshot {
        revision: document.revision,
        groups: document.groups,
        worktree_intents: document.worktree_intents,
        worktrees: document.worktrees,
        health,
        read_only,
        durability,
    }
}

fn worktree_plan_conflicts(
    left: &multiplex_domain::WorktreePlan,
    right: &multiplex_domain::WorktreePlan,
) -> bool {
    left.id == right.id
        || left.generated_branch == right.generated_branch
        || left.managed_path.as_path() == right.managed_path.as_path()
}

fn sort_groups(groups: &mut [Group]) {
    groups.sort_by_key(|group| (group.position, group.id));
}

fn group_index(groups: &[Group], id: GroupId) -> Result<usize, StoreError> {
    groups
        .iter()
        .position(|group| group.id == id)
        .ok_or(StoreError::GroupDomain(GroupError::NotFound))
}

fn require_unique_group_name(
    groups: &[Group],
    except: Option<GroupId>,
    name: &GroupName,
) -> Result<(), StoreError> {
    let comparison_key = name.comparison_key();
    if groups
        .iter()
        .any(|group| Some(group.id) != except && group.name.comparison_key() == comparison_key)
    {
        Err(GroupError::DuplicateName.into())
    } else {
        Ok(())
    }
}

fn next_group_revision(revision: Revision) -> Result<Revision, StoreError> {
    revision
        .next()
        .ok_or(StoreError::GroupDomain(GroupError::RevisionOverflow))
}

fn group_ids(groups: &[Group]) -> Vec<GroupId> {
    let mut ordered = groups.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|group| (group.position, group.id));
    ordered.into_iter().map(|group| group.id).collect()
}

fn next_group_tail_position(groups: &mut [Group]) -> Result<PositionKey, StoreError> {
    let ids = group_ids(groups);
    match ids
        .last()
        .and_then(|id| groups.iter().find(|group| group.id == *id))
    {
        None => Ok(PositionKey::FIRST),
        Some(group) => group
            .position
            .after()
            .map_err(|_| StoreError::GroupDomain(GroupError::PositionOverflow)),
    }
}

fn assign_group_position(
    groups: &mut [Group],
    order: &[GroupId],
    index: usize,
) -> Result<(), StoreError> {
    let position_for = |id: GroupId| {
        groups
            .iter()
            .find(|group| group.id == id)
            .map(|group| group.position)
    };
    let candidate = match (index.checked_sub(1), order.get(index + 1).copied()) {
        (None, Some(right_id)) => position_for(right_id)
            .filter(|right| right.get() > 1)
            .map(|right| PositionKey::new(right.get() / 2)),
        (Some(left_index), Some(right_id)) => PositionKey::between(
            position_for(order[left_index]).ok_or(StoreError::GroupDomain(GroupError::NotFound))?,
            position_for(right_id).ok_or(StoreError::GroupDomain(GroupError::NotFound))?,
        )
        .ok(),
        (Some(left_index), None) => position_for(order[left_index])
            .ok_or(StoreError::GroupDomain(GroupError::NotFound))?
            .after()
            .ok(),
        (None, None) => Some(PositionKey::FIRST),
    };
    if let Some(position) = candidate {
        let target = groups
            .iter_mut()
            .find(|group| group.id == order[index])
            .ok_or(StoreError::GroupDomain(GroupError::NotFound))?;
        target.position = position;
        return Ok(());
    }
    for (group_index, id) in order.iter().enumerate() {
        let target = groups
            .iter_mut()
            .find(|group| group.id == *id)
            .ok_or(StoreError::GroupDomain(GroupError::NotFound))?;
        target.position = PositionKey::rebalanced(group_index)
            .map_err(|_| StoreError::GroupDomain(GroupError::PositionOverflow))?;
    }
    Ok(())
}

fn next_revision(revision: Revision) -> Result<Revision, StoreError> {
    revision.next().ok_or(StoreError::RevisionOverflow)
}

fn require_revision(expected: Revision, actual: Revision) -> Result<(), StoreError> {
    if expected != actual {
        return Err(StoreError::StaleRevision { expected, actual });
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> StoreError {
    StoreError::Io {
        operation,
        kind: error.kind(),
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

    fn group_id(value: u128) -> GroupId {
        GroupId::from_uuid(Uuid::from_u128(10_000 + value))
    }

    fn worktree_id(value: u128) -> ManagedWorktreeId {
        ManagedWorktreeId::from_uuid(Uuid::from_u128(20_000 + value))
    }

    fn repository(root: &Path) -> LibraryRepository {
        LibraryRepository::open_with(root, INSTANCE_ID.to_string(), Arc::new(SystemAtomicWriter))
            .unwrap()
    }

    fn worktree_intent(
        id: ManagedWorktreeId,
        repository_root: &Path,
        managed_root: &Path,
        managed_path: &Path,
    ) -> WorktreeIntent {
        let canonical_managed_root = CanonicalPath::resolve(managed_root).unwrap();
        let canonical_managed_path = canonical_managed_root.as_path().join(
            managed_path
                .file_name()
                .expect("fixture managed path has a basename"),
        );
        WorktreeIntent {
            plan: multiplex_domain::WorktreePlan::new(
                id,
                CanonicalPath::resolve(repository_root).unwrap(),
                canonical_managed_root,
                multiplex_domain::BaseCandidate {
                    ref_name: multiplex_domain::GitReference::new("main").unwrap(),
                    commit_oid: multiplex_domain::CommitOid::new(&"a".repeat(40)).unwrap(),
                    source: multiplex_domain::BaseSource::ConfiguredMainline,
                },
                multiplex_domain::GitReference::new("termirust/worktree/test").unwrap(),
                multiplex_domain::ManagedPath::new(canonical_managed_path).unwrap(),
            )
            .unwrap(),
            child_display_name: "Isolated test".to_string(),
            state: WorktreeIntentState::Planned,
            revision: Revision::ZERO,
        }
    }

    #[test]
    fn worktree_registration_is_atomic_persistent_and_leaves_the_folder_alone() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repository_root = fixture.path().join("repository");
        let managed_root = fixture.path().join("managed");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&managed_root).unwrap();
        let repo = repository(&store_root);
        let managed_path = managed_root.join("child");
        let intent = repo
            .begin_worktree_intent(
                worktree_intent(
                    worktree_id(1),
                    &repository_root,
                    &managed_root,
                    &managed_path,
                ),
                Revision::ZERO,
            )
            .unwrap();
        assert_eq!(intent.revision, Revision::new(1));
        fs::create_dir(&managed_path).unwrap();
        let sentinel = managed_path.join("KEEP.txt");
        fs::write(&sentinel, "keep").unwrap();
        let registration = repo
            .register_worktree_child(worktree_id(1), Revision::new(1))
            .unwrap();
        assert_eq!(
            registration.managed_path.as_path(),
            CanonicalPath::resolve(&managed_path).unwrap().as_path()
        );

        let reopened = repository(&store_root);
        let snapshot = reopened.load().unwrap();
        assert_eq!(snapshot.worktree_intents.len(), 0);
        assert_eq!(snapshot.worktrees, vec![registration]);
        assert!(sentinel.exists());
    }

    #[test]
    fn worktree_registration_crash_intent_survives_and_is_reconcilable() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repository_root = fixture.path().join("repository");
        let managed_root = fixture.path().join("managed");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&managed_root).unwrap();
        let repo = repository(&store_root);
        let managed_path = managed_root.join("crash-child");
        repo.begin_worktree_intent(
            worktree_intent(
                worktree_id(2),
                &repository_root,
                &managed_root,
                &managed_path,
            ),
            Revision::ZERO,
        )
        .unwrap();

        let reopened = repository(&store_root);
        let recovered = reopened.load().unwrap();
        assert_eq!(recovered.worktree_intents.len(), 1);
        let marked = reopened
            .mark_worktree_intent_needs_inspection(worktree_id(2), recovered.revision)
            .unwrap();
        assert_eq!(marked.state, WorktreeIntentState::NeedsInspection);
        let marked_snapshot = reopened.load().unwrap();
        assert_eq!(marked_snapshot.worktree_intents, vec![marked]);

        reopened
            .cancel_worktree_intent(worktree_id(2), marked_snapshot.revision)
            .unwrap();
        assert!(reopened.load().unwrap().worktree_intents.is_empty());
        assert!(!managed_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_registration_rejects_symlink_swap_without_mutating_store() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repository_root = fixture.path().join("repository");
        let managed_root = fixture.path().join("managed");
        let outside = fixture.path().join("outside");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&managed_root).unwrap();
        fs::create_dir(&outside).unwrap();
        let repo = repository(&store_root);
        let managed_path = managed_root.join("swapped");
        repo.begin_worktree_intent(
            worktree_intent(
                worktree_id(3),
                &repository_root,
                &managed_root,
                &managed_path,
            ),
            Revision::ZERO,
        )
        .unwrap();
        symlink(&outside, &managed_path).unwrap();
        assert!(matches!(
            repo.register_worktree_child(worktree_id(3), Revision::new(1)),
            Err(StoreError::WorktreeDomain(WorktreeError::SymlinkSwap))
        ));
        let snapshot = repo.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(1));
        assert_eq!(snapshot.worktree_intents.len(), 1);
        assert!(snapshot.worktrees.is_empty());
    }

    #[test]
    fn worktree_registration_write_failure_preserves_recoverable_intent_atomically() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repository_root = fixture.path().join("repository");
        let managed_root = fixture.path().join("managed");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&managed_root).unwrap();
        let normal = repository(&store_root);
        let managed_path = managed_root.join("created-before-store-failure");
        normal
            .begin_worktree_intent(
                worktree_intent(
                    worktree_id(4),
                    &repository_root,
                    &managed_root,
                    &managed_path,
                ),
                Revision::ZERO,
            )
            .unwrap();
        fs::create_dir(&managed_path).unwrap();
        let prior = fs::read(normal.root().join(LIBRARY_FILE)).unwrap();
        let failing = LibraryRepository::open_with(
            &store_root,
            INSTANCE_ID.to_string(),
            Arc::new(DiskFullWriter),
        )
        .unwrap();

        assert!(matches!(
            failing.register_worktree_child(worktree_id(4), Revision::new(1)),
            Err(StoreError::Io {
                kind: io::ErrorKind::StorageFull,
                ..
            })
        ));
        assert_eq!(fs::read(normal.root().join(LIBRARY_FILE)).unwrap(), prior);
        let snapshot = normal.load().unwrap();
        assert_eq!(snapshot.worktree_intents.len(), 1);
        assert!(snapshot.worktrees.is_empty());
        assert!(managed_path.is_dir());
    }

    #[test]
    fn stale_concurrent_revision_commits_exactly_once() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = [(group_id(1), "First"), (group_id(2), "Second")]
            .into_iter()
            .map(|(id, name)| {
                let repo = repo.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    repo.create_group(id, name, Revision::ZERO)
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::StaleRevision { .. })))
                .count(),
            1
        );
        assert_eq!(repo.load().unwrap().groups.len(), 1);
    }

    #[test]
    fn corrupt_primary_loads_last_good_read_only_and_is_not_overwritten() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.create_group(group_id(1), "Build", Revision::ZERO)
            .unwrap();
        let library_path = repo.root().join(LIBRARY_FILE);
        fs::write(&library_path, b"{broken").unwrap();
        let corrupt_bytes = fs::read(&library_path).unwrap();

        let recovered = repo.load().unwrap();
        assert_eq!(recovered.health, StoreHealth::RecoveredLastGood);
        assert!(recovered.read_only);
        assert_eq!(recovered.groups.len(), 1);
        assert!(matches!(
            repo.rename_group(group_id(1), "Changed", recovered.revision),
            Err(StoreError::Corrupt { .. })
        ));
        assert_eq!(fs::read(library_path).unwrap(), corrupt_bytes);
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
        let prior = fs::read(normal.root().join(LIBRARY_FILE)).unwrap();
        let failing = LibraryRepository::open_with(
            &store_root,
            INSTANCE_ID.to_string(),
            Arc::new(DiskFullWriter),
        )
        .unwrap();
        assert!(matches!(
            failing.create_group(group_id(1), "Build", Revision::ZERO),
            Err(StoreError::Io {
                kind: io::ErrorKind::StorageFull,
                ..
            })
        ));
        assert_eq!(fs::read(normal.root().join(LIBRARY_FILE)).unwrap(), prior);
    }

    #[cfg(unix)]
    #[test]
    fn store_directory_and_metadata_are_user_only() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let root_mode = fs::metadata(repo.root()).unwrap().permissions().mode() & 0o777;
        let library_mode = fs::metadata(repo.root().join(LIBRARY_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(root_mode, 0o700);
        assert_eq!(library_mode, 0o600);
    }

    #[test]
    fn oversized_metadata_is_rejected_before_deserialization() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        let oversized = vec![b'x'; usize::try_from(MAX_LIBRARY_BYTES + 1).unwrap()];
        fs::write(repo.root().join(LIBRARY_FILE), oversized).unwrap();
        assert!(matches!(repo.load(), Err(StoreError::TooLarge { .. })));
    }

    #[test]
    fn groups_crud_order_and_collapse_persist_across_restart() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repo = repository(&store_root);
        let first = repo
            .create_group(group_id(1), "Build", Revision::ZERO)
            .unwrap();
        assert_eq!(
            first.inverse,
            GroupInverseCommand::RemoveCreated {
                group_id: group_id(1)
            }
        );
        repo.create_group(group_id(2), "Review", Revision::new(1))
            .unwrap();
        repo.rename_group(group_id(1), "Implement", Revision::new(2))
            .unwrap();
        repo.set_group_collapsed(group_id(2), true, Revision::new(3))
            .unwrap();
        repo.move_group_before(group_id(2), Some(group_id(1)), Revision::new(4))
            .unwrap();
        drop(repo);

        let snapshot = repository(&store_root).load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(5));
        assert_eq!(
            snapshot
                .groups
                .iter()
                .map(|group| group.id)
                .collect::<Vec<_>>(),
            [group_id(2), group_id(1)]
        );
        assert!(snapshot.groups[0].collapsed);
        assert_eq!(snapshot.groups[1].name.as_str(), "Implement");
    }

    #[test]
    fn groups_reject_duplicate_names_and_stale_mutations_without_change() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.create_group(group_id(1), "Review", Revision::ZERO)
            .unwrap();
        assert_eq!(
            repo.create_group(group_id(2), "review", Revision::new(1)),
            Err(StoreError::GroupDomain(GroupError::DuplicateName))
        );
        assert!(matches!(
            repo.rename_group(group_id(1), "Changed", Revision::ZERO),
            Err(StoreError::StaleRevision { .. })
        ));
        let snapshot = repo.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(1));
        assert_eq!(snapshot.groups[0].name.as_str(), "Review");
    }

    #[test]
    fn groups_non_empty_removal_requires_valid_explicit_destination_and_restores() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.create_group(group_id(1), "Source", Revision::ZERO)
            .unwrap();
        repo.create_group(group_id(2), "Destination", Revision::new(1))
            .unwrap();
        assert_eq!(
            repo.remove_group(group_id(1), None, true, Revision::new(2)),
            Err(StoreError::GroupDomain(
                GroupError::NonEmptyDestinationRequired
            ))
        );
        let removed = repo
            .remove_group(
                group_id(1),
                Some(GroupDestination::Group(group_id(2))),
                true,
                Revision::new(2),
            )
            .unwrap();
        assert_eq!(removed.value.id, group_id(1));
        assert_eq!(repo.load().unwrap().groups.len(), 1);
        repo.restore_group(removed.value, Revision::new(3)).unwrap();
        assert_eq!(repo.load().unwrap().groups.len(), 2);
    }

    #[test]
    fn groups_missing_removal_destination_preserves_document() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        repo.create_group(group_id(1), "Source", Revision::ZERO)
            .unwrap();
        let before = fs::read(repo.root().join(LIBRARY_FILE)).unwrap();
        assert_eq!(
            repo.remove_group(
                group_id(1),
                Some(GroupDestination::Group(group_id(99))),
                true,
                Revision::new(1),
            ),
            Err(StoreError::GroupDomain(GroupError::DestinationNotFound))
        );
        assert_eq!(fs::read(repo.root().join(LIBRARY_FILE)).unwrap(), before);
    }

    /// A store as 0.0.5 left it: a Project holding a group, and a session naming the Project.
    fn write_legacy_store(root: &Path, project_folder: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        let folder = serde_json::to_value(CanonicalPath::resolve(project_folder).unwrap()).unwrap();
        let group = group_id(1).to_string();
        let project = "00000000-0000-0000-0000-00000000000a";
        let legacy = serde_json::json!({
            "revision": 4,
            "projects": [{
                "id": project,
                "display_name": "Payments",
                "canonical_root": folder,
                "position": 1,
                "revision": 1
            }],
            "groups": [{
                "id": group,
                "project_id": project,
                "name": "Review",
                "position": 1,
                "collapsed": false,
                "revision": 2
            }],
            "worktree_intents": [],
            "worktrees": []
        });
        fs::write(
            root.join(FORMAT_FILE),
            format!(
                r#"{{"format_version":{CURRENT_FORMAT_VERSION},"minimum_reader":1,"instance_id":"{INSTANCE_ID}"}}"#
            ),
        )
        .unwrap();
        fs::write(
            root.join("projects.json"),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        project.to_owned()
    }

    #[test]
    fn a_store_from_before_projects_were_removed_keeps_its_groups() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let project_folder = fixture.path().join("payments");
        fs::create_dir(&project_folder).unwrap();
        write_legacy_store(&store_root, &project_folder);

        // Read-only first, as the TUI does: it sees the groups without changing anything.
        let read_only = LibraryRepository::load_existing_read_only(&store_root).unwrap();
        assert_eq!(read_only.groups.len(), 1);
        assert!(store_root.join("projects.json").exists());
        assert!(!store_root.join(LIBRARY_FILE).exists());

        let repo = repository(&store_root);
        let snapshot = repo.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(4));
        assert_eq!(snapshot.groups.len(), 1);
        assert_eq!(snapshot.groups[0].name.as_str(), "Review");
        // The old file is kept under another name, never deleted.
        assert!(!store_root.join("projects.json").exists());
        assert!(store_root.join("projects.migrated.json").exists());
        // Opening again finds nothing left to carry.
        assert_eq!(repository(&store_root).load().unwrap(), snapshot);
    }
}
