use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ActivityState, Group, HostedSession, HostedSessionState, LaunchPreset,
    MAX_SESSIONS_PER_PROJECT, PositionKey, Project, ProjectId, Revision,
};

pub const DERIVED_INDEX_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexSourceRevisions {
    pub projects: Revision,
    pub sessions: Revision,
    pub presets: Revision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSessionIndex {
    pub version: u16,
    pub source_revisions: IndexSourceRevisions,
    pub projects: Vec<ProjectSessionIndexEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSessionIndexEntry {
    pub project_id: ProjectId,
    pub position: PositionKey,
    pub session_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteDocumentKind {
    Project,
    Group,
    Preset,
    Session,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteDocumentStatus {
    Attention,
    Busy,
    Done,
    Running,
    Idle,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteIndex {
    pub version: u16,
    pub source_revisions: IndexSourceRevisions,
    pub documents: Vec<PaletteIndexDocument>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteIndexDocument {
    pub kind: PaletteDocumentKind,
    pub id: String,
    pub title: String,
    pub project_id: Option<ProjectId>,
    pub project_label: Option<String>,
    pub group_label: Option<String>,
    pub preset_label: Option<String>,
    pub runtime_label: Option<String>,
    pub status: PaletteDocumentStatus,
    pub pinned: bool,
    pub archived: bool,
    pub position: PositionKey,
    pub meaningful_activity_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexBuildError {
    DuplicateProject,
    DuplicateDocument,
    OrphanedGroup,
    OrphanedSession,
    SessionLimit,
}

impl fmt::Display for IndexBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "derived index build failed: {self:?}")
    }
}

impl std::error::Error for IndexBuildError {}

pub fn build_project_session_index(
    source_revisions: IndexSourceRevisions,
    projects: &[Project],
    sessions: &[HostedSession],
) -> Result<ProjectSessionIndex, IndexBuildError> {
    let mut ordered_projects = projects.iter().collect::<Vec<_>>();
    ordered_projects.sort_by_key(|project| (project.position, project.id));
    let mut project_ids = HashSet::with_capacity(ordered_projects.len());
    let mut sessions_by_project = HashMap::<ProjectId, Vec<&HostedSession>>::new();
    for project in &ordered_projects {
        if !project_ids.insert(project.id) {
            return Err(IndexBuildError::DuplicateProject);
        }
    }
    for session in sessions {
        if !project_ids.contains(&session.project_id) {
            return Err(IndexBuildError::OrphanedSession);
        }
        let entries = sessions_by_project.entry(session.project_id).or_default();
        if entries.len() >= MAX_SESSIONS_PER_PROJECT {
            return Err(IndexBuildError::SessionLimit);
        }
        entries.push(session);
    }
    let projects = ordered_projects
        .into_iter()
        .map(|project| {
            let mut sessions = sessions_by_project.remove(&project.id).unwrap_or_default();
            sessions.sort_by_key(|session| (session.group_id, session.position, session.id));
            ProjectSessionIndexEntry {
                project_id: project.id,
                position: project.position,
                session_ids: sessions
                    .into_iter()
                    .map(|session| session.id.to_string())
                    .collect(),
            }
        })
        .collect();
    Ok(ProjectSessionIndex {
        version: DERIVED_INDEX_VERSION,
        source_revisions,
        projects,
    })
}

pub fn build_palette_index(
    source_revisions: IndexSourceRevisions,
    projects: &[Project],
    groups: &[Group],
    presets: &[LaunchPreset],
    sessions: &[HostedSession],
) -> Result<PaletteIndex, IndexBuildError> {
    let project_labels = projects
        .iter()
        .map(|project| (project.id, project.display_name.as_str().to_string()))
        .collect::<HashMap<_, _>>();
    if project_labels.len() != projects.len() {
        return Err(IndexBuildError::DuplicateProject);
    }
    let group_labels = groups
        .iter()
        .map(|group| (group.id, group.name.as_str().to_string()))
        .collect::<HashMap<_, _>>();
    if groups
        .iter()
        .any(|group| !project_labels.contains_key(&group.project_id))
    {
        return Err(IndexBuildError::OrphanedGroup);
    }
    let preset_labels = presets
        .iter()
        .map(|preset| {
            (
                preset.id,
                (
                    preset.label.as_str().to_string(),
                    preset
                        .runtime
                        .as_ref()
                        .map(|runtime| runtime.as_str().to_string()),
                ),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut documents =
        Vec::with_capacity(projects.len() + groups.len() + presets.len() + sessions.len());
    for project in projects {
        let title = project.display_name.as_str().to_string();
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Project,
            id: project.id.to_string(),
            title: title.clone(),
            project_id: Some(project.id),
            project_label: Some(title),
            group_label: None,
            preset_label: None,
            runtime_label: None,
            status: PaletteDocumentStatus::Unknown,
            pinned: false,
            archived: false,
            position: project.position,
            meaningful_activity_at: 0,
        });
    }
    for group in groups {
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Group,
            id: group.id.to_string(),
            title: group.name.as_str().to_string(),
            project_id: Some(group.project_id),
            project_label: project_labels.get(&group.project_id).cloned(),
            group_label: Some(group.name.as_str().to_string()),
            preset_label: None,
            runtime_label: None,
            status: PaletteDocumentStatus::Unknown,
            pinned: false,
            archived: false,
            position: group.position,
            meaningful_activity_at: 0,
        });
    }
    for preset in presets {
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Preset,
            id: preset.id.to_string(),
            title: preset.label.as_str().to_string(),
            project_id: None,
            project_label: None,
            group_label: None,
            preset_label: Some(preset.label.as_str().to_string()),
            runtime_label: preset
                .runtime
                .as_ref()
                .map(|runtime| runtime.as_str().to_string()),
            status: if preset.enabled {
                PaletteDocumentStatus::Unknown
            } else {
                PaletteDocumentStatus::Unavailable
            },
            pinned: preset.favorite,
            archived: false,
            position: preset.position,
            meaningful_activity_at: 0,
        });
    }
    for session in sessions {
        let Some(project_label) = project_labels.get(&session.project_id).cloned() else {
            return Err(IndexBuildError::OrphanedSession);
        };
        let (preset_label, runtime_label) = session
            .preset_id
            .and_then(|id| preset_labels.get(&id).cloned())
            .map(|(label, runtime)| (Some(label), runtime))
            .unwrap_or_default();
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Session,
            id: session.id.to_string(),
            title: session.title.as_str().to_string(),
            project_id: Some(session.project_id),
            project_label: Some(project_label),
            group_label: session
                .group_id
                .and_then(|id| group_labels.get(&id).cloned()),
            preset_label,
            runtime_label,
            status: session_status(session),
            pinned: session.pinned,
            archived: session.archived_at.is_some(),
            position: session.position,
            meaningful_activity_at: session.updated_at,
        });
    }
    documents.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.position.cmp(&right.position))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut ids = HashSet::with_capacity(documents.len());
    if documents
        .iter()
        .any(|document| !ids.insert((document.kind, document.id.as_str())))
    {
        return Err(IndexBuildError::DuplicateDocument);
    }
    Ok(PaletteIndex {
        version: DERIVED_INDEX_VERSION,
        source_revisions,
        documents,
    })
}

