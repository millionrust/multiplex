use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ActivityState, Group, HostedSession, HostedSessionState, LaunchPreset, PositionKey, Revision,
};

pub const DERIVED_INDEX_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexSourceRevisions {
    pub sessions: Revision,
    pub presets: Revision,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteDocumentKind {
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

pub fn build_palette_index(
    source_revisions: IndexSourceRevisions,
    groups: &[Group],
    presets: &[LaunchPreset],
    sessions: &[HostedSession],
) -> Result<PaletteIndex, IndexBuildError> {
    let group_labels = groups
        .iter()
        .map(|group| (group.id, group.name.as_str().to_string()))
        .collect::<HashMap<_, _>>();
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

    let mut documents = Vec::with_capacity(groups.len() + presets.len() + sessions.len());
    for group in groups {
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Group,
            id: group.id.to_string(),
            title: group.name.as_str().to_string(),
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
        let (preset_label, runtime_label) = session
            .preset_id
            .and_then(|id| preset_labels.get(&id).cloned())
            .map(|(label, runtime)| (Some(label), runtime))
            .unwrap_or_default();
        documents.push(PaletteIndexDocument {
            kind: PaletteDocumentKind::Session,
            id: session.id.to_string(),
            title: session.title.as_str().to_string(),
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
