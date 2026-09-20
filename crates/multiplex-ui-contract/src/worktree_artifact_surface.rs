use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_WORKTREE_ARTIFACT_ROWS: usize = 2_048;
pub const MAX_WORKTREE_ARTIFACT_CONTROLS: usize = 4_096;
const MAX_WORKTREE_ARTIFACT_NODES: usize = 8_192;
pub const WORKTREE_ARTIFACT_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const ROOT_NODE: u64 = 130_000;
const STATUS_NODE: u64 = 130_001;
const PROGRESS_NODE: u64 = 130_002;
const LIST_NODE: u64 = 130_003;
const ROW_NODE_BASE: u64 = 8_u64 << 60;
const CONTROL_NODE_BASE: u64 = 9_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WorktreeArtifactRowKind {
    Worktree,
    Session,
    Artifact,
    Evidence,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorktreeArtifactRowId {
    pub kind: WorktreeArtifactRowKind,
    pub owner: u128,
    pub value: u128,
}

impl WorktreeArtifactRowId {
    pub const fn worktree(value: u128) -> Self {
        Self {
            kind: WorktreeArtifactRowKind::Worktree,
            owner: 0,
            value,
        }
    }

    pub const fn session(value: u128) -> Self {
        Self {
            kind: WorktreeArtifactRowKind::Session,
            owner: 0,
            value,
        }
    }

    pub const fn artifact(session: u128, value: u128) -> Self {
        Self {
            kind: WorktreeArtifactRowKind::Artifact,
            owner: session,
            value,
        }
    }

    pub const fn evidence(worktree: u128, value: u128) -> Self {
        Self {
            kind: WorktreeArtifactRowKind::Evidence,
            owner: worktree,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorktreeArtifactScreen {
    WorktreeLaunch,
    ArtifactGallery,
}

impl WorktreeArtifactScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::WorktreeLaunch => MessageId::WorktreeTitle,
            Self::ArtifactGallery => MessageId::ArtifactGalleryTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorktreeArtifactSurfaceState {
    Ready,
    Loading,
    Empty,
    Inspecting,
    Creating,
    Verifying,
    Registered,
    Importing,
    Partial,
    Cancelled,
    Offline,
    PermissionDenied,
    Timeout,
    Malformed,
    Recovery,
    Unavailable,
    Corrupt,
    Unsupported,
    Quota,
    DiskNearFull,
    DiskFull,
    Quarantined,
    RiskReview,
    UnknownCompletion,
    Error,
}

impl WorktreeArtifactSurfaceState {
    pub const ALL: [Self; 25] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::Inspecting,
        Self::Creating,
        Self::Verifying,
        Self::Registered,
        Self::Importing,
        Self::Partial,
        Self::Cancelled,
        Self::Offline,
        Self::PermissionDenied,
        Self::Timeout,
        Self::Malformed,
        Self::Recovery,
        Self::Unavailable,
        Self::Corrupt,
        Self::Unsupported,
        Self::Quota,
        Self::DiskNearFull,
        Self::DiskFull,
        Self::Quarantined,
        Self::RiskReview,
        Self::UnknownCompletion,
        Self::Error,
    ];

    pub const fn message(self, screen: WorktreeArtifactScreen) -> MessageId {
        match (screen, self) {
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Ready) => MessageId::WorktreeStageReady,
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Inspecting | Self::Loading) => {
                MessageId::WorktreeStageInspecting
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Creating) => {
                MessageId::WorktreeStageCreating
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Verifying) => {
                MessageId::WorktreeStageVerifying
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Registered) => {
                MessageId::WorktreeStageRegistered
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Offline) => {
                MessageId::WorktreeOfflineStatus
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::PermissionDenied) => {
                MessageId::WorktreeErrorPermission
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Timeout) => {
                MessageId::WorktreeErrorTimeout
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Cancelled) => {
                MessageId::WorktreeErrorCancelled
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Recovery | Self::UnknownCompletion) => {
                MessageId::WorktreeRecoveryBanner
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::DiskFull) => {
                MessageId::WorktreeErrorStorageFull
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Quota) => {
                MessageId::WorktreeErrorResourceLimit
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::Unavailable) => {
                MessageId::WorktreeErrorGitUnavailable
            }
            (WorktreeArtifactScreen::WorktreeLaunch, Self::RiskReview) => {
                MessageId::WorktreeCurrentWarning
            }
            (WorktreeArtifactScreen::WorktreeLaunch, _) => MessageId::WorktreeErrorGeneric,
            (WorktreeArtifactScreen::ArtifactGallery, Self::Ready) => {
                MessageId::ArtifactGalleryDescription
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Loading) => {
                MessageId::ArtifactGalleryLoading
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Empty) => {
                MessageId::ArtifactGalleryEmpty
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Importing) => {
                MessageId::ArtifactOperationImporting
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Cancelled) => {
                MessageId::ArtifactErrorCancelled
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::PermissionDenied) => {
                MessageId::ArtifactErrorPermission
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Timeout) => {
                MessageId::ArtifactErrorTimeout
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Malformed | Self::Corrupt) => {
                MessageId::ArtifactErrorCorrupt
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Unsupported) => {
                MessageId::ArtifactPreviewMetadataOnly
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Quota | Self::DiskNearFull) => {
                MessageId::ArtifactErrorQuota
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::DiskFull) => {
                MessageId::ArtifactErrorStorageFull
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Quarantined) => {
                MessageId::ArtifactStateQuarantined
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Unavailable | Self::Offline) => {
                MessageId::ArtifactErrorUnavailable
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::RiskReview) => {
                MessageId::ArtifactPurgeWarning
            }
            (WorktreeArtifactScreen::ArtifactGallery, Self::Recovery | Self::UnknownCompletion) => {
                MessageId::ArtifactErrorSourceChanged
            }
            (WorktreeArtifactScreen::ArtifactGallery, _) => MessageId::ArtifactErrorDecode,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Loading | Self::Inspecting | Self::Creating | Self::Verifying | Self::Importing
        )
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::PermissionDenied
                | Self::Timeout
                | Self::Malformed
                | Self::Recovery
                | Self::Unavailable
                | Self::Corrupt
                | Self::DiskFull
                | Self::UnknownCompletion
                | Self::Error
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeArtifactRow {
    pub id: WorktreeArtifactRowId,
    pub parent: Option<WorktreeArtifactRowId>,
    pub name: String,
    pub status: MessageId,
    pub detail: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub expanded: Option<bool>,
    pub invalid: bool,
    pub stale: bool,
    pub position: usize,
    pub set_size: usize,
}

impl WorktreeArtifactRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > crate::MAX_SEMANTIC_TEXT_CHARS
            || self.name.chars().any(|character| character == '\0')
            || self.position == 0
            || self.set_size == 0
            || self.position > self.set_size
            || self.detail.as_ref().is_some_and(|detail| {
                detail.chars().count() > crate::MAX_SEMANTIC_TEXT_CHARS
                    || detail.chars().any(|character| character == '\0')
            })
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        if let Some(parent) = self.parent
            && parent == self.id
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorktreeArtifactAction {
    SetWorktreeBase,
    SetWorktreeBranch,
    ReviewWorktree,
    FetchWorktree,
    ConfirmCurrentBase,
    CreateWorktree,
    VerifyRecovery,
    ForgetRecovery,
    SelectPreset(u128),
    StartSession,
    CancelOrCloseWorktree,
    SelectArtifactSession(u128),
    ImportArtifact(u128),
    ShowArtifactList,
    ShowArtifactGrid,
    ConfirmArtifactImport,
    CancelArtifactImport,
    CancelArtifactOperation,
    PreviewArtifact(WorktreeArtifactRowId),
    ExportArtifact(WorktreeArtifactRowId),
    ToggleArtifactMetadata(WorktreeArtifactRowId),
    QuarantineArtifact(WorktreeArtifactRowId),
    RestoreArtifact(WorktreeArtifactRowId),
    RequestArtifactPurge(WorktreeArtifactRowId),
    ConfirmArtifactPurge(WorktreeArtifactRowId),
    CancelArtifactPurge,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorktreeArtifactControlRole {
    Button,
    TextField,
    RadioButton,
}

impl WorktreeArtifactControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField => SemanticRole::TextField,
            Self::RadioButton => SemanticRole::RadioButton,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeArtifactControl {
    pub action: WorktreeArtifactAction,
    pub parent: Option<WorktreeArtifactRowId>,
    pub role: WorktreeArtifactControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub invalid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorktreeArtifactProgress {
    pub label: MessageId,
    pub current: u64,
    pub maximum: Option<u64>,
    pub cancellable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorktreeArtifactTextScale(u16);

impl WorktreeArtifactTextScale {
    pub const fn try_new(percent: u16) -> Option<Self> {
        if percent >= 100 && percent <= 400 {
            Some(Self(percent))
        } else {
            None
        }
    }

    pub const fn layout(self) -> WorktreeArtifactResponsiveLayout {
        match self.0 {
            100..=150 => WorktreeArtifactResponsiveLayout::IndexAndDetail,
            151..=200 => WorktreeArtifactResponsiveLayout::Compact,
            _ => WorktreeArtifactResponsiveLayout::Stacked,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeArtifactResponsiveLayout {
    IndexAndDetail,
    Compact,
    Stacked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeArtifactSemanticSnapshot {
    pub screen: WorktreeArtifactScreen,
    pub state: WorktreeArtifactSurfaceState,
    pub rows: Vec<WorktreeArtifactRow>,
    pub controls: Vec<WorktreeArtifactControl>,
    pub progress: Option<WorktreeArtifactProgress>,
    pub recording_friendly: bool,
}

impl WorktreeArtifactSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_WORKTREE_ARTIFACT_ROWS
            || self.controls.len() > MAX_WORKTREE_ARTIFACT_CONTROLS
            || self.rows.len() + self.controls.len() + 4 > MAX_WORKTREE_ARTIFACT_NODES
        {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }

        let mut row_nodes = HashMap::with_capacity(self.rows.len());
        let mut semantic_ids = HashSet::with_capacity(self.rows.len() + self.controls.len());
        for row in &self.rows {
            row.validate()?;
            let node = worktree_artifact_row_semantic_node(row.id);
            if !semantic_ids.insert(node) || row_nodes.insert(row.id, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }

        let mut control_nodes = HashMap::with_capacity(self.controls.len());
        for control in &self.controls {
            if control.value.as_ref().is_some_and(|value| {
                value.chars().count() > crate::MAX_SEMANTIC_ACTION_VALUE_CHARS
                    || value.chars().any(|character| character == '\0')
            }) {
                return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
            }
            let node = worktree_artifact_control_semantic_node(control.action);
            if !semantic_ids.insert(node) || control_nodes.insert(control.action, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }

        let root_id = semantic_id(ROOT_NODE);
        let root_role = if self.screen == WorktreeArtifactScreen::WorktreeLaunch {
            SemanticRole::Dialog
        } else {
            SemanticRole::Landmark
        };
        let mut root = named_node(root_id, root_role, self.screen.title());
        root.parent = Some(parent);
        root.state.busy = self.state.is_busy();

        let status_id = semantic_id(STATUS_NODE);
        let mut status = named_node(
            status_id,
            if self.state.is_error() {
                SemanticRole::Alert
            } else {
                SemanticRole::Status
            },
            self.state.message(self.screen),
        );
        status.parent = Some(root_id);
        status.state.live = Some(if self.state.is_error() {
            LiveRegionPoliteness::Immediate
        } else {
            LiveRegionPoliteness::Polite
        });

        let list_id = semantic_id(LIST_NODE);
        let mut list = SemanticNode::new(list_id, SemanticRole::List);
        list.parent = Some(root_id);

        let mut nodes = vec![root, status, list];
        if let Some(progress) = self.progress {
            let progress_id = semantic_id(PROGRESS_NODE);
            let mut node = named_node(progress_id, SemanticRole::ProgressIndicator, progress.label);
            node.parent = Some(root_id);
            node.state.busy = true;
            node.value = progress.maximum.map(|maximum| SemanticValue::Number {
                current: i64::try_from(progress.current.min(maximum)).unwrap_or(i64::MAX),
                minimum: 0,
                maximum: i64::try_from(maximum).unwrap_or(i64::MAX),
            });
            if progress.cancellable {
                node.actions.insert(SemanticAction::Cancel);
            }
            nodes.push(node);
        }

        for row in &self.rows {
            let node_id = row_nodes[&row.id];
            let mut node = SemanticNode::new(node_id, SemanticRole::ListItem);
            node.parent = match row.parent {
                Some(parent_id) => Some(*row_nodes.get(&parent_id).ok_or_else(|| {
                    SemanticError::new(SemanticErrorCode::MissingParent, Some(node_id))
                })?),
                None => Some(list_id),
            };
            node.name = Some(if self.recording_friendly {
                SemanticText::Message(private_row_message(row.id.kind))
            } else {
                SemanticText::user_text(bidi_isolate(&row.name))?
            });
            node.description = Some(SemanticText::Message(row.status));
            node.value = row.detail.as_ref().map(|detail| {
                if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(private_row_message(
                        row.id.kind,
                    )))
                } else {
                    SemanticValue::PublicText(
                        SemanticText::user_text(bidi_isolate(detail))
                            .expect("validated row detail remains bounded"),
                    )
                }
            });
            if node.value.is_none() {
                node.value = Some(SemanticValue::Number {
                    current: row.position as i64,
                    minimum: 1,
                    maximum: row.set_size as i64,
                });
            }
            node.state = SemanticState {
                disabled: row.disabled,
                selected: row.selected,
                expanded: row.expanded,
                checked: None,
                invalid: row.invalid,
                busy: false,
                hidden: false,
                live: row.stale.then_some(LiveRegionPoliteness::Polite),
            };
            node.actions.insert(SemanticAction::Focus);
            if !row.disabled
                && matches!(
                    row.id.kind,
                    WorktreeArtifactRowKind::Session | WorktreeArtifactRowKind::Artifact
                )
            {
                node.actions.insert(SemanticAction::Activate);
            }
            nodes.push(node);
        }

        for control in &self.controls {
            let node_id = control_nodes[&control.action];
            let mut node = named_node(node_id, control.role.semantic_role(), control.name);
            node.parent = match control.parent {
                Some(parent_id) => Some(*row_nodes.get(&parent_id).ok_or_else(|| {
                    SemanticError::new(SemanticErrorCode::MissingParent, Some(node_id))
                })?),
                None => Some(root_id),
            };
            node.state.disabled = control.disabled;
            node.state.selected = control.selected;
            node.state.checked = (control.role == WorktreeArtifactControlRole::RadioButton)
                .then_some(control.selected);
            node.state.invalid = control.invalid;
            if let Some(value) = control.value.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(control.name))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(value))?)
                });
            }
            node.actions.insert(SemanticAction::Focus);
            if !control.disabled {
                node.actions.insert(match control.role {
                    WorktreeArtifactControlRole::TextField => SemanticAction::SetValue,
                    WorktreeArtifactControlRole::Button
                    | WorktreeArtifactControlRole::RadioButton => SemanticAction::Activate,
                });
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    pub fn routes(
        &self,
    ) -> Vec<(
        (SemanticNodeId, SemanticAction),
        WorktreeArtifactAccessibilityCommand,
    )> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2 + 1);
        for row in &self.rows {
            let node = worktree_artifact_row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                WorktreeArtifactAccessibilityCommand::FocusRow(row.id),
            ));
            if !row.disabled
                && matches!(
                    row.id.kind,
                    WorktreeArtifactRowKind::Session | WorktreeArtifactRowKind::Artifact
                )
            {
                routes.push((
                    (node, SemanticAction::Activate),
                    WorktreeArtifactAccessibilityCommand::ActivateRow(row.id),
                ));
            }
        }
        for control in &self.controls {
            let node = worktree_artifact_control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                WorktreeArtifactAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                let (semantic_action, command) = match control.role {
                    WorktreeArtifactControlRole::TextField => (
                        SemanticAction::SetValue,
                        WorktreeArtifactAccessibilityCommand::SetControlValue(control.action),
                    ),
                    WorktreeArtifactControlRole::Button
                    | WorktreeArtifactControlRole::RadioButton => (
                        SemanticAction::Activate,
                        WorktreeArtifactAccessibilityCommand::ActivateControl(control.action),
                    ),
                };
                routes.push(((node, semantic_action), command));
            }
        }
        if self.progress.is_some_and(|progress| progress.cancellable) {
            routes.push((
                (semantic_id(PROGRESS_NODE), SemanticAction::Cancel),
                WorktreeArtifactAccessibilityCommand::CancelProgress,
            ));
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeArtifactAccessibilityCommand {
    FocusRow(WorktreeArtifactRowId),
    ActivateRow(WorktreeArtifactRowId),
    FocusControl(WorktreeArtifactAction),
    SetControlValue(WorktreeArtifactAction),
    ActivateControl(WorktreeArtifactAction),
    CancelProgress,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorktreeArtifactSelectionResult {
    pub selected: Option<WorktreeArtifactRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_worktree_artifact_selection(
    previous: &[WorktreeArtifactRowId],
    next: &[WorktreeArtifactRowId],
    selected: Option<WorktreeArtifactRowId>,
) -> WorktreeArtifactSelectionResult {
    if let Some(selected) = selected
        && next.contains(&selected)
    {
        return WorktreeArtifactSelectionResult {
            selected: Some(selected),
            focus_heading: false,
        };
    }
    let prior_index = selected
        .and_then(|selected| previous.iter().position(|candidate| *candidate == selected))
        .unwrap_or(0);
    WorktreeArtifactSelectionResult {
        selected: next
            .get(prior_index.min(next.len().saturating_sub(1)))
            .copied(),
        focus_heading: next.is_empty(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorktreeArtifactAnnouncementCoalescer {
    last_announcement: Option<Instant>,
    pending: bool,
}

impl WorktreeArtifactAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last_announcement.is_none_or(|previous| {
                now.saturating_duration_since(previous) >= WORKTREE_ARTIFACT_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last_announcement = Some(now);
            self.pending = false;
            return true;
        }
        self.pending = true;
        false
    }

    pub fn flush(&mut self, now: Instant) -> bool {
        if self.pending
            && self.last_announcement.is_none_or(|previous| {
                now.saturating_duration_since(previous) >= WORKTREE_ARTIFACT_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last_announcement = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn worktree_artifact_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}

pub fn worktree_artifact_row_semantic_node(id: WorktreeArtifactRowId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE | (stable_hash(&id) & NODE_MASK))
}

pub fn worktree_artifact_control_semantic_node(action: WorktreeArtifactAction) -> SemanticNodeId {
    semantic_id(CONTROL_NODE_BASE | (stable_hash(&action) & NODE_MASK))
}

fn private_row_message(kind: WorktreeArtifactRowKind) -> MessageId {
    match kind {
        WorktreeArtifactRowKind::Worktree | WorktreeArtifactRowKind::Evidence => {
            MessageId::WorktreePrivateReference
        }
        WorktreeArtifactRowKind::Session => MessageId::ProductPrivateSessionRow,
        WorktreeArtifactRowKind::Artifact => MessageId::ArtifactPrivateRow,
    }
}

fn named_node(id: SemanticNodeId, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(id, role);
    node.name = Some(SemanticText::Message(name));
    node
}

fn bidi_isolate(value: &str) -> String {
    format!("\u{2068}{value}\u{2069}")
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("semantic node IDs are non-zero"))
}

fn stable_hash(value: &impl Hash) -> u64 {
    let mut hasher = StableHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignTokens, Locale, Localizer, ThemeKind};

    fn artifact_row(name: &str) -> WorktreeArtifactRow {
        WorktreeArtifactRow {
            id: WorktreeArtifactRowId::artifact(7, 11),
            parent: None,
            name: name.to_string(),
            status: MessageId::ArtifactStateReady,
            detail: Some("text/plain, 4 bytes, explicit import".to_string()),
            selected: true,
            disabled: false,
            expanded: Some(false),
            invalid: false,
            stale: false,
            position: 1,
            set_size: 1,
        }
    }

    fn snapshot() -> WorktreeArtifactSemanticSnapshot {
        let row = artifact_row("report.txt");
        WorktreeArtifactSemanticSnapshot {
            screen: WorktreeArtifactScreen::ArtifactGallery,
            state: WorktreeArtifactSurfaceState::Ready,
            rows: vec![row.clone()],
            controls: vec![WorktreeArtifactControl {
                action: WorktreeArtifactAction::PreviewArtifact(row.id),
                parent: Some(row.id),
                role: WorktreeArtifactControlRole::Button,
                name: MessageId::ArtifactPreviewAction,
                value: None,
                selected: false,
                disabled: false,
                invalid: false,
            }],
            progress: None,
            recording_friendly: false,
        }
    }

    #[test]
    fn worktree_artifact_surface_preserves_typed_rows_and_actions() {
        let snapshot = snapshot();
        let row = snapshot.rows[0].id;
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        assert!(nodes.iter().any(|node| {
            node.id == worktree_artifact_row_semantic_node(row) && node.state.selected
        }));
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command
                == WorktreeArtifactAccessibilityCommand::ActivateControl(
                    WorktreeArtifactAction::PreviewArtifact(row),
                )
        }));
    }

    #[test]
    fn worktree_progress_has_exact_cancel_boundary() {
        let mut snapshot = snapshot();
        snapshot.screen = WorktreeArtifactScreen::WorktreeLaunch;
        snapshot.state = WorktreeArtifactSurfaceState::Creating;
        snapshot.progress = Some(WorktreeArtifactProgress {
            label: MessageId::WorktreeStageCreating,
            current: 2,
            maximum: Some(4),
            cancellable: true,
        });
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        let progress = nodes
            .iter()
            .find(|node| node.id == semantic_id(PROGRESS_NODE))
            .unwrap();
        assert!(progress.actions.contains(&SemanticAction::Cancel));
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command == WorktreeArtifactAccessibilityCommand::CancelProgress
        }));

        snapshot.progress.as_mut().unwrap().cancellable = false;
        assert!(!snapshot.routes().iter().any(|(_, command)| {
            *command == WorktreeArtifactAccessibilityCommand::CancelProgress
        }));
    }

    #[test]
    fn hostile_names_are_isolated_and_preview_bytes_never_enter_semantics() {
        let hostile = "\u{202e}$(touch should-not-run).html";
        let mut snapshot = snapshot();
        snapshot.rows[0].name = hostile.to_string();
        snapshot.rows[0].detail = Some("text/html, metadata only, explicit import".to_string());
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        let row = nodes
            .iter()
            .find(|node| node.id == worktree_artifact_row_semantic_node(snapshot.rows[0].id))
            .unwrap();
        assert_eq!(
            row.name,
            Some(SemanticText::UserText(format!("\u{2068}{hostile}\u{2069}")))
        );
        let rendered = format!("{nodes:?}");
        assert!(!rendered.contains("<script>preview-canary</script>"));

        snapshot.recording_friendly = true;
        let masked = format!("{:?}", snapshot.try_nodes(semantic_id(9)).unwrap());
        assert!(!masked.contains("touch should-not-run"));
    }

    #[test]
    fn every_state_localizes_for_required_locales_and_themes() {
        for locale in [Locale::EnUs, Locale::EnXa, Locale::ArXb] {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            for screen in [
                WorktreeArtifactScreen::WorktreeLaunch,
                WorktreeArtifactScreen::ArtifactGallery,
            ] {
                for state in WorktreeArtifactSurfaceState::ALL {
                    let message = localizer
                        .format_static(state.message(screen))
                        .expect("surface state localizes");
                    assert!(!message.trim().is_empty());
                }
            }
        }
        for theme in ThemeKind::ALL {
            let tokens = DesignTokens::new(theme);
            assert!(tokens.color_bg_canvas().alpha > 0);
            assert!(tokens.focus_ring_width().0 >= 2.0);
        }
    }

    #[test]
    fn scale_reflows_and_selection_reconciles_without_focus_loss() {
        assert_eq!(
            WorktreeArtifactTextScale::try_new(100).unwrap().layout(),
            WorktreeArtifactResponsiveLayout::IndexAndDetail
        );
        assert_eq!(
            WorktreeArtifactTextScale::try_new(200).unwrap().layout(),
            WorktreeArtifactResponsiveLayout::Compact
        );
        assert_eq!(
            WorktreeArtifactTextScale::try_new(400).unwrap().layout(),
            WorktreeArtifactResponsiveLayout::Stacked
        );
        assert!(WorktreeArtifactTextScale::try_new(99).is_none());
        assert!(WorktreeArtifactTextScale::try_new(401).is_none());

        let first = WorktreeArtifactRowId::artifact(1, 1);
        let second = WorktreeArtifactRowId::artifact(1, 2);
        let result =
            reconcile_worktree_artifact_selection(&[first, second], &[second], Some(first));
        assert_eq!(result.selected, Some(second));
        assert!(!result.focus_heading);
        let empty = reconcile_worktree_artifact_selection(&[second], &[], Some(second));
        assert!(empty.selected.is_none());
        assert!(empty.focus_heading);
    }

    #[test]
    fn invalid_parent_and_resource_limit_are_rejected() {
        let mut invalid = snapshot();
        invalid.rows[0].parent = Some(WorktreeArtifactRowId::artifact(9, 99));
        assert_eq!(
            invalid.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::MissingParent
        );

        let mut oversized = snapshot();
        oversized.rows = (0..=MAX_WORKTREE_ARTIFACT_ROWS)
            .map(|index| WorktreeArtifactRow {
                id: WorktreeArtifactRowId::artifact(1, index as u128 + 1),
                ..artifact_row("bounded")
            })
            .collect();
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn announcements_are_coalesced_but_final_state_is_immediate() {
        let start = Instant::now();
        let mut coalescer = WorktreeArtifactAnnouncementCoalescer::default();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(!coalescer.flush(start + Duration::from_millis(100)));
        assert!(coalescer.flush(start + Duration::from_millis(260)));
        assert!(coalescer.record_change(start + Duration::from_millis(270), true));
    }
}
