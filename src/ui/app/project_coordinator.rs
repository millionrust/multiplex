use std::path::{Path, PathBuf};
use std::sync::Arc;

use termirust_domain::{
    HostedSessionId, LaunchPreset, LaunchResolutionError, PresetId, Project, ProjectId,
    ResolvedLaunch, Revision, resolve_launch,
};
use termirust_store::{PresetSnapshot, ProjectSnapshot};

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

#[derive(Clone)]
pub(super) struct ProjectCoordinator {
    resolver: Arc<dyn ProjectLaunchResolver>,
}

impl Default for ProjectCoordinator {
    fn default() -> Self {
        Self {
            resolver: Arc::new(SystemProjectLaunchResolver),
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
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use termirust_domain::{
        CanonicalPath, HostedSessionId, LaunchPreset, LaunchResolutionError, LocalizedUserText,
        PermissionPolicy, PositionKey, PresetDraft, PresetId, PresetOrigin, Project, ProjectId,
        ResolvedLaunch, Revision, WorkingDirectoryRule,
    };
    use termirust_store::{Durability, PresetSnapshot, ProjectSnapshot, StoreHealth};

    use super::{
        ProjectCoordinator, ProjectLaunchResolver, ProjectLaunchReviewError,
        ProjectLaunchReviewInput, ReviewedProjectLaunch,
    };

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
        }
    }
}
