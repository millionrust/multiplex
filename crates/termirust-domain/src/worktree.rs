use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::MAX_PATH_BYTES;
use crate::{
    CanonicalPath, HostedSessionId, ManagedWorktreeId, PresetId, Project, ProjectId, Revision,
};

pub const MAX_GIT_REF_BYTES: usize = 1_024;
pub const MAX_COMMIT_OID_BYTES: usize = 64;
pub const MAX_WORKTREE_REGISTRATIONS: usize = 1_000;

#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GitReference(String);

impl GitReference {
    pub fn new(value: &str) -> Result<Self, WorktreeError> {
        if value.is_empty() {
            return Err(WorktreeError::InvalidReference);
        }
        if value.len() > MAX_GIT_REF_BYTES
            || value.contains('\0')
            || value.chars().any(char::is_control)
        {
            return Err(WorktreeError::InvalidReference);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> Result<(), WorktreeError> {
        Self::new(&self.0).map(|_| ())
    }
}

impl fmt::Debug for GitReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GitReference(<redacted>)")
    }
}

impl fmt::Display for GitReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CommitOid(String);

impl CommitOid {
    pub fn new(value: &str) -> Result<Self, WorktreeError> {
        if !(40..=MAX_COMMIT_OID_BYTES).contains(&value.len())
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(WorktreeError::InvalidOid);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(12)]
    }

    pub fn validate(&self) -> Result<(), WorktreeError> {
        Self::new(&self.0).map(|_| ())
    }
}

impl fmt::Debug for CommitOid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CommitOid")
            .field(&self.short())
            .finish()
    }
}

impl fmt::Display for CommitOid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ManagedPath(PathBuf);

