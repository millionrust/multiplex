//! Reviewing and resolving a session launch.
//!
//! This used to sit behind a Project: a launch named one, and the Project held the folder the
//! session would run in. The folder was always the part that mattered, so a launch names it
//! directly and the Project is gone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use multiplex_domain::{
    CanonicalPath, HostedSessionId, LaunchPreset, LaunchResolutionError, PresetId, ResolvedLaunch,
    Revision, WorktreeError, WorktreeLaunchDraft, WorktreePlan, resolve_launch,
};
use multiplex_store::PresetSnapshot;

use crate::worktree_launch::{GitRunner, WorktreeCancellation, WorktreeInspection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LaunchReviewError {
    PresetRequired,
    PresetStoreUnavailable,
    ReviewStale,
    FolderMissing,
    PresetMissing,
}

pub(super) struct LaunchReviewInput<'a> {
    pub folder: CanonicalPath,
    pub selected_preset_id: Option<PresetId>,
    pub preset_store_revision: Revision,
    pub preset_snapshot: Option<&'a PresetSnapshot>,
}

#[derive(Clone)]
pub(super) struct ReviewedLaunch {
    folder: CanonicalPath,
    preset: LaunchPreset,
}

#[derive(Debug)]
pub(super) struct LaunchResolution {
    pub resolved: ResolvedLaunch,
    pub folder: CanonicalPath,
    pub preset: LaunchPreset,
}

pub(super) struct WorktreeInspectionRequest {
    pub repository_root: PathBuf,
    pub managed_root: PathBuf,
    pub worktree_id: multiplex_domain::ManagedWorktreeId,
    pub draft: WorktreeLaunchDraft,
    pub cancellation: WorktreeCancellation,
}

pub(super) struct WorktreePlanRequest {
    pub plan: WorktreePlan,
    pub cancellation: WorktreeCancellation,
}

trait LaunchResolver: Send + Sync {
    fn resolve(
        &self,
        session_id: HostedSessionId,
        folder: &CanonicalPath,
        preset: &LaunchPreset,
        path_snapshot: &[PathBuf],
        platform_home: Option<&Path>,
    ) -> Result<ResolvedLaunch, LaunchResolutionError>;
}

struct SystemLaunchResolver;

trait WorktreeWorker: Send + Sync {
    fn inspect(
        &self,
        request: WorktreeInspectionRequest,
    ) -> Result<WorktreeInspection, WorktreeError>;

    fn create(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError>;

    fn verify(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError>;
}

struct SystemWorktreeWorker;

impl LaunchResolver for SystemLaunchResolver {
    fn resolve(
        &self,
        session_id: HostedSessionId,
        folder: &CanonicalPath,
        preset: &LaunchPreset,
        path_snapshot: &[PathBuf],
        platform_home: Option<&Path>,
    ) -> Result<ResolvedLaunch, LaunchResolutionError> {
        resolve_launch(session_id, folder, preset, path_snapshot, platform_home)
    }
}

impl WorktreeWorker for SystemWorktreeWorker {
    fn inspect(
        &self,
        request: WorktreeInspectionRequest,
    ) -> Result<WorktreeInspection, WorktreeError> {
        GitRunner::default().inspect(
            &request.repository_root,
            &request.managed_root,
            request.worktree_id,
            &request.draft,
            &request.cancellation,
        )
    }

    fn create(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
        GitRunner::default().create(&request.plan, &request.cancellation)
    }

    fn verify(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
        GitRunner::default().verify(&request.plan, &request.cancellation)
    }
}

#[derive(Clone)]
pub(super) struct LaunchCoordinator {
    resolver: Arc<dyn LaunchResolver>,
    worktree_worker: Arc<dyn WorktreeWorker>,
}

impl Default for LaunchCoordinator {
    fn default() -> Self {
        Self {
            resolver: Arc::new(SystemLaunchResolver),
            worktree_worker: Arc::new(SystemWorktreeWorker),
        }
    }
}

impl LaunchCoordinator {
    pub fn review_session_launch(
        &self,
        input: LaunchReviewInput<'_>,
    ) -> Result<ReviewedLaunch, LaunchReviewError> {
        let preset_id = input
            .selected_preset_id
            .ok_or(LaunchReviewError::PresetRequired)?;
        let preset_snapshot = input
            .preset_snapshot
            .ok_or(LaunchReviewError::PresetStoreUnavailable)?;
        if preset_snapshot.revision != input.preset_store_revision {
            return Err(LaunchReviewError::ReviewStale);
        }
        // The folder is checked again here, because the review is what the person is looking at
        // and a folder that has gone since they opened it should not reach a launch.
        if CanonicalPath::resolve(input.folder.as_path())
            .is_ok_and(|current| current.identity() == input.folder.identity())
        {
            let preset = preset_snapshot
                .presets
                .iter()
                .find(|preset| preset.id == preset_id)
                .cloned()
                .ok_or(LaunchReviewError::PresetMissing)?;
            Ok(ReviewedLaunch {
                folder: input.folder,
                preset,
            })
        } else {
            Err(LaunchReviewError::FolderMissing)
        }
    }

    pub fn resolve_session_launch(
        &self,
        reviewed: ReviewedLaunch,
        session_id: HostedSessionId,
        path_snapshot: Vec<PathBuf>,
        platform_home: Option<PathBuf>,
    ) -> Result<LaunchResolution, LaunchResolutionError> {
        let resolved = self.resolver.resolve(
            session_id,
            &reviewed.folder,
            &reviewed.preset,
            &path_snapshot,
            platform_home.as_deref(),
        )?;
        Ok(LaunchResolution {
            resolved,
            folder: reviewed.folder,
            preset: reviewed.preset,
        })
    }

    pub fn inspect_worktree(
        &self,
        request: WorktreeInspectionRequest,
    ) -> Result<WorktreeInspection, WorktreeError> {
        self.worktree_worker.inspect(request)
    }

    pub fn create_worktree(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
        self.worktree_worker.create(request)
    }

    pub fn verify_worktree(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
        self.worktree_worker.verify(request)
    }
}
