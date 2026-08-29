use std::path::{Path, PathBuf};
use std::sync::Arc;

use termirust_domain::{
    HostedSessionId, LaunchPreset, LaunchResolutionError, PresetId, Project, ProjectId,
    ResolvedLaunch, Revision, WorktreeError, WorktreeLaunchDraft, WorktreePlan, resolve_launch,
};
use termirust_store::{PresetSnapshot, ProjectSnapshot};

use crate::worktree_launch::{GitRunner, WorktreeCancellation, WorktreeInspection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProjectLaunchReviewError {
    PresetRequired,
    ProjectStoreUnavailable,
    PresetStoreUnavailable,
    ReviewStale,
    ProjectMissing,
    PresetMissing,
}

pub(super) struct ProjectLaunchReviewInput<'a> {
    pub project_id: ProjectId,
    pub selected_preset_id: Option<PresetId>,
    pub project_store_revision: Revision,
    pub preset_store_revision: Revision,
    pub project_snapshot: Option<&'a ProjectSnapshot>,
    pub preset_snapshot: Option<&'a PresetSnapshot>,
}

#[derive(Clone)]
pub(super) struct ReviewedProjectLaunch {
    project: Project,
    preset: LaunchPreset,
}

#[derive(Debug)]
pub(super) struct ProjectLaunchResolution {
    pub resolved: ResolvedLaunch,
    pub project: Project,
    pub preset: LaunchPreset,
}

pub(super) struct WorktreeInspectionRequest {
    pub project_root: PathBuf,
    pub managed_root: PathBuf,
    pub worktree_id: termirust_domain::ManagedWorktreeId,
    pub child_project_id: ProjectId,
    pub draft: WorktreeLaunchDraft,
    pub cancellation: WorktreeCancellation,
}

pub(super) struct WorktreePlanRequest {
    pub plan: WorktreePlan,
    pub cancellation: WorktreeCancellation,
}

trait ProjectLaunchResolver: Send + Sync {
    fn resolve(
        &self,
        session_id: HostedSessionId,
        project: &Project,
        preset: &LaunchPreset,
        path_snapshot: &[PathBuf],
        platform_home: Option<&Path>,
    ) -> Result<ResolvedLaunch, LaunchResolutionError>;
}

struct SystemProjectLaunchResolver;

trait ProjectWorktreeWorker: Send + Sync {
    fn inspect(
        &self,
        request: WorktreeInspectionRequest,
    ) -> Result<WorktreeInspection, WorktreeError>;