fn session_status(session: &HostedSession) -> PaletteDocumentStatus {
    match session.activity.state {
        ActivityState::NeedsInput => PaletteDocumentStatus::Attention,
        ActivityState::Busy => PaletteDocumentStatus::Busy,
        ActivityState::Done => PaletteDocumentStatus::Done,
        ActivityState::Failed => PaletteDocumentStatus::Unavailable,
        ActivityState::Idle => PaletteDocumentStatus::Idle,
        ActivityState::Unknown if session.lifecycle.is_running() => PaletteDocumentStatus::Running,
        ActivityState::Unknown => match session.lifecycle {
            HostedSessionState::Starting
            | HostedSessionState::Validating
            | HostedSessionState::Draft => PaletteDocumentStatus::Idle,
            HostedSessionState::Exited => PaletteDocumentStatus::Done,
            HostedSessionState::Failed
            | HostedSessionState::Cancelled
            | HostedSessionState::Offline
            | HostedSessionState::Orphaned
            | HostedSessionState::Gap
            | HostedSessionState::PermissionDenied
            | HostedSessionState::Incompatible => PaletteDocumentStatus::Unavailable,
            HostedSessionState::Provisioning
            | HostedSessionState::Attaching
            | HostedSessionState::Replaying
            | HostedSessionState::Live
            | HostedSessionState::RecordingPaused
            | HostedSessionState::Stopping
            | HostedSessionState::RunningAppAttached => PaletteDocumentStatus::Running,
        },
    }
}
