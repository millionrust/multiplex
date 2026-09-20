use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_HOST_CONNECTION_ROWS: usize = 2_048;
pub const MAX_HOST_CONNECTION_CONTROLS: usize = 4_096;
const MAX_HOST_CONNECTION_NODES: usize = 8_192;
pub const HOST_CONNECTION_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);
const ROOT_NODE: u64 = 140_000;
const STATUS_NODE: u64 = 140_001;
const LIST_NODE: u64 = 140_002;
const ROW_NODE_BASE: u64 = 10_u64 << 60;
const CONTROL_NODE_BASE: u64 = 11_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HostConnectionRowKind {
    Host,
    Diagnostic,
    Protocol,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostConnectionRowId {
    pub kind: HostConnectionRowKind,
    pub owner: u128,
    pub value: u128,
}

impl HostConnectionRowId {
    pub const fn host(value: u128) -> Self {
        Self {
            kind: HostConnectionRowKind::Host,
            owner: 0,
            value,
        }
    }

    pub const fn diagnostic(host: u128, operation: u64) -> Self {
        Self {
            kind: HostConnectionRowKind::Diagnostic,
            owner: host,
            value: operation as u128,
        }
    }

    pub const fn protocol(value: u128) -> Self {
        Self {
            kind: HostConnectionRowKind::Protocol,
            owner: 0,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostConnectionScreen {
    Hosts,
    HostEditor,
    ConnectUsername,
    ChooseProtocol,
    ConnectionFailure,
}

impl HostConnectionScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::Hosts => MessageId::HostsTitle,
            Self::HostEditor => MessageId::HostEditorTitle,
            Self::ConnectUsername => MessageId::ConnectUsernameTitle,
            Self::ChooseProtocol => MessageId::ConnectProtocolTitle,
            Self::ConnectionFailure => MessageId::ConnectFailureTitle,
        }
    }

    const fn is_dialog(self) -> bool {
        !matches!(self, Self::Hosts)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostConnectionSurfaceState {
    Ready,
    Loading,
    Empty,
    FilterEmpty,
    Editing,
    Validating,
    Connecting,
    DiagnosticQueued,
    DiagnosticRunning,
    Partial,
    Cancelled,
    Offline,
    PermissionDenied,
    AuthenticationDenied,
    HostKeyUnknown,
    HostKeyMismatch,
    Timeout,
    InvalidTarget,
    CredentialStoreUnavailable,
    Stale,
    Recovery,
    Unavailable,
    Error,
}

impl HostConnectionSurfaceState {
    pub const ALL: [Self; 23] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::FilterEmpty,
        Self::Editing,
        Self::Validating,
        Self::Connecting,
        Self::DiagnosticQueued,
        Self::DiagnosticRunning,
        Self::Partial,
        Self::Cancelled,
        Self::Offline,
        Self::PermissionDenied,
        Self::AuthenticationDenied,
        Self::HostKeyUnknown,
        Self::HostKeyMismatch,
        Self::Timeout,
        Self::InvalidTarget,
        Self::CredentialStoreUnavailable,
        Self::Stale,
        Self::Recovery,
        Self::Unavailable,
        Self::Error,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::HostsStateReady,
            Self::Loading => MessageId::HostsStateLoading,
            Self::Empty => MessageId::HostsEmptyTitle,
            Self::FilterEmpty => MessageId::HostsFilterEmpty,
            Self::Editing => MessageId::HostEditorStateEditing,
            Self::Validating => MessageId::ConnectStateValidating,
            Self::Connecting => MessageId::ConnectStateConnecting,
            Self::DiagnosticQueued => MessageId::HostDiagnosticQueued,
            Self::DiagnosticRunning => MessageId::HostDiagnosticRunning,
            Self::Partial => MessageId::HostsStatePartial,
            Self::Cancelled => MessageId::ConnectStateCancelled,
            Self::Offline => MessageId::ConnectErrorOffline,
            Self::PermissionDenied => MessageId::ConnectErrorPermission,
            Self::AuthenticationDenied => MessageId::ConnectErrorAuthentication,
            Self::HostKeyUnknown => MessageId::ConnectErrorHostKeyUnknown,
            Self::HostKeyMismatch => MessageId::ConnectErrorHostKeyMismatch,
            Self::Timeout => MessageId::ConnectErrorTimeout,
            Self::InvalidTarget => MessageId::ConnectErrorInvalidTarget,
            Self::CredentialStoreUnavailable => MessageId::ConnectErrorCredentialStore,
            Self::Stale => MessageId::ConnectErrorStale,
            Self::Recovery => MessageId::ConnectStateRecovery,
            Self::Unavailable => MessageId::ConnectErrorUnavailable,
            Self::Error => MessageId::ConnectErrorGeneric,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Loading
                | Self::Validating
                | Self::Connecting
                | Self::DiagnosticQueued
                | Self::DiagnosticRunning
        )
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::PermissionDenied
                | Self::AuthenticationDenied
                | Self::HostKeyUnknown
                | Self::HostKeyMismatch
                | Self::Timeout
                | Self::InvalidTarget
                | Self::CredentialStoreUnavailable
                | Self::Stale
                | Self::Unavailable
                | Self::Error
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostConnectionRow {
    pub id: HostConnectionRowId,
    pub parent: Option<HostConnectionRowId>,
    pub name: String,
    pub status: MessageId,
    pub detail: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub checked: Option<bool>,
    pub invalid: bool,
    pub stale: bool,
    pub position: usize,
    pub set_size: usize,
}

impl HostConnectionRow {
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
        if !matches!(
            (self.id.kind, self.parent.map(|parent| parent.kind)),
            (
                HostConnectionRowKind::Host | HostConnectionRowKind::Protocol,
                None
            ) | (
                HostConnectionRowKind::Diagnostic,
                Some(HostConnectionRowKind::Host)
            )
        ) {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostAuthChoice {
    Password,
    PrivateKey,
    LocalAgent,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostConnectionAction {
    AddHost,
    SetSearch,
    QuickConnect,
    SetQuickConnectPassword,
    SelectHost(HostConnectionRowId),
    ConnectHost(HostConnectionRowId),
    EditHost(HostConnectionRowId),
    ToggleFavorite(HostConnectionRowId),
    ToggleBatchSelection(HostConnectionRowId),
    SelectVisible,
    ClearSelection,
    DiagnoseSelected,
    ClearFinishedDiagnostics,
    CancelDiagnostic(HostConnectionRowId),
    RetryDiagnostic(HostConnectionRowId),
    SetBulkGroup,
    ApplyBulkGroup,
    SetHostLabel,
    SetHostAddress,
    SetHostPort,
    SetHostUsername,
    SelectAuth(HostAuthChoice),
    SetHostPassword,
    SetHostKeyPath,
    SetHostKeyPassphrase,
    SetHostAgentSocket,
    SaveHost,
    DeleteHost,
    CloseHostEditor,
    SetConnectUsername,
    ContinueAndSave,
    SelectProtocol(HostConnectionRowId),
    SetProtocolPort,
    ContinueProtocol,
    ForwardAgentOnce,
    CopyFailureLog,
    EditFailedHost,
    RetryConnection,
    CloseConnectionDialog,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostConnectionControlRole {
    Button,
    TextField,
    PasswordField,
    Checkbox,
    RadioButton,
}

impl HostConnectionControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField | Self::PasswordField => SemanticRole::TextField,
            Self::Checkbox => SemanticRole::Checkbox,
            Self::RadioButton => SemanticRole::RadioButton,
        }
    }

    const fn is_value_control(self) -> bool {
        matches!(self, Self::TextField | Self::PasswordField)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostConnectionControl {
    pub action: HostConnectionAction,
    pub parent: Option<HostConnectionRowId>,
    pub role: HostConnectionControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub invalid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostConnectionTextScale(u16);

impl HostConnectionTextScale {
    pub const fn try_new(percent: u16) -> Option<Self> {
        if percent >= 100 && percent <= 400 {
            Some(Self(percent))
        } else {
            None
        }
    }

    pub const fn layout(self) -> HostConnectionResponsiveLayout {
        match self.0 {
            100..=150 => HostConnectionResponsiveLayout::ListAndInspector,
            151..=200 => HostConnectionResponsiveLayout::Compact,
            _ => HostConnectionResponsiveLayout::Stacked,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostConnectionResponsiveLayout {
    ListAndInspector,
    Compact,
    Stacked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostConnectionSemanticSnapshot {
    pub screen: HostConnectionScreen,
    pub state: HostConnectionSurfaceState,
    pub rows: Vec<HostConnectionRow>,
    pub controls: Vec<HostConnectionControl>,
    pub recording_friendly: bool,
}

impl HostConnectionSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_HOST_CONNECTION_ROWS
            || self.controls.len() > MAX_HOST_CONNECTION_CONTROLS
            || self.rows.len() + self.controls.len() + 3 > MAX_HOST_CONNECTION_NODES
        {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        let mut row_nodes = HashMap::with_capacity(self.rows.len());
        let mut semantic_ids = HashSet::with_capacity(self.rows.len() + self.controls.len());
        for row in &self.rows {
            row.validate()?;
            let node = host_connection_row_semantic_node(row.id);
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
            let node = host_connection_control_semantic_node(control.action);
            if !semantic_ids.insert(node) || control_nodes.insert(control.action, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }

        let root_id = semantic_id(ROOT_NODE);
        let mut root = named_node(
            root_id,
            if self.screen.is_dialog() {
                SemanticRole::Dialog
            } else {
                SemanticRole::Landmark
            },
            self.screen.title(),
        );
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
                            .expect("validated host detail remains bounded"),
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
                checked: row.checked,
                invalid: row.invalid,
                busy: false,
                hidden: false,
                live: row.stale.then_some(LiveRegionPoliteness::Polite),
            };
            node.actions.insert(SemanticAction::Focus);
            if !row.disabled && row.id.kind == HostConnectionRowKind::Host {
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
            node.state.checked = matches!(
                control.role,
                HostConnectionControlRole::Checkbox | HostConnectionControlRole::RadioButton
            )
            .then_some(control.selected);
            node.state.invalid = control.invalid;
            if control.role == HostConnectionControlRole::PasswordField {
                node.value = control.value.as_ref().map(|_| {
                    SemanticValue::PublicText(SemanticText::Message(MessageId::HostSecretSet))
                });
            } else if let Some(value) = control.value.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(private_control_message(
                        control.action,
                    )))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(value))?)
                });
            }
            node.actions.insert(SemanticAction::Focus);
            if !control.disabled {
                node.actions.insert(if control.role.is_value_control() {
                    SemanticAction::SetValue
                } else {
                    SemanticAction::Activate
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
        HostConnectionAccessibilityCommand,
    )> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2);
        for row in &self.rows {
            let node = host_connection_row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                HostConnectionAccessibilityCommand::FocusRow(row.id),
            ));
            if !row.disabled && row.id.kind == HostConnectionRowKind::Host {
                routes.push((
                    (node, SemanticAction::Activate),
                    HostConnectionAccessibilityCommand::ActivateRow(row.id),
                ));
            }
        }
        for control in &self.controls {
            let node = host_connection_control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                HostConnectionAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                let (action, command) = if control.role.is_value_control() {
                    (
                        SemanticAction::SetValue,
                        HostConnectionAccessibilityCommand::SetControlValue(control.action),
                    )
                } else {
                    (
                        SemanticAction::Activate,
                        HostConnectionAccessibilityCommand::ActivateControl(control.action),
                    )
                };
                routes.push(((node, action), command));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostConnectionAccessibilityCommand {
    FocusRow(HostConnectionRowId),
    ActivateRow(HostConnectionRowId),
    FocusControl(HostConnectionAction),
    SetControlValue(HostConnectionAction),
    ActivateControl(HostConnectionAction),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostConnectionSelectionResult {
    pub selected: Option<HostConnectionRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_host_connection_selection(
    previous: &[HostConnectionRowId],
    next: &[HostConnectionRowId],
    selected: Option<HostConnectionRowId>,
) -> HostConnectionSelectionResult {
    if let Some(selected) = selected
        && next.contains(&selected)
    {
        return HostConnectionSelectionResult {
            selected: Some(selected),
            focus_heading: false,
        };
    }
    let prior_index = selected
        .and_then(|selected| previous.iter().position(|id| *id == selected))
        .unwrap_or(0);
    HostConnectionSelectionResult {
        selected: next
            .get(prior_index.min(next.len().saturating_sub(1)))
            .copied(),
        focus_heading: next.is_empty(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct HostConnectionAnnouncementCoalescer {
    last_announcement: Option<Instant>,
    pending: bool,
}

impl HostConnectionAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last_announcement.is_none_or(|previous| {
                now.saturating_duration_since(previous) >= HOST_CONNECTION_ANNOUNCEMENT_INTERVAL
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
                now.saturating_duration_since(previous) >= HOST_CONNECTION_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last_announcement = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn stable_host_row_value(host_id: &str) -> u128 {
    u128::from(stable_hash(&host_id))
}
pub fn host_connection_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}
pub fn host_connection_row_semantic_node(id: HostConnectionRowId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE | (stable_hash(&id) & NODE_MASK))
}
pub fn host_connection_control_semantic_node(action: HostConnectionAction) -> SemanticNodeId {
    semantic_id(CONTROL_NODE_BASE | (stable_hash(&action) & NODE_MASK))
}

fn private_row_message(kind: HostConnectionRowKind) -> MessageId {
    match kind {
        HostConnectionRowKind::Host => MessageId::HostPrivateRow,
        HostConnectionRowKind::Diagnostic => MessageId::HostPrivateDiagnostic,
        HostConnectionRowKind::Protocol => MessageId::ConnectProtocolSsh,
    }
}

fn private_control_message(action: HostConnectionAction) -> MessageId {
    match action {
        HostConnectionAction::SetHostAddress => MessageId::HostPrivateAddress,
        HostConnectionAction::SetHostUsername | HostConnectionAction::SetConnectUsername => {
            MessageId::HostPrivateUsername
        }
        HostConnectionAction::SetHostKeyPath => MessageId::HostPrivateKeyPath,
        HostConnectionAction::SetHostAgentSocket => MessageId::HostPrivateAgentSocket,
        HostConnectionAction::SetHostLabel => MessageId::HostPrivateRow,
        _ => MessageId::HostPrivateValue,
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

    fn host_row(name: &str) -> HostConnectionRow {
        HostConnectionRow {
            id: HostConnectionRowId::host(stable_host_row_value("host-1")),
            parent: None,
            name: name.to_string(),
            status: MessageId::HostAuthPassword,
            detail: Some("deploy@example.test:22".to_string()),
            selected: true,
            disabled: false,
            checked: Some(true),
            invalid: false,
            stale: false,
            position: 1,
            set_size: 1,
        }
    }

    fn snapshot() -> HostConnectionSemanticSnapshot {
        let row = host_row("Production");
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::Hosts,
            state: HostConnectionSurfaceState::Ready,
            rows: vec![row.clone()],
            controls: vec![HostConnectionControl {
                action: HostConnectionAction::ConnectHost(row.id),
                parent: Some(row.id),
                role: HostConnectionControlRole::Button,
                name: MessageId::CommonConnect,
                value: None,
                selected: false,
                disabled: false,
                invalid: false,
            }],
            recording_friendly: false,
        }
    }

    #[test]
    fn host_rows_and_controls_have_typed_routes() {
        let snapshot = snapshot();
        let row = snapshot.rows[0].id;
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        assert!(
            nodes.iter().any(
                |node| node.id == host_connection_row_semantic_node(row) && node.state.selected
            )
        );
        assert!(snapshot.routes().iter().any(|(_, command)| *command
            == HostConnectionAccessibilityCommand::ActivateControl(
                HostConnectionAction::ConnectHost(row)
            )));
    }

    #[test]
    fn secrets_and_recording_values_never_enter_semantics() {
        let mut snapshot = snapshot();
        snapshot.screen = HostConnectionScreen::HostEditor;
        snapshot.controls = vec![
            HostConnectionControl {
                action: HostConnectionAction::SetHostPassword,
                parent: None,
                role: HostConnectionControlRole::PasswordField,
                name: MessageId::HostPasswordField,
                value: Some("password-canary".to_string()),
                selected: false,
                disabled: false,
                invalid: false,
            },
            HostConnectionControl {
                action: HostConnectionAction::SetHostAddress,
                parent: None,
                role: HostConnectionControlRole::TextField,
                name: MessageId::HostAddressField,
                value: Some("private.example.test".to_string()),
                selected: false,
                disabled: false,
                invalid: false,
            },
        ];
        let semantics = format!("{:?}", snapshot.try_nodes(semantic_id(9)).unwrap());
        assert!(!semantics.contains("password-canary"));
        snapshot.recording_friendly = true;
        let masked = format!("{:?}", snapshot.try_nodes(semantic_id(9)).unwrap());
        assert!(!masked.contains("private.example.test"));
    }

    #[test]
    fn disabled_actions_are_not_routable() {
        let mut snapshot = snapshot();
        snapshot.controls[0].disabled = true;
        let action = snapshot.controls[0].action;
        assert!(
            !snapshot.routes().iter().any(|(_, command)| *command
                == HostConnectionAccessibilityCommand::ActivateControl(action))
        );
    }

    #[test]
    fn every_state_localizes_for_required_locales_and_themes() {
        for locale in [Locale::EnUs, Locale::EnXa, Locale::ArXb] {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            for screen in [
                HostConnectionScreen::Hosts,
                HostConnectionScreen::HostEditor,
                HostConnectionScreen::ConnectUsername,
                HostConnectionScreen::ChooseProtocol,
                HostConnectionScreen::ConnectionFailure,
            ] {
                assert!(!localizer.format_static(screen.title()).unwrap().is_empty());
                for state in HostConnectionSurfaceState::ALL {
                    assert!(!localizer.format_static(state.message()).unwrap().is_empty());
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
    fn scale_reflows_and_selection_reconciles() {
        assert_eq!(
            HostConnectionTextScale::try_new(100).unwrap().layout(),
            HostConnectionResponsiveLayout::ListAndInspector
        );
        assert_eq!(
            HostConnectionTextScale::try_new(200).unwrap().layout(),
            HostConnectionResponsiveLayout::Compact
        );
        assert_eq!(
            HostConnectionTextScale::try_new(400).unwrap().layout(),
            HostConnectionResponsiveLayout::Stacked
        );
        assert!(HostConnectionTextScale::try_new(99).is_none());
        assert!(HostConnectionTextScale::try_new(401).is_none());
        let first = HostConnectionRowId::host(1);
        let second = HostConnectionRowId::host(2);
        let result = reconcile_host_connection_selection(&[first, second], &[second], Some(first));
        assert_eq!(result.selected, Some(second));
        assert!(!result.focus_heading);
        assert!(reconcile_host_connection_selection(&[second], &[], Some(second)).focus_heading);
    }

    #[test]
    fn invalid_parent_and_resource_limit_are_rejected() {
        let mut invalid = snapshot();
        invalid.rows[0].parent = Some(HostConnectionRowId::host(99));
        assert_eq!(
            invalid.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );
        let mut oversized = snapshot();
        oversized.rows = (0..=MAX_HOST_CONNECTION_ROWS)
            .map(|index| HostConnectionRow {
                id: HostConnectionRowId::host(index as u128 + 1),
                ..host_row("bounded")
            })
            .collect();
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn announcements_coalesce_but_final_state_is_immediate() {
        let start = Instant::now();
        let mut coalescer = HostConnectionAnnouncementCoalescer::default();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(!coalescer.flush(start + Duration::from_millis(100)));
        assert!(coalescer.flush(start + Duration::from_millis(260)));
        assert!(coalescer.record_change(start + Duration::from_millis(270), true));
    }
}