    fn create(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError>;

    fn verify(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError>;
}

struct SystemProjectWorktreeWorker;

impl ProjectLaunchResolver for SystemProjectLaunchResolver {
    fn resolve(
        &self,
        session_id: HostedSessionId,
        project: &Project,
        preset: &LaunchPreset,
        path_snapshot: &[PathBuf],
        platform_home: Option<&Path>,
    ) -> Result<ResolvedLaunch, LaunchResolutionError> {
        resolve_launch(session_id, project, preset, path_snapshot, platform_home)
    }
}

impl ProjectWorktreeWorker for SystemProjectWorktreeWorker {
    fn inspect(
        &self,
        request: WorktreeInspectionRequest,
    ) -> Result<WorktreeInspection, WorktreeError> {
        GitRunner::default().inspect(
            &request.project_root,
            &request.managed_root,
            request.worktree_id,
            request.child_project_id,
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
pub(super) struct ProjectCoordinator {
    resolver: Arc<dyn ProjectLaunchResolver>,
    worktree_worker: Arc<dyn ProjectWorktreeWorker>,
}

impl Default for ProjectCoordinator {
    fn default() -> Self {
        Self {
            resolver: Arc::new(SystemProjectLaunchResolver),
            worktree_worker: Arc::new(SystemProjectWorktreeWorker),
        }
    }
}

impl ProjectCoordinator {
    pub fn review_session_launch(
        &self,
        input: ProjectLaunchReviewInput<'_>,
    ) -> Result<ReviewedProjectLaunch, ProjectLaunchReviewError> {
        let preset_id = input
            .selected_preset_id
            .ok_or(ProjectLaunchReviewError::PresetRequired)?;
        let project_snapshot = input
            .project_snapshot
            .ok_or(ProjectLaunchReviewError::ProjectStoreUnavailable)?;
        let preset_snapshot = input
            .preset_snapshot
            .ok_or(ProjectLaunchReviewError::PresetStoreUnavailable)?;
        if project_snapshot.revision != input.project_store_revision
            || preset_snapshot.revision != input.preset_store_revision
        {
            return Err(ProjectLaunchReviewError::ReviewStale);
        }
        let project = project_snapshot
            .projects
            .iter()
            .find(|summary| summary.project.id == input.project_id)
            .map(|summary| summary.project.clone())
            .ok_or(ProjectLaunchReviewError::ProjectMissing)?;
        let preset = preset_snapshot
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned()
            .ok_or(ProjectLaunchReviewError::PresetMissing)?;
        Ok(ReviewedProjectLaunch { project, preset })
    }

    pub fn resolve_session_launch(
        &self,
        reviewed: ReviewedProjectLaunch,
        session_id: HostedSessionId,
        path_snapshot: Vec<PathBuf>,
        platform_home: Option<PathBuf>,
    ) -> Result<ProjectLaunchResolution, LaunchResolutionError> {
        let resolved = self.resolver.resolve(
            session_id,
            &reviewed.project,
            &reviewed.preset,
            &path_snapshot,
            platform_home.as_deref(),
        )?;
        Ok(ProjectLaunchResolution {
            resolved,
            project: reviewed.project,
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use termirust_domain::{
        BaseCandidate, BaseSource, CanonicalPath, CommitOid, GitReference, HostedSessionId,
        LaunchPreset, LaunchResolutionError, LocalizedUserText, ManagedPath, ManagedWorktreeId,
        PermissionPolicy, PositionKey, PresetDraft, PresetId, PresetOrigin, Project, ProjectId,
        ResolvedLaunch, Revision, WorkingDirectoryRule, WorktreeError, WorktreeLaunchDraft,
        WorktreePlan,
    };
    use termirust_store::{Durability, PresetSnapshot, ProjectSnapshot, StoreHealth};

    use super::{
        ProjectCoordinator, ProjectLaunchResolver, ProjectLaunchReviewError,
        ProjectLaunchReviewInput, ProjectWorktreeWorker, ReviewedProjectLaunch,
        SystemProjectWorktreeWorker, WorktreeInspectionRequest, WorktreePlanRequest,
    };
    use crate::worktree_launch::{WorktreeCancellation, WorktreeInspection};

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ResolveCall {
        session_id: HostedSessionId,
        project_id: ProjectId,
        preset_id: PresetId,
        path_snapshot: Vec<PathBuf>,
        platform_home: Option<PathBuf>,
    }

    struct RecordingResolver {
        calls: Arc<Mutex<Vec<ResolveCall>>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum WorktreeCall {
        Inspect {
            project_root: PathBuf,
            managed_root: PathBuf,
            worktree_id: ManagedWorktreeId,
            child_project_id: ProjectId,
            draft: WorktreeLaunchDraft,
        },
        Create {
            plan: WorktreePlan,
        },
        Verify {
            plan: WorktreePlan,
        },
    }

    struct RecordingWorktreeWorker {
        calls: Arc<Mutex<Vec<WorktreeCall>>>,
    }

    impl ProjectWorktreeWorker for RecordingWorktreeWorker {
        fn inspect(
            &self,
            request: WorktreeInspectionRequest,
        ) -> Result<WorktreeInspection, WorktreeError> {
            self.calls.lock().unwrap().push(WorktreeCall::Inspect {
                project_root: request.project_root,
                managed_root: request.managed_root,
                worktree_id: request.worktree_id,
                child_project_id: request.child_project_id,
                draft: request.draft,
            });
            request.cancellation.cancel();
            Err(WorktreeError::DirtySource)
        }

        fn create(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
            self.calls
                .lock()
                .unwrap()
                .push(WorktreeCall::Create { plan: request.plan });
            request.cancellation.cancel();
            Err(WorktreeError::PathCollision)
        }

        fn verify(&self, request: WorktreePlanRequest) -> Result<(), WorktreeError> {
            self.calls
                .lock()
                .unwrap()
                .push(WorktreeCall::Verify { plan: request.plan });
            request.cancellation.cancel();
            Err(WorktreeError::VerificationMismatch)
        }
    }

    impl ProjectLaunchResolver for RecordingResolver {
        fn resolve(
            &self,
            session_id: HostedSessionId,
            project: &Project,
            preset: &LaunchPreset,
            path_snapshot: &[PathBuf],
            platform_home: Option<&Path>,
        ) -> Result<ResolvedLaunch, LaunchResolutionError> {
            self.calls.lock().unwrap().push(ResolveCall {
                session_id,
                project_id: project.id,
                preset_id: preset.id,
                path_snapshot: path_snapshot.to_vec(),
                platform_home: platform_home.map(Path::to_path_buf),
            });
            Err(LaunchResolutionError::ExecutableMissing)
        }
    }

    fn fixtures() -> (tempfile::TempDir, Project, LaunchPreset) {
        let root = tempfile::tempdir().unwrap();
        let project = Project {
            id: ProjectId::new(),
            display_name: LocalizedUserText::new("Project coordinator fixture").unwrap(),
            canonical_root: CanonicalPath::resolve(root.path()).unwrap(),
            position: PositionKey::FIRST,
            revision: Revision::new(7),
        };
        let preset = PresetDraft {
            id: PresetId::new(),
            label: "fixture-shell".to_string(),
            executable: std::env::current_exe().unwrap().display().to_string(),
            args: vec!["--exact".to_string()],
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: None,
            enabled: true,
            favorite: true,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::User,
            confirm_risky_favorite: false,
        }
        .validate(PositionKey::FIRST, Revision::new(9))
        .unwrap();
        (root, project, preset)
    }

    fn snapshots(project: &Project, preset: &LaunchPreset) -> (ProjectSnapshot, PresetSnapshot) {
        (
            ProjectSnapshot {
                revision: project.revision,
                projects: vec![project.clone().into()],
                groups: Vec::new(),
                worktree_intents: Vec::new(),
                worktrees: Vec::new(),
                health: StoreHealth::Healthy,
                read_only: false,
                durability: Durability::Full,
            },
            PresetSnapshot {
                revision: preset.revision,
                presets: vec![preset.clone()],
                health: StoreHealth::Healthy,
                read_only: false,
                durability: Durability::Full,
            },
        )
    }

    #[test]
    fn launch_review_preserves_failure_precedence_and_exact_selection() {
        let (_root, project, preset) = fixtures();
        let (projects, presets) = snapshots(&project, &preset);
        let coordinator = ProjectCoordinator::default();
        let input = |selected_preset_id, project_snapshot, preset_snapshot, project_revision| {
            ProjectLaunchReviewInput {
                project_id: project.id,
                selected_preset_id,
                project_store_revision: project_revision,
                preset_store_revision: preset.revision,
                project_snapshot,
                preset_snapshot,
            }
        };

        assert!(matches!(
            coordinator.review_session_launch(input(None, None, None, project.revision)),
            Err(ProjectLaunchReviewError::PresetRequired)
        ));
        assert!(matches!(
            coordinator
                .review_session_launch(input(Some(preset.id), None, None, project.revision,)),
            Err(ProjectLaunchReviewError::ProjectStoreUnavailable)
        ));
        assert!(matches!(
            coordinator.review_session_launch(input(
                Some(preset.id),
                Some(&projects),
                None,
                project.revision,
            )),
            Err(ProjectLaunchReviewError::PresetStoreUnavailable)
        ));
        assert!(matches!(
            coordinator.review_session_launch(input(
                Some(preset.id),
                Some(&projects),
                Some(&presets),
                Revision::new(project.revision.get() + 1),
            )),
            Err(ProjectLaunchReviewError::ReviewStale)
        ));

        let reviewed = coordinator
            .review_session_launch(input(
                Some(preset.id),
                Some(&projects),
                Some(&presets),
                project.revision,
            ))
            .unwrap();
        assert_eq!(reviewed.project, project);
        assert_eq!(reviewed.preset, preset);
    }

    #[test]
    fn launch_review_distinguishes_missing_project_and_preset() {
        let (_root, project, preset) = fixtures();
        let (mut projects, mut presets) = snapshots(&project, &preset);
        let coordinator = ProjectCoordinator::default();
        projects.projects.clear();
        assert!(matches!(
            coordinator.review_session_launch(ProjectLaunchReviewInput {
                project_id: project.id,
                selected_preset_id: Some(preset.id),
                project_store_revision: projects.revision,
                preset_store_revision: presets.revision,
                project_snapshot: Some(&projects),
                preset_snapshot: Some(&presets),
            }),
            Err(ProjectLaunchReviewError::ProjectMissing)
        ));

        projects.projects.push(project.clone().into());
        presets.presets.clear();
        assert!(matches!(
            coordinator.review_session_launch(ProjectLaunchReviewInput {
                project_id: project.id,
                selected_preset_id: Some(preset.id),
                project_store_revision: projects.revision,
                preset_store_revision: presets.revision,
                project_snapshot: Some(&projects),
                preset_snapshot: Some(&presets),
            }),
            Err(ProjectLaunchReviewError::PresetMissing)
        ));
    }

    #[test]
    fn resolver_receives_exact_reviewed_entities_session_environment_and_error() {
        let (_root, project, preset) = fixtures();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = ProjectCoordinator {
            resolver: Arc::new(RecordingResolver {
                calls: calls.clone(),
            }),
            worktree_worker: Arc::new(SystemProjectWorktreeWorker),
        };
        let session_id = HostedSessionId::new();
        let path_snapshot = vec![PathBuf::from("/exact/one"), PathBuf::from("/exact/two")];
        let platform_home = Some(PathBuf::from("/exact/home"));
        let result = coordinator.resolve_session_launch(
            ReviewedProjectLaunch {
                project: project.clone(),
                preset: preset.clone(),
            },
            session_id,
            path_snapshot.clone(),
            platform_home.clone(),
        );

        assert_eq!(
            result.unwrap_err(),
            LaunchResolutionError::ExecutableMissing
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [ResolveCall {
                session_id,
                project_id: project.id,
                preset_id: preset.id,
                path_snapshot,
                platform_home,
            }]
        );
    }

    #[test]
    fn system_resolver_returns_the_exact_reviewed_project_and_preset() {
        let (_root, project, preset) = fixtures();
        let coordinator = ProjectCoordinator::default();
        let session_id = HostedSessionId::new();
        let result = coordinator
            .resolve_session_launch(
                ReviewedProjectLaunch {
                    project: project.clone(),
                    preset: preset.clone(),
                },
                session_id,
                Vec::new(),
                None,
            )
            .unwrap();
        assert_eq!(result.resolved.session_id, session_id);
        assert_eq!(result.project, project);
        assert_eq!(result.preset, preset);
    }

    fn worktree_plan(
        repository: &Path,
        managed_root: &Path,
        worktree_id: ManagedWorktreeId,
        source_project_id: ProjectId,
        child_project_id: ProjectId,
    ) -> WorktreePlan {
        let repository_root = CanonicalPath::resolve(repository).unwrap();
        let managed_root = CanonicalPath::resolve(managed_root).unwrap();
        WorktreePlan::new(
            worktree_id,
            source_project_id,
            child_project_id,
            repository_root,
            managed_root.clone(),
            BaseCandidate {
                ref_name: GitReference::new("main").unwrap(),
                commit_oid: CommitOid::new(&"a".repeat(40)).unwrap(),
                source: BaseSource::ConfiguredMainline,
            },
            GitReference::new("termirust/worktree/exact").unwrap(),
            ManagedPath::new(managed_root.as_path().join("child")).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn worktree_operations_preserve_exact_inputs_shared_cancellation_and_errors() {
        let repository = tempfile::tempdir().unwrap();
        let managed_root = tempfile::tempdir().unwrap();
        let worktree_id = ManagedWorktreeId::new();
        let source_project_id = ProjectId::new();
        let child_project_id = ProjectId::new();
        let plan = worktree_plan(
            repository.path(),
            managed_root.path(),
            worktree_id,
            source_project_id,
            child_project_id,
        );
        let draft = WorktreeLaunchDraft {
            source_project_id,
            requested_base: Some(GitReference::new("origin/main").unwrap()),
            fetch: true,
            confirm_current_branch: false,
            branch: plan.generated_branch.clone(),
            preset_id: Some(PresetId::new()),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = ProjectCoordinator {
            resolver: Arc::new(super::SystemProjectLaunchResolver),
            worktree_worker: Arc::new(RecordingWorktreeWorker {
                calls: calls.clone(),
            }),
        };
        let inspect_cancellation = WorktreeCancellation::default();
        let create_cancellation = WorktreeCancellation::default();
        let verify_cancellation = WorktreeCancellation::default();

        assert_eq!(
            coordinator.inspect_worktree(WorktreeInspectionRequest {
                project_root: repository.path().to_path_buf(),
                managed_root: managed_root.path().to_path_buf(),
                worktree_id,
                child_project_id,
                draft: draft.clone(),
                cancellation: inspect_cancellation.clone(),
            }),
            Err(WorktreeError::DirtySource)
        );
        assert_eq!(
            coordinator.create_worktree(WorktreePlanRequest {
                plan: plan.clone(),
                cancellation: create_cancellation.clone(),
            }),
            Err(WorktreeError::PathCollision)
        );
        assert_eq!(
            coordinator.verify_worktree(WorktreePlanRequest {
                plan: plan.clone(),
                cancellation: verify_cancellation.clone(),
            }),
            Err(WorktreeError::VerificationMismatch)
        );
        assert!(inspect_cancellation.is_cancelled());
        assert!(create_cancellation.is_cancelled());
        assert!(verify_cancellation.is_cancelled());
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                WorktreeCall::Inspect {
                    project_root: repository.path().to_path_buf(),
                    managed_root: managed_root.path().to_path_buf(),
                    worktree_id,
                    child_project_id,
                    draft,
                },
                WorktreeCall::Create { plan: plan.clone() },
                WorktreeCall::Verify { plan },
            ]
        );
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("project_coordinator.rs").contains(&forbidden_crate));
    }

    #[test]
    fn project_coordinator_is_the_only_ui_launch_resolution_boundary() {
        let app_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/app");
        for entry in std::fs::read_dir(app_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("rs")
                || path.file_name().and_then(|value| value.to_str())
                    == Some("project_coordinator.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("resolve_launch("),
                "{} bypasses ProjectCoordinator launch resolution",
                path.display()
            );
            assert!(
                !source.contains("GitRunner::default()"),
                "{} bypasses ProjectCoordinator worktree operations",
                path.display()
            );
        }
    }
}
