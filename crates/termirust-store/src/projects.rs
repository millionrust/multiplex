use std::collections::HashSet;
use std::fmt;
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
    AddProject, CanonicalPath, Group, GroupDestination, GroupError, GroupId, GroupInverseCommand,
    GroupMutation, GroupName, LocalizedUserText, MAX_GROUPS_PER_PROJECT,
    MAX_WORKTREE_REGISTRATIONS, ManagedWorktreeId, PositionKey, Project, ProjectError, ProjectId,
    ProjectService, ProjectSummary, Revision, WorktreeError, WorktreeIntent, WorktreeIntentState,
    WorktreeRegistration, validate_group_set,
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
const INTERACTIVE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreHealth {
    Healthy,
    RecoveredLastGood,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSnapshot {
    pub revision: Revision,
    pub projects: Vec<ProjectSummary>,
    pub groups: Vec<Group>,
    pub worktree_intents: Vec<WorktreeIntent>,
    pub worktrees: Vec<WorktreeRegistration>,
    pub health: StoreHealth,
    pub read_only: bool,
    pub durability: Durability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedProject {
    pub project: Project,
    pub groups: Vec<Group>,
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
    GroupDomain(GroupError),
    PresetDomain(termirust_domain::PresetError),
    SessionDomain(termirust_domain::SessionStateError),
    WorktreeDomain(WorktreeError),
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
            Self::GroupDomain(error) => error.fmt(formatter),
            Self::PresetDomain(error) => error.fmt(formatter),
            Self::SessionDomain(error) => error.fmt(formatter),
            Self::WorktreeDomain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<ProjectError> for StoreError {
    fn from(error: ProjectError) -> Self {
        Self::Domain(error)
    }
}

impl From<GroupError> for StoreError {
    fn from(error: GroupError) -> Self {
        Self::GroupDomain(error)
    }
}

impl From<termirust_domain::PresetError> for StoreError {
    fn from(error: termirust_domain::PresetError) -> Self {
        Self::PresetDomain(error)
    }
}

impl From<termirust_domain::SessionStateError> for StoreError {
    fn from(error: termirust_domain::SessionStateError) -> Self {
        Self::SessionDomain(error)
    }
}

impl From<WorktreeError> for StoreError {
    fn from(error: WorktreeError) -> Self {
        Self::WorktreeDomain(error)
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
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    worktree_intents: Vec<WorktreeIntent>,
    #[serde(default)]
    worktrees: Vec<WorktreeRegistration>,
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
        require_project(&document, intent.plan.source_project_id)?;
        if document.projects.len() >= MAX_PROJECTS {
            return Err(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS,
            }
            .into());
        }
        if document.worktrees.len() + document.worktree_intents.len() >= MAX_WORKTREE_REGISTRATIONS
        {
            return Err(WorktreeError::ResourceLimit {
                limit: MAX_WORKTREE_REGISTRATIONS,
            }
            .into());
        }
        if document
            .projects
            .iter()
            .any(|project| project.id == intent.plan.child_project_id)
            || document
                .worktree_intents
                .iter()
                .any(|candidate| worktree_plan_conflicts(&candidate.plan, &intent.plan))
            || document.worktrees.iter().any(|candidate| {
                candidate.id == intent.plan.id
                    || candidate.child_project_id == intent.plan.child_project_id
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
    ) -> Result<(Project, WorktreeRegistration), StoreError> {
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
        require_project(&document, intent.plan.source_project_id)?;
        if document.projects.len() >= MAX_PROJECTS {
            return Err(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS,
            }
            .into());
        }
        let canonical_path = CanonicalPath::resolve(intent.plan.managed_path.as_path())?;
        if canonical_path.as_path() != intent.plan.managed_path.as_path()
            || canonical_path.as_path() == intent.plan.managed_root.as_path()
            || !canonical_path
                .as_path()
                .starts_with(intent.plan.managed_root.as_path())
        {
            return Err(WorktreeError::SymlinkSwap.into());
        }
        if document.projects.iter().any(|project| {
            project.id == intent.plan.child_project_id
                || project.canonical_root.identity() == canonical_path.identity()
        }) || document.worktrees.iter().any(|registration| {
            registration.id == id
                || registration.child_project_id == intent.plan.child_project_id
                || registration.branch == intent.plan.generated_branch
        }) {
            return Err(WorktreeError::RegistrationConflict.into());
        }
        let revision = next_revision(document.revision)?;
        let project = Project {
            id: intent.plan.child_project_id,
            display_name: LocalizedUserText::new(&intent.child_display_name)?,
            canonical_root: canonical_path.clone(),
            position: next_tail_position(&mut document.projects)?,
            revision,
        };
        let registration = WorktreeRegistration {
            id,
            source_project_id: intent.plan.source_project_id,
            child_project_id: project.id,
            repository_root: intent.plan.repository_root,
            managed_root: intent.plan.managed_root,
            managed_path: canonical_path,
            base: intent.plan.selected_base,
            branch: intent.plan.generated_branch,
            revision,
        };
        registration.validate()?;
        document.worktree_intents.remove(intent_index);
        document.projects.push(project.clone());
        document.worktrees.push(registration.clone());
        document.revision = revision;
        sort_projects(&mut document.projects);
        self.write_document_locked(&document)?;
        Ok((project, registration))
    }

    pub fn remove_project(
        &self,
        id: ProjectId,
        expected: Revision,
    ) -> Result<RemovedProject, StoreError> {
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        let index = document
            .projects
            .iter()
            .position(|project| project.id == id)
            .ok_or(StoreError::Domain(ProjectError::Unavailable))?;
        let project = document.projects.remove(index);
        let (groups, retained_groups): (Vec<_>, Vec<_>) = document
            .groups
            .into_iter()
            .partition(|group| group.project_id == project.id);
        document.groups = retained_groups;
        document.revision = next_revision(document.revision)?;
        self.write_document_locked(&document)?;
        Ok(RemovedProject { project, groups })
    }

    pub fn restore_project(
        &self,
        mut removed: RemovedProject,
        expected: Revision,
    ) -> Result<RemovedProject, StoreError> {
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
            candidate.id == removed.project.id
                || candidate.canonical_root.identity() == removed.project.canonical_root.identity()
        }) {
            return Err(StoreError::Domain(ProjectError::AlreadyPresent {
                id: existing.id,
            }));
        }
        if removed
            .groups
            .iter()
            .any(|group| group.project_id != removed.project.id)
        {
            return Err(StoreError::GroupDomain(GroupError::WrongProject));
        }
        if removed.groups.len() > MAX_GROUPS_PER_PROJECT {
            return Err(StoreError::GroupDomain(GroupError::ResourceLimit {
                limit: MAX_GROUPS_PER_PROJECT,
            }));
        }
        let existing_group_ids: HashSet<_> = document.groups.iter().map(|group| group.id).collect();
        if removed
            .groups
            .iter()
            .any(|group| existing_group_ids.contains(&group.id))
        {
            return Err(StoreError::GroupDomain(GroupError::Store {
                code: "duplicate-group-id",
            }));
        }
        let revision = next_revision(document.revision)?;
        removed.project.revision = revision;
        removed.project.position = next_tail_position(&mut document.projects)?;
        for group in &mut removed.groups {
            group.revision = revision;
        }
        document.projects.push(removed.project.clone());
        document.groups.extend(removed.groups.iter().cloned());
        document.revision = revision;
        sort_projects(&mut document.projects);
        sort_groups(&mut document.groups);
        validate_document(&document)?;
        self.write_document_locked(&document)?;
        Ok(removed)
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

    pub fn create_group(
        &self,
        project_id: ProjectId,
        id: GroupId,
        name: &str,
        expected: Revision,
    ) -> Result<GroupMutation<Group>, StoreError> {
        let name = GroupName::new(name)?;
        let _lock = self.acquire_lock()?;
        self.validate_format_locked()?;
        let mut document = self.mutable_document_locked()?;
        require_revision(expected, document.revision)?;
        require_project(&document, project_id)?;
        let project_group_count = document
            .groups
            .iter()
            .filter(|group| group.project_id == project_id)
            .count();
        if project_group_count >= MAX_GROUPS_PER_PROJECT {
            return Err(GroupError::ResourceLimit {
                limit: MAX_GROUPS_PER_PROJECT,
            }
            .into());
        }
        require_unique_group_name(&document.groups, project_id, None, &name)?;
        if document.groups.iter().any(|group| group.id == id) {
            return Err(GroupError::Store {
                code: "duplicate-group-id",
            }
            .into());
        }
        let revision = next_group_revision(document.revision)?;
        let position = next_group_tail_position(&mut document.groups, project_id)?;
        let group = Group {
            id,
            project_id,
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
        let project_id = document.groups[index].project_id;
        require_unique_group_name(&document.groups, project_id, Some(id), &name)?;
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
        let project_id = document.groups[index].project_id;
        if let Some(before_id) = before {
            let destination = document
                .groups
                .iter()
                .find(|group| group.id == before_id)
                .ok_or(StoreError::GroupDomain(GroupError::DestinationNotFound))?;
            if destination.project_id != project_id {
                return Err(GroupError::WrongProject.into());
            }
        }
        let mut order = project_group_ids(&document.groups, project_id);
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
        let current_order = project_group_ids(&document.groups, project_id);
        if order == current_order {
            return Ok(GroupMutation {
                value: document.groups[index].clone(),
                inverse: GroupInverseCommand::MoveBefore {
                    group_id: id,
                    before: previous_before,
                },
            });
        }
        assign_group_position(&mut document.groups, project_id, &order, new_index)?;
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
        let project_id = document.groups[index].project_id;
        let destination = match (has_sessions, destination) {
            (true, None) => return Err(GroupError::NonEmptyDestinationRequired.into()),
            (_, Some(destination)) => destination,
            (false, None) => GroupDestination::ProjectRoot,
        };
        if destination.group_id() == Some(id) {
            return Err(GroupError::DestinationIsSource.into());
        }
        if let Some(destination_id) = destination.group_id() {
            let destination_group = document
                .groups
                .iter()
                .find(|group| group.id == destination_id)
                .ok_or(StoreError::GroupDomain(GroupError::DestinationNotFound))?;
            if destination_group.project_id != project_id {
                return Err(GroupError::WrongProject.into());
            }
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
        require_project(&document, group.project_id)?;
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
        let project_group_count = document
            .groups
            .iter()
            .filter(|candidate| candidate.project_id == group.project_id)
            .count();
        if project_group_count >= MAX_GROUPS_PER_PROJECT {
            return Err(GroupError::ResourceLimit {
                limit: MAX_GROUPS_PER_PROJECT,
            }
            .into());
        }
        require_unique_group_name(&document.groups, group.project_id, None, &group.name)?;
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

        let projects_path = self.root.join(PROJECTS_FILE);
        if !projects_path.exists() {
            let document = ProjectsDocument {
                revision: Revision::ZERO,
                projects: Vec::new(),
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
        sort_groups(&mut document.groups);
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
        if persisted.revision != document.revision
            || persisted.projects != document.projects
            || persisted.groups != document.groups
            || persisted.worktree_intents != document.worktree_intents
            || persisted.worktrees != document.worktrees
        {
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
                    return Err(io_error("lock project metadata", error));
                }
                thread::sleep(LOCK_RETRY_INTERVAL);
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
    let project_ids = document
        .projects
        .iter()
        .map(|project| project.id)
        .collect::<Vec<_>>();
    validate_group_set(&document.groups, &project_ids).map_err(|error| ProjectError::Store {
        code: match error {
            GroupError::DuplicateName => "duplicate-group-name",
            GroupError::ProjectNotFound => "orphaned-group",
            GroupError::ResourceLimit { .. } => "group-limit",
            GroupError::Store { code } => code,
            _ => "invalid-group",
        },
    })?;
    if document
        .groups
        .iter()
        .any(|group| group.revision > document.revision)
    {
        return Err(ProjectError::Store {
            code: "future-group-revision",
        });
    }
    if document.worktree_intents.len() + document.worktrees.len() > MAX_WORKTREE_REGISTRATIONS {
        return Err(ProjectError::Store {
            code: "worktree-limit",
        });
    }
    let mut worktree_ids = HashSet::new();
    let mut worktree_children = HashSet::new();
    let mut worktree_paths = HashSet::new();
    let mut worktree_branches = HashSet::new();
    for intent in &document.worktree_intents {
        intent.plan.validate().map_err(|_| ProjectError::Store {
            code: "invalid-worktree-intent",
        })?;
        LocalizedUserText::new(&intent.child_display_name).map_err(|_| ProjectError::Store {
            code: "invalid-worktree-label",
        })?;
        if intent.revision > document.revision
            || !worktree_ids.insert(intent.plan.id)
            || !worktree_children.insert(intent.plan.child_project_id)
            || !worktree_paths.insert(intent.plan.managed_path.as_path())
            || !worktree_branches.insert(&intent.plan.generated_branch)
        {
            return Err(ProjectError::Store {
                code: "duplicate-worktree-intent",
            });
        }
    }
    for registration in &document.worktrees {
        registration.validate().map_err(|_| ProjectError::Store {
            code: "invalid-worktree-registration",
        })?;
        if registration.revision > document.revision
            || !worktree_ids.insert(registration.id)
            || !worktree_children.insert(registration.child_project_id)
            || !worktree_paths.insert(registration.managed_path.as_path())
            || !worktree_branches.insert(&registration.branch)
        {
            return Err(ProjectError::Store {
                code: "duplicate-worktree-registration",
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
        groups: document.groups,
        worktree_intents: document.worktree_intents,
        worktrees: document.worktrees,
        health,
        read_only,
        durability,
    }
}

fn worktree_plan_conflicts(
    left: &termirust_domain::WorktreePlan,
    right: &termirust_domain::WorktreePlan,
) -> bool {
    left.id == right.id
        || left.child_project_id == right.child_project_id
        || left.generated_branch == right.generated_branch
        || left.managed_path.as_path() == right.managed_path.as_path()
}

fn sort_projects(projects: &mut [Project]) {
    projects.sort_by_key(|project| (project.position, project.id));
}

fn sort_groups(groups: &mut [Group]) {
    groups.sort_by_key(|group| (group.project_id, group.position, group.id));
}

fn group_index(groups: &[Group], id: GroupId) -> Result<usize, StoreError> {
    groups
        .iter()
        .position(|group| group.id == id)
        .ok_or(StoreError::GroupDomain(GroupError::NotFound))
}

fn require_project(document: &ProjectsDocument, id: ProjectId) -> Result<(), StoreError> {
    if document.projects.iter().any(|project| project.id == id) {
        Ok(())
    } else {
        Err(GroupError::ProjectNotFound.into())
    }
}

fn require_unique_group_name(
    groups: &[Group],
    project_id: ProjectId,
    except: Option<GroupId>,
    name: &GroupName,
) -> Result<(), StoreError> {
    let comparison_key = name.comparison_key();
    if groups.iter().any(|group| {
        group.project_id == project_id
            && Some(group.id) != except
            && group.name.comparison_key() == comparison_key
    }) {
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

fn project_group_ids(groups: &[Group], project_id: ProjectId) -> Vec<GroupId> {
    let mut project_groups = groups
        .iter()
        .filter(|group| group.project_id == project_id)
        .collect::<Vec<_>>();
    project_groups.sort_by_key(|group| (group.position, group.id));
    project_groups.into_iter().map(|group| group.id).collect()
}

fn next_group_tail_position(
    groups: &mut [Group],
    project_id: ProjectId,
) -> Result<PositionKey, StoreError> {
    let ids = project_group_ids(groups, project_id);
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
    project_id: ProjectId,
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
            .find(|group| group.project_id == project_id && group.id == *id)
            .ok_or(StoreError::GroupDomain(GroupError::NotFound))?;
        target.position = PositionKey::rebalanced(group_index)
            .map_err(|_| StoreError::GroupDomain(GroupError::PositionOverflow))?;
    }
    Ok(())
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
        StoreError::GroupDomain(_) => ProjectError::Store {
            code: "group-domain",
        },
        StoreError::SessionDomain(_) => ProjectError::Store {
            code: "session-domain",
        },
        StoreError::WorktreeDomain(_) => ProjectError::Store {
            code: "worktree-domain",
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

    fn group_id(value: u128) -> GroupId {
        GroupId::from_uuid(Uuid::from_u128(10_000 + value))
    }

    fn worktree_id(value: u128) -> ManagedWorktreeId {
        ManagedWorktreeId::from_uuid(Uuid::from_u128(20_000 + value))
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

    fn worktree_intent(
        id: ManagedWorktreeId,
        source_project_id: ProjectId,
        child_project_id: ProjectId,
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
            plan: termirust_domain::WorktreePlan::new(
                id,
                source_project_id,
                child_project_id,
                CanonicalPath::resolve(repository_root).unwrap(),
                canonical_managed_root,
                termirust_domain::BaseCandidate {
                    ref_name: termirust_domain::GitReference::new("main").unwrap(),
                    commit_oid: termirust_domain::CommitOid::new(&"a".repeat(40)).unwrap(),
                    source: termirust_domain::BaseSource::ConfiguredMainline,
                },
                termirust_domain::GitReference::new("termirust/worktree/test").unwrap(),
                termirust_domain::ManagedPath::new(canonical_managed_path).unwrap(),
            )
            .unwrap(),
            child_display_name: "Isolated test".to_string(),
            state: WorktreeIntentState::Planned,
            revision: Revision::ZERO,
        }
    }

    #[test]
    fn worktree_registration_is_atomic_persistent_and_project_removal_keeps_evidence() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let repository_root = fixture.path().join("repository");
        let managed_root = fixture.path().join("managed");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&managed_root).unwrap();
        let repo = repository(&store_root);
        let source = repo
            .add_project(add_request(id(1), &repository_root, Revision::ZERO))
            .unwrap();
        let managed_path = managed_root.join("child");
        let intent = repo
            .begin_worktree_intent(
                worktree_intent(
                    worktree_id(1),
                    source.id,
                    id(2),
                    &repository_root,
                    &managed_root,
                    &managed_path,
                ),
                Revision::new(1),
            )
            .unwrap();
        assert_eq!(intent.revision, Revision::new(2));
        fs::create_dir(&managed_path).unwrap();
        let sentinel = managed_path.join("KEEP.txt");
        fs::write(&sentinel, "keep").unwrap();
        let (child, registration) = repo
            .register_worktree_child(worktree_id(1), Revision::new(2))
            .unwrap();
        assert_eq!(child.id, id(2));
        assert_eq!(registration.child_project_id, child.id);

        let reopened = repository(&store_root);
        let snapshot = reopened.load().unwrap();
        assert_eq!(snapshot.worktree_intents.len(), 0);
        assert_eq!(snapshot.worktrees, vec![registration.clone()]);
        assert_eq!(snapshot.projects.len(), 2);
        reopened
            .remove_project(child.id, snapshot.revision)
            .unwrap();
        let after_remove = reopened.load().unwrap();
        assert_eq!(after_remove.worktrees, vec![registration]);
        assert_eq!(after_remove.projects.len(), 1);
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
        let source = repo
            .add_project(add_request(id(1), &repository_root, Revision::ZERO))
            .unwrap();
        let managed_path = managed_root.join("crash-child");
        repo.begin_worktree_intent(
            worktree_intent(
                worktree_id(2),
                source.id,
                id(3),
                &repository_root,
                &managed_root,
                &managed_path,
            ),
            Revision::new(1),
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
        let source = repo
            .add_project(add_request(id(1), &repository_root, Revision::ZERO))
            .unwrap();
        let managed_path = managed_root.join("swapped");
        repo.begin_worktree_intent(
            worktree_intent(
                worktree_id(3),
                source.id,
                id(4),
                &repository_root,
                &managed_root,
                &managed_path,
            ),
            Revision::new(1),
        )
        .unwrap();
        symlink(&outside, &managed_path).unwrap();
        assert!(matches!(
            repo.register_worktree_child(worktree_id(3), Revision::new(2)),
            Err(StoreError::WorktreeDomain(WorktreeError::SymlinkSwap))
        ));
        let snapshot = repo.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(2));
        assert_eq!(snapshot.worktree_intents.len(), 1);
        assert!(snapshot.worktrees.is_empty());
        assert_eq!(snapshot.projects.len(), 1);
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
        let source = normal
            .add_project(add_request(id(1), &repository_root, Revision::ZERO))
            .unwrap();
        let managed_path = managed_root.join("created-before-store-failure");
        normal
            .begin_worktree_intent(
                worktree_intent(
                    worktree_id(4),
                    source.id,
                    id(5),
                    &repository_root,
                    &managed_root,
                    &managed_path,
                ),
                Revision::new(1),
            )
            .unwrap();
        fs::create_dir(&managed_path).unwrap();
        let prior = fs::read(normal.root().join(PROJECTS_FILE)).unwrap();
        let failing = ProjectRepository::open_with(
            &store_root,
            INSTANCE_ID.to_string(),
            Arc::new(DiskFullWriter),
        )
        .unwrap();

        assert!(matches!(
            failing.register_worktree_child(worktree_id(4), Revision::new(2)),
            Err(StoreError::Io {
                kind: io::ErrorKind::StorageFull,
                ..
            })
        ));
        assert_eq!(fs::read(normal.root().join(PROJECTS_FILE)).unwrap(), prior);
        let snapshot = normal.load().unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.worktree_intents.len(), 1);
        assert!(snapshot.worktrees.is_empty());
        assert!(managed_path.is_dir());
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
            groups: Vec::new(),
            worktree_intents: Vec::new(),
            worktrees: Vec::new(),
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
            groups: Vec::new(),
            worktree_intents: Vec::new(),
            worktrees: Vec::new(),
        };
        assert_eq!(
            validate_document(&over_limit),
            Err(ProjectError::ResourceLimit {
                limit: MAX_PROJECTS
            })
        );
    }

    #[test]
    fn groups_crud_order_and_collapse_persist_across_restart() {
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("store");
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&store_root);
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        let first = repo
            .create_group(project.id, group_id(1), "Build", Revision::new(1))
            .unwrap();
        assert_eq!(
            first.inverse,
            GroupInverseCommand::RemoveCreated {
                group_id: group_id(1)
            }
        );
        repo.create_group(project.id, group_id(2), "Review", Revision::new(2))
            .unwrap();
        repo.rename_group(group_id(1), "Implement", Revision::new(3))
            .unwrap();
        repo.set_group_collapsed(group_id(2), true, Revision::new(4))
            .unwrap();
        repo.move_group_before(group_id(2), Some(group_id(1)), Revision::new(5))
            .unwrap();
        drop(repo);

        let snapshot = repository(&store_root).load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(6));
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
    fn groups_survive_project_removal_and_atomic_undo() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        repo.create_group(project.id, group_id(1), "Build", Revision::new(1))
            .unwrap();
        repo.create_group(project.id, group_id(2), "Review", Revision::new(2))
            .unwrap();
        repo.set_group_collapsed(group_id(2), true, Revision::new(3))
            .unwrap();
        let before = repo.load().unwrap();

        let removed = repo.remove_project(project.id, before.revision).unwrap();
        assert_eq!(removed.groups, before.groups);
        let after_remove = repo.load().unwrap();
        assert!(after_remove.projects.is_empty());
        assert!(after_remove.groups.is_empty());

        let restored = repo
            .restore_project(removed, after_remove.revision)
            .unwrap();
        let after_restore = repo.load().unwrap();
        assert_eq!(restored.project.id, project.id);
        assert_eq!(after_restore.projects.len(), 1);
        assert_eq!(
            after_restore
                .groups
                .iter()
                .map(|group| (
                    group.id,
                    group.name.as_str(),
                    group.position,
                    group.collapsed
                ))
                .collect::<Vec<_>>(),
            before
                .groups
                .iter()
                .map(|group| (
                    group.id,
                    group.name.as_str(),
                    group.position,
                    group.collapsed
                ))
                .collect::<Vec<_>>()
        );
        assert!(
            after_restore
                .groups
                .iter()
                .all(|group| group.revision == after_restore.revision)
        );
    }

    #[test]
    fn groups_reject_duplicate_names_and_stale_mutations_without_change() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        repo.create_group(project.id, group_id(1), "Review", Revision::new(1))
            .unwrap();
        assert_eq!(
            repo.create_group(project.id, group_id(2), "review", Revision::new(2)),
            Err(StoreError::GroupDomain(GroupError::DuplicateName))
        );
        assert!(matches!(
            repo.rename_group(group_id(1), "Changed", Revision::new(1)),
            Err(StoreError::Domain(ProjectError::StaleRevision { .. }))
        ));
        let snapshot = repo.load().unwrap();
        assert_eq!(snapshot.revision, Revision::new(2));
        assert_eq!(snapshot.groups[0].name.as_str(), "Review");
    }

    #[test]
    fn groups_concurrent_creation_from_one_revision_commits_exactly_once() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles = [(group_id(1), "First"), (group_id(2), "Second")]
            .into_iter()
            .map(|(group_id, name)| {
                let repo = repo.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    repo.create_group(project.id, group_id, name, Revision::new(1))
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
                    Err(StoreError::Domain(ProjectError::StaleRevision { .. }))
                ))
                .count(),
            1
        );
        assert_eq!(repo.load().unwrap().groups.len(), 1);
    }

    #[test]
    fn groups_non_empty_removal_requires_valid_explicit_destination_and_restores() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        repo.create_group(project.id, group_id(1), "Source", Revision::new(1))
            .unwrap();
        repo.create_group(project.id, group_id(2), "Destination", Revision::new(2))
            .unwrap();
        assert_eq!(
            repo.remove_group(group_id(1), None, true, Revision::new(3)),
            Err(StoreError::GroupDomain(
                GroupError::NonEmptyDestinationRequired
            ))
        );
        let removed = repo
            .remove_group(
                group_id(1),
                Some(GroupDestination::Group(group_id(2))),
                true,
                Revision::new(3),
            )
            .unwrap();
        assert_eq!(removed.value.id, group_id(1));
        assert_eq!(repo.load().unwrap().groups.len(), 1);
        repo.restore_group(removed.value, Revision::new(4)).unwrap();
        assert_eq!(repo.load().unwrap().groups.len(), 2);
    }

    #[test]
    fn groups_missing_removal_destination_preserves_document() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("project");
        fs::create_dir(&project_root).unwrap();
        let repo = repository(&fixture.path().join("store"));
        let project = repo
            .add_project(add_request(id(1), &project_root, Revision::ZERO))
            .unwrap();
        repo.create_group(project.id, group_id(1), "Source", Revision::new(1))
            .unwrap();
        let before = fs::read(repo.root().join(PROJECTS_FILE)).unwrap();
        assert_eq!(
            repo.remove_group(
                group_id(1),
                Some(GroupDestination::Group(group_id(99))),
                true,
                Revision::new(2),
            ),
            Err(StoreError::GroupDomain(GroupError::DestinationNotFound))
        );
        assert_eq!(fs::read(repo.root().join(PROJECTS_FILE)).unwrap(), before);
    }

    #[test]
    fn legacy_projects_document_without_groups_loads_as_empty() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = repository(&fixture.path().join("store"));
        fs::write(
            repo.root().join(PROJECTS_FILE),
            br#"{"revision":0,"projects":[]}"#,
        )
        .unwrap();
        let snapshot = repo.load().unwrap();
        assert!(snapshot.groups.is_empty());
        assert!(snapshot.worktree_intents.is_empty());
        assert!(snapshot.worktrees.is_empty());
    }
}
