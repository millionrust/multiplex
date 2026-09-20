use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_SFTP_ROWS: usize = 2_048;
pub const MAX_SFTP_CONTROLS: usize = 4_096;
pub const SFTP_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);
const ROOT_NODE: u64 = 160_000;
const STATUS_NODE: u64 = 160_001;
const LIST_NODE: u64 = 160_002;
const ROW_NODE_BASE: u64 = 12_u64 << 60;
const CONTROL_NODE_BASE: u64 = 13_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SftpRowKind {
    LocalEntry,
    Host,
    RemoteEntry,
    Transfer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SftpRowId {
    pub kind: SftpRowKind,
    pub owner: u128,
    pub value: u128,
}

impl SftpRowId {
    pub const fn local(value: u128) -> Self {
        Self {
            kind: SftpRowKind::LocalEntry,
            owner: 0,
            value,
        }
    }

    pub const fn host(value: u128) -> Self {
        Self {
            kind: SftpRowKind::Host,
            owner: 0,
            value,
        }
    }

    pub const fn remote(workspace_id: u64, value: u128) -> Self {
        Self {
            kind: SftpRowKind::RemoteEntry,
            owner: workspace_id as u128,
            value,
        }
    }

    pub const fn transfer(workspace_id: u64, operation_id: u64) -> Self {
        Self {
            kind: SftpRowKind::Transfer,
            owner: workspace_id as u128,
            value: operation_id as u128,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SftpScreen {
    Library,
    HostPicker,
    Workspace,
}

impl SftpScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::Library => MessageId::SftpLibraryTitle,
            Self::HostPicker => MessageId::SftpHostPickerTitle,
            Self::Workspace => MessageId::SftpWorkspaceTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SftpSurfaceState {
    Ready,
    Loading,
    Empty,
    FilterEmpty,
    HostRequired,
    LocalUnavailable,
    Disconnected,
    PermissionDenied,
    Offline,
    Stale,
    Queued,
    Transferring,
    Conflict,
    CancelRequested,
    Cancelled,
    Partial,
    Completed,
    DiskFull,
    ResourceLimit,
    Timeout,
    Error,
}

impl SftpSurfaceState {
    pub const ALL: [Self; 21] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::FilterEmpty,
        Self::HostRequired,
        Self::LocalUnavailable,
        Self::Disconnected,
        Self::PermissionDenied,
        Self::Offline,
        Self::Stale,
        Self::Queued,
        Self::Transferring,
        Self::Conflict,
        Self::CancelRequested,
        Self::Cancelled,
        Self::Partial,
        Self::Completed,
        Self::DiskFull,
        Self::ResourceLimit,
        Self::Timeout,
        Self::Error,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::SftpStateReady,
            Self::Loading => MessageId::SftpStateLoading,
            Self::Empty => MessageId::SftpStateEmpty,
            Self::FilterEmpty => MessageId::SftpStateFilterEmpty,
            Self::HostRequired => MessageId::SftpStateHostRequired,
            Self::LocalUnavailable => MessageId::SftpStateLocalUnavailable,
            Self::Disconnected => MessageId::SftpStateDisconnected,
            Self::PermissionDenied => MessageId::SftpStatePermission,
            Self::Offline => MessageId::SftpStateOffline,
            Self::Stale => MessageId::SftpStateStale,
            Self::Queued => MessageId::SftpStateQueued,
            Self::Transferring => MessageId::SftpStateTransferring,
            Self::Conflict => MessageId::SftpStateConflict,
            Self::CancelRequested => MessageId::SftpStateCancelRequested,
            Self::Cancelled => MessageId::SftpStateCancelled,
            Self::Partial => MessageId::SftpStatePartial,
            Self::Completed => MessageId::SftpStateCompleted,
            Self::DiskFull => MessageId::SftpStateDiskFull,
            Self::ResourceLimit => MessageId::SftpStateLimit,
            Self::Timeout => MessageId::SftpStateTimeout,
            Self::Error => MessageId::SftpStateError,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Loading | Self::Queued | Self::Transferring | Self::CancelRequested
        )
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::Disconnected
                | Self::PermissionDenied
                | Self::Offline
                | Self::Stale
                | Self::DiskFull
                | Self::ResourceLimit
                | Self::Timeout
                | Self::Error
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SftpRow {
    pub id: SftpRowId,
    pub name: String,
    pub detail: Option<String>,
    pub status: MessageId,
    pub selected: bool,
    pub disabled: bool,
    pub activatable: bool,
    pub stale: bool,
    pub position: usize,
    pub set_size: usize,
}

impl SftpRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if !valid_user_text(&self.name)
            || self
                .detail
                .as_ref()
                .is_some_and(|value| !valid_user_text(value))
            || self.position == 0
            || self.set_size == 0
            || self.position > self.set_size
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SftpConflictChoice {
    Replace,
    Skip,
    Resume,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SftpAction {
    ToggleLocalFilter,
    SetLocalFilter,
    OpenLocalFolder,
    NavigateLocalParent,
    ShowHostPicker,
    CloseHostPicker,
    ConnectHost(SftpRowId),
    OpenWorkspaceFiles(u64),
    BackToTerminal(u64),
    NavigateRemoteParent(u64),
    RefreshRemote(u64),
    Upload(u64),
    Download(u64),
    Delete(u64),
    SelectEntry(SftpRowId),
    OpenEntry(SftpRowId),
    CancelTransfer {
        workspace_id: u64,
        operation_id: u64,
    },
    RetryTransfer {
        workspace_id: u64,
        operation_id: u64,
    },
    ResolveConflict {
        workspace_id: u64,
        operation_id: u64,
        choice: SftpConflictChoice,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SftpControlRole {
    Button,
    TextField,
}

impl SftpControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField => SemanticRole::TextField,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SftpControl {
    pub action: SftpAction,
    pub parent: Option<SftpRowId>,
    pub role: SftpControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub invalid: bool,
}

impl SftpControl {
    fn validate(&self) -> Result<(), SemanticError> {
        if self
            .value
            .as_ref()
            .is_some_and(|value| !valid_user_text(value))
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SftpSemanticSnapshot {
    pub screen: SftpScreen,
    pub state: SftpSurfaceState,
    pub rows: Vec<SftpRow>,
    pub controls: Vec<SftpControl>,
    pub recording_friendly: bool,
}

impl SftpSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_SFTP_ROWS || self.controls.len() > MAX_SFTP_CONTROLS {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        let mut row_ids = HashSet::with_capacity(self.rows.len());
        for row in &self.rows {
            row.validate()?;
            if !row_ids.insert(row.id) {
                return Err(SemanticError::new(SemanticErrorCode::DuplicateNode, None));
            }
        }
        let mut actions = HashSet::with_capacity(self.controls.len());
        for control in &self.controls {
            control.validate()?;
            if !actions.insert(control.action)
                || control.parent.is_some_and(|row| !row_ids.contains(&row))
            {
                return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
            }
        }

        let root_id = semantic_id(ROOT_NODE);
        let mut root = named_node(root_id, SemanticRole::Landmark, self.screen.title());
        root.parent = Some(parent);
        root.state.busy = self.state.is_busy();

        let mut status = named_node(
            semantic_id(STATUS_NODE),
            if self.state.is_error() {
                SemanticRole::Alert
            } else {
                SemanticRole::Status
            },
            self.state.message(),
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

        for row in &self.rows {
            let mut node =
                SemanticNode::new(sftp_row_semantic_node(row.id), SemanticRole::ListItem);
            node.parent = Some(list_id);
            node.name = Some(if self.recording_friendly {
                SemanticText::Message(private_row_message(row.id.kind))
            } else {
                SemanticText::user_text(bidi_isolate(&row.name))?
            });
            node.description = Some(SemanticText::Message(row.status));
            node.value = row.detail.as_ref().map(|detail| {
                if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(MessageId::SftpPrivatePath))
                } else {
                    SemanticValue::PublicText(
                        SemanticText::user_text(bidi_isolate(detail))
                            .expect("validated SFTP detail remains safe"),
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
                expanded: None,
                checked: None,
                invalid: false,
                busy: row.id.kind == SftpRowKind::Transfer && self.state.is_busy(),
                hidden: false,
                live: row.stale.then_some(LiveRegionPoliteness::Polite),
            };
            node.actions.insert(SemanticAction::Focus);
            if row.activatable && !row.disabled {
                node.actions.insert(SemanticAction::Activate);
            }
            nodes.push(node);
        }

        for control in &self.controls {
            let mut node = named_node(
                sftp_control_semantic_node(control.action),
                control.role.semantic_role(),
                control.name,
            );
            node.parent = control.parent.map(sftp_row_semantic_node).or(Some(root_id));
            node.state.disabled = control.disabled;
            node.state.selected = control.selected;
            node.state.invalid = control.invalid;
            if let Some(value) = control.value.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(MessageId::SftpPrivatePath))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(value))?)
                });
            }
            node.actions.insert(SemanticAction::Focus);
            if !control.disabled {
                node.actions
                    .insert(if control.role == SftpControlRole::TextField {
                        SemanticAction::SetValue
                    } else {
                        SemanticAction::Activate
                    });
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    pub fn routes(&self) -> Vec<((SemanticNodeId, SemanticAction), SftpAccessibilityCommand)> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2);
        for row in &self.rows {
            let node = sftp_row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                SftpAccessibilityCommand::FocusRow(row.id),
            ));
            if row.activatable && !row.disabled {
                routes.push((
                    (node, SemanticAction::Activate),
                    SftpAccessibilityCommand::ActivateRow(row.id),
                ));
            }
        }
        for control in &self.controls {
            let node = sftp_control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                SftpAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                routes.push((
                    (
                        node,
                        if control.role == SftpControlRole::TextField {
                            SemanticAction::SetValue
                        } else {
                            SemanticAction::Activate
                        },
                    ),
                    if control.role == SftpControlRole::TextField {
                        SftpAccessibilityCommand::SetControlValue(control.action)
                    } else {
                        SftpAccessibilityCommand::ActivateControl(control.action)
                    },
                ));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpAccessibilityCommand {
    FocusRow(SftpRowId),
    ActivateRow(SftpRowId),
    FocusControl(SftpAction),
    SetControlValue(SftpAction),
    ActivateControl(SftpAction),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpResponsiveLayout {
    DualPane,
    Compact,
    Stacked,
}

pub const fn sftp_responsive_layout(scale_percent: u16) -> Option<SftpResponsiveLayout> {
    match scale_percent {
        100..=150 => Some(SftpResponsiveLayout::DualPane),
        151..=200 => Some(SftpResponsiveLayout::Compact),
        201..=400 => Some(SftpResponsiveLayout::Stacked),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SftpSelectionResult {
    pub selected: Option<SftpRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_sftp_selection(
    previous: &[SftpRowId],
    next: &[SftpRowId],
    selected: Option<SftpRowId>,
) -> SftpSelectionResult {
    if let Some(selected) = selected
        && next.contains(&selected)
    {
        return SftpSelectionResult {
            selected: Some(selected),
            focus_heading: false,
        };
    }
    let prior_index = selected
        .and_then(|selected| previous.iter().position(|row| *row == selected))
        .unwrap_or(0);
    let replacement = selected.and_then(|selected| {
        next.iter()
            .enumerate()
            .filter(|(_, row)| row.kind == selected.kind && row.owner == selected.owner)
            .min_by_key(|(index, _)| index.abs_diff(prior_index))
            .map(|(_, row)| *row)
    });
    SftpSelectionResult {
        selected: replacement,
        focus_heading: replacement.is_none(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct SftpAnnouncementCoalescer {
    last: Option<Instant>,
    pending: bool,
}

impl SftpAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= SFTP_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            return true;
        }
        self.pending = true;
        false
    }

    pub fn flush(&mut self, now: Instant) -> bool {
        if self.pending
            && self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= SFTP_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn stable_sftp_value(value: &str) -> u128 {
    u128::from(stable_hash(&value))
}

pub fn sftp_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}

pub fn sftp_row_semantic_node(id: SftpRowId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE | (stable_hash(&id) & NODE_MASK))
}

pub fn sftp_control_semantic_node(action: SftpAction) -> SemanticNodeId {
    semantic_id(CONTROL_NODE_BASE | (stable_hash(&action) & NODE_MASK))
}

fn private_row_message(kind: SftpRowKind) -> MessageId {
    match kind {
        SftpRowKind::Host => MessageId::SftpPrivateHost,
        SftpRowKind::LocalEntry | SftpRowKind::RemoteEntry => MessageId::SftpPrivateEntry,
        SftpRowKind::Transfer => MessageId::SftpStateTransferring,
    }
}

fn valid_user_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= crate::MAX_SEMANTIC_TEXT_CHARS
        && !value.chars().any(|character| character == '\0')
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

    fn snapshot() -> SftpSemanticSnapshot {
        let row = SftpRow {
            id: SftpRowId::remote(7, stable_sftp_value("/srv/canary.txt")),
            name: "canary.txt".to_string(),
            detail: Some("/srv/canary.txt".to_string()),
            status: MessageId::SftpRowRemoteFile,
            selected: true,
            disabled: false,
            activatable: true,
            stale: false,
            position: 1,
            set_size: 1,
        };
        SftpSemanticSnapshot {
            screen: SftpScreen::Workspace,
            state: SftpSurfaceState::Ready,
            rows: vec![row.clone()],
            controls: vec![SftpControl {
                action: SftpAction::SelectEntry(row.id),
                parent: Some(row.id),
                role: SftpControlRole::Button,
                name: MessageId::SftpSelectEntryAction,
                value: None,
                selected: true,
                disabled: false,
                invalid: false,
            }],
            recording_friendly: false,
        }
    }

    #[test]
    fn rows_and_exact_actions_have_stable_routes() {
        let snapshot = snapshot();
        let row = snapshot.rows[0].id;
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node.id == sftp_row_semantic_node(row))
        );
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command == SftpAccessibilityCommand::ActivateControl(SftpAction::SelectEntry(row))
        }));
    }

    #[test]
    fn recording_mode_masks_names_paths_and_filter_values() {
        let mut snapshot = snapshot();
        snapshot.recording_friendly = true;
        snapshot.controls.push(SftpControl {
            action: SftpAction::SetLocalFilter,
            parent: None,
            role: SftpControlRole::TextField,
            name: MessageId::SftpFilterField,
            value: Some("secret-filter-canary".to_string()),
            selected: false,
            disabled: false,
            invalid: false,
        });
        let rendered = format!("{:?}", snapshot.try_nodes(semantic_id(9)).unwrap());
        assert!(!rendered.contains("canary.txt"));
        assert!(!rendered.contains("/srv/canary.txt"));
        assert!(!rendered.contains("secret-filter-canary"));
    }

    #[test]
    fn every_state_locale_theme_and_scale_is_defined() {
        for state in SftpSurfaceState::ALL {
            assert!(MessageId::ALL.contains(&state.message()));
        }
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            assert!(
                !localizer
                    .format_static(MessageId::SftpStateConflict)
                    .unwrap()
                    .is_empty()
            );
        }
        for theme in ThemeKind::ALL {
            assert!(DesignTokens::new(theme).focus_ring_width().0 >= 2.0);
        }
        for scale in [100, 150, 200, 300, 400] {
            assert!(sftp_responsive_layout(scale).is_some());
        }
        assert!(sftp_responsive_layout(99).is_none());
        assert!(sftp_responsive_layout(401).is_none());
    }

    #[test]
    fn malformed_and_oversized_snapshots_fail_closed() {
        let mut malformed = snapshot();
        malformed.rows[0].name = "\0".to_string();
        assert_eq!(
            malformed.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );
        let row = snapshot().rows[0].clone();
        let mut oversized = snapshot();
        oversized.rows = vec![row; MAX_SFTP_ROWS + 1];
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn selection_reconciles_without_retargeting_another_owner() {
        let first = SftpRowId::remote(1, 11);
        let removed = SftpRowId::remote(1, 12);
        let other_workspace = SftpRowId::remote(2, 12);
        let result =
            reconcile_sftp_selection(&[first, removed], &[first, other_workspace], Some(removed));
        assert_eq!(result.selected, Some(first));
        assert_ne!(result.selected, Some(other_workspace));
    }

    #[test]
    fn transfer_announcements_are_bounded_but_final_state_is_immediate() {
        let start = Instant::now();
        let mut coalescer = SftpAnnouncementCoalescer::default();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(coalescer.record_change(start + Duration::from_millis(20), true));
    }

    #[test]
    fn conflict_controls_keep_exact_workspace_and_operation_identity() {
        let replace = SftpAction::ResolveConflict {
            workspace_id: 7,
            operation_id: 41,
            choice: SftpConflictChoice::Replace,
        };
        let stale = SftpAction::ResolveConflict {
            workspace_id: 8,
            operation_id: 41,
            choice: SftpConflictChoice::Replace,
        };
        assert_ne!(replace, stale);
        assert_ne!(
            sftp_control_semantic_node(replace),
            sftp_control_semantic_node(stale)
        );
    }
}