impl ManagedPath {
    pub fn new(path: PathBuf) -> Result<Self, WorktreeError> {
        let encoded = path.to_str().ok_or(WorktreeError::InvalidPath)?;
        if !path.is_absolute()
            || encoded.is_empty()
            || encoded.contains('\0')
            || encoded.len() > MAX_PATH_BYTES
        {
            return Err(WorktreeError::InvalidPath);
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn validate(&self) -> Result<(), WorktreeError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

impl fmt::Debug for ManagedPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ManagedPath(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseSource {
    UserSelected,
    ConfiguredMainline,
    FetchedRemoteMainline,
    CurrentBranchConfirmed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BaseCandidate {
    pub ref_name: GitReference,
    pub commit_oid: CommitOid,
    pub source: BaseSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeLaunchDraft {
    pub source_project_id: ProjectId,
    pub requested_base: Option<GitReference>,
    pub fetch: bool,
    pub confirm_current_branch: bool,
    pub branch: GitReference,
    pub preset_id: Option<PresetId>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreePlan {
    pub id: ManagedWorktreeId,
    pub source_project_id: ProjectId,
    pub child_project_id: ProjectId,
    pub repository_root: CanonicalPath,
    pub managed_root: CanonicalPath,
    pub selected_base: BaseCandidate,
    pub generated_branch: GitReference,
    pub managed_path: ManagedPath,
}

impl WorktreePlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ManagedWorktreeId,
        source_project_id: ProjectId,
        child_project_id: ProjectId,
        repository_root: CanonicalPath,
        managed_root: CanonicalPath,
        selected_base: BaseCandidate,
        generated_branch: GitReference,
        managed_path: ManagedPath,
    ) -> Result<Self, WorktreeError> {
        let plan = Self {
            id,
            source_project_id,
            child_project_id,
            repository_root,
            managed_root,
            selected_base,
            generated_branch,
            managed_path,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), WorktreeError> {
        self.selected_base.ref_name.validate()?;
        self.selected_base.commit_oid.validate()?;
        self.generated_branch.validate()?;
        self.managed_path.validate()?;
        if self.source_project_id == self.child_project_id
            || self.managed_path.as_path() == self.managed_root.as_path()
            || !self
                .managed_path
                .as_path()
                .starts_with(self.managed_root.as_path())
            || self
                .managed_root
                .as_path()
                .starts_with(self.repository_root.as_path())
        {
            return Err(WorktreeError::Containment);
        }
        Ok(())
    }
}

impl fmt::Debug for WorktreePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorktreePlan")
            .field("id", &self.id)
            .field("source_project_id", &self.source_project_id)
            .field("child_project_id", &self.child_project_id)
            .field("repository_root", &"<redacted>")
            .field("managed_root", &"<redacted>")
            .field("selected_base", &self.selected_base)
            .field("generated_branch", &"<redacted>")
            .field("managed_path", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeIntentState {
    Planned,
    NeedsInspection,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreeIntent {
    pub plan: WorktreePlan,
    pub child_display_name: String,
    pub state: WorktreeIntentState,
    pub revision: Revision,
}

impl fmt::Debug for WorktreeIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorktreeIntent")
            .field("id", &self.plan.id)
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("content", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreeRegistration {
    pub id: ManagedWorktreeId,
    pub source_project_id: ProjectId,
    pub child_project_id: ProjectId,
    pub repository_root: CanonicalPath,
    pub managed_root: CanonicalPath,
    pub managed_path: CanonicalPath,
    pub base: BaseCandidate,
    pub branch: GitReference,
    pub revision: Revision,
}

impl WorktreeRegistration {
    pub fn validate(&self) -> Result<(), WorktreeError> {
        self.base.ref_name.validate()?;
        self.base.commit_oid.validate()?;
        self.branch.validate()?;
        if self.source_project_id == self.child_project_id
            || self.managed_path.as_path() == self.managed_root.as_path()
            || !self
                .managed_path
                .as_path()
                .starts_with(self.managed_root.as_path())
            || self
                .managed_root
                .as_path()
                .starts_with(self.repository_root.as_path())
        {
            return Err(WorktreeError::Containment);
        }
        Ok(())
    }
}

impl fmt::Debug for WorktreeRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorktreeRegistration")
            .field("id", &self.id)
            .field("source_project_id", &self.source_project_id)
            .field("child_project_id", &self.child_project_id)
            .field("base", &self.base)
            .field("revision", &self.revision)
            .field("paths", &"<redacted>")
            .field("branch", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeLaunchStage {
    Inspecting,
    Ready,
    Creating,
    Verifying,
    Registered,
    Launching,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeLaunchOutcome {
    pub child_project: Project,
    pub optional_session: Option<HostedSessionId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeError {
    InvalidRepository,
    InvalidReference,
    InvalidOid,
    InvalidPath,
    DirtySource,
    DetachedHead,
    NoBase,
    SubmodulesUnsupported,
    BranchCollision,
    PathCollision,
    Containment,
    SymlinkSwap,
    GitUnavailable,
    FetchFailed,
    PermissionDenied,
    StorageFull,
    GitFailed { code: &'static str },
    Timeout,
    Cancelled,
    OutputLimit,
    VerificationMismatch,
    RegistrationConflict,
    ResourceLimit { limit: usize },
    Store { code: &'static str },
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepository => formatter.write_str("folder is not a valid Git repository"),
            Self::InvalidReference => formatter.write_str("Git reference is invalid"),
            Self::InvalidOid => formatter.write_str("Git commit identity is invalid"),
            Self::InvalidPath => formatter.write_str("managed worktree path is invalid"),
            Self::DirtySource => formatter.write_str("source repository has uncommitted changes"),
            Self::DetachedHead => formatter.write_str("current Git checkout is detached"),
            Self::NoBase => formatter.write_str("no safe worktree base is available"),
            Self::SubmodulesUnsupported => formatter.write_str("repository uses submodules"),
            Self::BranchCollision => formatter.write_str("worktree branch already exists"),
            Self::PathCollision => formatter.write_str("managed worktree path already exists"),
            Self::Containment => formatter.write_str("managed worktree escaped its allowed root"),
            Self::SymlinkSwap => {
                formatter.write_str("managed worktree path changed during creation")
            }
            Self::GitUnavailable => formatter.write_str("Git is unavailable"),
            Self::FetchFailed => formatter.write_str("Git fetch failed"),
            Self::PermissionDenied => formatter.write_str("worktree access was denied"),
            Self::StorageFull => formatter.write_str("worktree storage is full"),
            Self::GitFailed { code } => write!(formatter, "Git operation failed ({code})"),
            Self::Timeout => formatter.write_str("Git operation timed out"),
            Self::Cancelled => formatter.write_str("worktree operation was cancelled"),
            Self::OutputLimit => formatter.write_str("Git output exceeded the safety limit"),
            Self::VerificationMismatch => {
                formatter.write_str("Git verification did not match the plan")
            }
            Self::RegistrationConflict => {
                formatter.write_str("worktree registration conflicts with stored state")
            }
            Self::ResourceLimit { limit } => write!(formatter, "worktree limit of {limit} reached"),
            Self::Store { code } => write!(formatter, "worktree store error ({code})"),
        }
    }
}

impl std::error::Error for WorktreeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_reference_and_oid_are_bounded() {
        assert!(GitReference::new("main").is_ok());
        assert_eq!(
            GitReference::new("bad\nref"),
            Err(WorktreeError::InvalidReference)
        );
        assert_eq!(
            GitReference::new(&"x".repeat(MAX_GIT_REF_BYTES + 1)),
            Err(WorktreeError::InvalidReference)
        );
        assert_eq!(
            CommitOid::new(&"a".repeat(40)).unwrap().short(),
            "aaaaaaaaaaaa"
        );
        assert_eq!(CommitOid::new("abc"), Err(WorktreeError::InvalidOid));
    }

    #[test]
    fn managed_paths_and_debug_are_content_free() {
        let path = ManagedPath::new(PathBuf::from("/private/canary-secret")).unwrap();
        assert!(!format!("{path:?}").contains("canary-secret"));
        assert_eq!(
            ManagedPath::new(PathBuf::from("relative")),
            Err(WorktreeError::InvalidPath)
        );
    }

    #[test]
    fn plan_rejects_root_equality_and_escape() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = fixture.path().join("repository");
        let managed = fixture.path().join("managed");
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::create_dir_all(&managed).unwrap();
        let repository = CanonicalPath::resolve(&repository).unwrap();
        let managed = CanonicalPath::resolve(&managed).unwrap();
        let base = BaseCandidate {
            ref_name: GitReference::new("main").unwrap(),
            commit_oid: CommitOid::new(&"a".repeat(40)).unwrap(),
            source: BaseSource::ConfiguredMainline,
        };
        let result = WorktreePlan::new(
            ManagedWorktreeId::new(),
            ProjectId::new(),
            ProjectId::new(),
            repository,
            managed.clone(),
            base,
            GitReference::new("termirust/test").unwrap(),
            ManagedPath::new(managed.as_path().to_path_buf()).unwrap(),
        );
        assert_eq!(result, Err(WorktreeError::Containment));
    }
}
