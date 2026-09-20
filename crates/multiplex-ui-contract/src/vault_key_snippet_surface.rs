use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_VAULT_KEY_SNIPPET_ROWS: usize = 4_096;
pub const MAX_VAULT_KEY_SNIPPET_CONTROLS: usize = 8_192;
pub const MAX_SNIPPET_INSERT_BYTES: usize = 64 * 1024;
pub const VAULT_KEY_SNIPPET_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const ROOT_NODE: u64 = 180_000;
const STATUS_NODE: u64 = 180_001;
const LIST_NODE: u64 = 180_002;
const ROW_NODE_BASE: u64 = 14_u64 << 60;
const CONTROL_NODE_BASE: u64 = 15_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum VaultKeySnippetRowKind {
    Vault,
    Member,
    Key,
    Identity,
    Snippet,
    Host,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VaultKeySnippetRowId {
    pub kind: VaultKeySnippetRowKind,
    pub owner: u128,
    pub value: u128,
}

impl VaultKeySnippetRowId {
    pub const fn vault(value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Vault,
            owner: 0,
            value,
        }
    }

    pub const fn member(vault: u128, value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Member,
            owner: vault,
            value,
        }
    }

    pub const fn key(vault: u128, value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Key,
            owner: vault,
            value,
        }
    }

    pub const fn identity(vault: u128, value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Identity,
            owner: vault,
            value,
        }
    }

    pub const fn snippet(vault: u128, value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Snippet,
            owner: vault,
            value,
        }
    }

    pub const fn host(operation: u128, value: u128) -> Self {
        Self {
            kind: VaultKeySnippetRowKind::Host,
            owner: operation,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VaultKeySnippetScreen {
    Vaults,
    KeychainKeys,
    KeychainIdentities,
    Snippets,
    KeyLifecycle,
    SnippetPrompts,
    SnippetInsertReview,
}

impl VaultKeySnippetScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::Vaults => MessageId::VaultsTitle,
            Self::KeychainKeys => MessageId::KeychainKeysTitle,
            Self::KeychainIdentities => MessageId::KeychainIdentitiesTitle,
            Self::Snippets => MessageId::SnippetsTitle,
            Self::KeyLifecycle => MessageId::KeyLifecycleTitle,
            Self::SnippetPrompts => MessageId::SnippetPromptsTitle,
            Self::SnippetInsertReview => MessageId::SnippetInsertReviewTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VaultKeySnippetSurfaceState {
    Ready,
    Empty,
    FilterEmpty,
    Editing,
    Validating,
    Locked,
    Unlocking,
    WrongSecret,
    KeyringDenied,
    Corrupt,
    NewerFormat,
    ImportMalformed,
    Oversize,
    Generating,
    Reviewing,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    TerminalRequired,
    Stale,
    StorageFailure,
    Unavailable,
    Error,
}

impl VaultKeySnippetSurfaceState {
    pub const ALL: [Self; 24] = [
        Self::Ready,
        Self::Empty,
        Self::FilterEmpty,
        Self::Editing,
        Self::Validating,
        Self::Locked,
        Self::Unlocking,
        Self::WrongSecret,
        Self::KeyringDenied,
        Self::Corrupt,
        Self::NewerFormat,
        Self::ImportMalformed,
        Self::Oversize,
        Self::Generating,
        Self::Reviewing,
        Self::Running,
        Self::CancelRequested,
        Self::Cancelled,
        Self::Completed,
        Self::TerminalRequired,
        Self::Stale,
        Self::StorageFailure,
        Self::Unavailable,
        Self::Error,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::VaultKeySnippetStateReady,
            Self::Empty => MessageId::VaultKeySnippetStateEmpty,
            Self::FilterEmpty => MessageId::VaultKeySnippetStateFilterEmpty,
            Self::Editing => MessageId::VaultKeySnippetStateEditing,
            Self::Validating => MessageId::VaultKeySnippetStateValidating,
            Self::Locked => MessageId::VaultKeySnippetStateLocked,
            Self::Unlocking => MessageId::VaultKeySnippetStateUnlocking,
            Self::WrongSecret => MessageId::VaultKeySnippetStateWrongSecret,
            Self::KeyringDenied => MessageId::VaultKeySnippetStateKeyringDenied,
            Self::Corrupt => MessageId::VaultKeySnippetStateCorrupt,
            Self::NewerFormat => MessageId::VaultKeySnippetStateNewerFormat,
            Self::ImportMalformed => MessageId::VaultKeySnippetStateImportMalformed,
            Self::Oversize => MessageId::VaultKeySnippetStateOversize,
            Self::Generating => MessageId::VaultKeySnippetStateGenerating,
            Self::Reviewing => MessageId::VaultKeySnippetStateReviewing,
            Self::Running => MessageId::VaultKeySnippetStateRunning,
            Self::CancelRequested => MessageId::VaultKeySnippetStateCancelRequested,
            Self::Cancelled => MessageId::VaultKeySnippetStateCancelled,
            Self::Completed => MessageId::VaultKeySnippetStateCompleted,
            Self::TerminalRequired => MessageId::VaultKeySnippetStateTerminalRequired,
            Self::Stale => MessageId::VaultKeySnippetStateStale,
            Self::StorageFailure => MessageId::VaultKeySnippetStateStorageFailure,
            Self::Unavailable => MessageId::VaultKeySnippetStateUnavailable,
            Self::Error => MessageId::VaultKeySnippetStateError,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Validating | Self::Unlocking | Self::Generating | Self::Running
        )
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::WrongSecret
                | Self::KeyringDenied
                | Self::Corrupt
                | Self::NewerFormat
                | Self::ImportMalformed
                | Self::Oversize
                | Self::TerminalRequired
                | Self::Stale
                | Self::StorageFailure
                | Self::Unavailable
                | Self::Error
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SecretFieldState {
    Masked,
    Unavailable,
}

impl SecretFieldState {
    const fn message(self) -> MessageId {
        match self {
            Self::Masked => MessageId::SecretFieldMasked,
            Self::Unavailable => MessageId::SecretFieldUnavailable,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultKeySnippetRow {
    pub id: VaultKeySnippetRowId,
    pub parent: Option<VaultKeySnippetRowId>,
    pub name: String,
    pub status: MessageId,
    pub detail: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub activatable: bool,
    pub destructive: bool,
    pub position: usize,
    pub set_size: usize,
}

impl VaultKeySnippetRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if !valid_user_text(&self.name)
            || self.position == 0
            || self.set_size == 0
            || self.position > self.set_size
            || self
                .detail
                .as_ref()
                .is_some_and(|detail| !valid_user_text(detail))
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        if !matches!(
            (self.id.kind, self.parent.map(|parent| parent.kind)),
            (
                VaultKeySnippetRowKind::Vault
                    | VaultKeySnippetRowKind::Key
                    | VaultKeySnippetRowKind::Identity
                    | VaultKeySnippetRowKind::Snippet
                    | VaultKeySnippetRowKind::Host,
                None
            ) | (
                VaultKeySnippetRowKind::Member,
                Some(VaultKeySnippetRowKind::Vault)
            )
        ) {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VaultKeySnippetAction {
    ShowKeys,
    ShowIdentities,
    NewVault,
    SelectVault(VaultKeySnippetRowId),
    SaveVault,
    DeleteVault(VaultKeySnippetRowId),
    SaveMember(VaultKeySnippetRowId),
    DeleteMember(VaultKeySnippetRowId),
    GenerateKey,
    AddKeyFile,
    UseKey(VaultKeySnippetRowId),
    DeployKey(VaultKeySnippetRowId),
    RemoveRemoteKey(VaultKeySnippetRowId),
    SelectHost(VaultKeySnippetRowId),
    ConfirmKeyOperation,
    CancelKeyOperation,
    CloseKeyLifecycle,
    NewSnippet,
    SelectSnippet(VaultKeySnippetRowId),
    SaveSnippet,
    DeleteSnippet(VaultKeySnippetRowId),
    ToggleSnippetPinned(VaultKeySnippetRowId),
    InsertSnippetAsText {
        snippet: VaultKeySnippetRowId,
        pane_id: u64,
    },
    ConfirmSnippetPrompts,
    CancelSnippetPrompts,
    ConfirmSnippetInsert {
        snippet: VaultKeySnippetRowId,
        pane_id: u64,
    },
    CancelSnippetInsert,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultKeySnippetControlRole {
    Button,
    TextField,
}

impl VaultKeySnippetControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField => SemanticRole::TextField,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultKeySnippetControl {
    pub action: VaultKeySnippetAction,
    pub parent: Option<VaultKeySnippetRowId>,
    pub role: VaultKeySnippetControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub secret: Option<SecretFieldState>,
    pub selected: bool,
    pub disabled: bool,
    pub invalid: bool,
    pub destructive: bool,
}

impl VaultKeySnippetControl {
    fn validate(&self) -> Result<(), SemanticError> {
        if self.secret.is_some() && self.value.is_some()
            || self
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
pub struct VaultKeySnippetSemanticSnapshot {
    pub screen: VaultKeySnippetScreen,
    pub state: VaultKeySnippetSurfaceState,
    pub rows: Vec<VaultKeySnippetRow>,
    pub controls: Vec<VaultKeySnippetControl>,
    pub recording_friendly: bool,
}

impl VaultKeySnippetSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_VAULT_KEY_SNIPPET_ROWS
            || self.controls.len() > MAX_VAULT_KEY_SNIPPET_CONTROLS
        {
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
            let mut node = SemanticNode::new(row_semantic_node(row.id), SemanticRole::ListItem);
            node.parent = row.parent.map(row_semantic_node).or(Some(list_id));
            node.name = Some(if self.recording_friendly {
                SemanticText::Message(private_row_message(row.id.kind))
            } else {
                SemanticText::user_text(bidi_isolate(&row.name))?
            });
            node.description = Some(SemanticText::Message(row.status));
            if let Some(detail) = row.detail.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(private_row_message(
                        row.id.kind,
                    )))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(detail))?)
                });
            }
            node.state = SemanticState {
                disabled: row.disabled,
                selected: row.selected,
                expanded: None,
                checked: None,
                invalid: false,
                busy: false,
                hidden: false,
                live: None,
            };
            node.actions.insert(SemanticAction::Focus);
            if row.activatable && !row.disabled {
                node.actions.insert(SemanticAction::Activate);
            }
            nodes.push(node);
        }

        for control in &self.controls {
            let mut node = named_node(
                control_semantic_node(control.action),
                control.role.semantic_role(),
                control.name,
            );
            node.parent = control.parent.map(row_semantic_node).or(Some(root_id));
            node.state.disabled = control.disabled;
            node.state.selected = control.selected;
            node.state.invalid = control.invalid;
            if control.destructive {
                node.description = Some(SemanticText::Message(
                    MessageId::SensitiveDestructiveActionWarning,
                ));
            }
            if let Some(secret) = control.secret {
                node.value = Some(SemanticValue::PublicText(SemanticText::Message(
                    secret.message(),
                )));
            } else if let Some(value) = control.value.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(
                        MessageId::VaultKeySnippetPrivateValue,
                    ))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(value))?)
                });
            }
            node.actions.insert(SemanticAction::Focus);
            if !control.disabled {
                node.actions
                    .insert(if control.role == VaultKeySnippetControlRole::TextField {
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
        VaultKeySnippetAccessibilityCommand,
    )> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2);
        for row in &self.rows {
            let node = row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                VaultKeySnippetAccessibilityCommand::FocusRow(row.id),
            ));
            if row.activatable && !row.disabled {
                routes.push((
                    (node, SemanticAction::Activate),
                    VaultKeySnippetAccessibilityCommand::ActivateRow(row.id),
                ));
            }
        }
        for control in &self.controls {
            let node = control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                VaultKeySnippetAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                routes.push((
                    (
                        node,
                        if control.role == VaultKeySnippetControlRole::TextField {
                            SemanticAction::SetValue
                        } else {
                            SemanticAction::Activate
                        },
                    ),
                    if control.role == VaultKeySnippetControlRole::TextField {
                        VaultKeySnippetAccessibilityCommand::SetControlValue(control.action)
                    } else {
                        VaultKeySnippetAccessibilityCommand::ActivateControl(control.action)
                    },
                ));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultKeySnippetAccessibilityCommand {
    FocusRow(VaultKeySnippetRowId),
    ActivateRow(VaultKeySnippetRowId),
    FocusControl(VaultKeySnippetAction),
    SetControlValue(VaultKeySnippetAction),
    ActivateControl(VaultKeySnippetAction),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultKeySnippetResponsiveLayout {
    Split,
    Compact,
    Stacked,
}

pub const fn vault_key_snippet_responsive_layout(
    scale_percent: u16,
) -> Option<VaultKeySnippetResponsiveLayout> {
    match scale_percent {
        100..=150 => Some(VaultKeySnippetResponsiveLayout::Split),
        151..=200 => Some(VaultKeySnippetResponsiveLayout::Compact),
        201..=400 => Some(VaultKeySnippetResponsiveLayout::Stacked),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VaultKeySnippetSelectionResult {
    pub selected: Option<VaultKeySnippetRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_vault_key_snippet_selection(
    previous: &[VaultKeySnippetRowId],
    next: &[VaultKeySnippetRowId],
    selected: Option<VaultKeySnippetRowId>,
) -> VaultKeySnippetSelectionResult {
    if let Some(selected) = selected
        && next.contains(&selected)
    {
        return VaultKeySnippetSelectionResult {
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
    VaultKeySnippetSelectionResult {
        selected: replacement,
        focus_heading: replacement.is_none(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct VaultKeySnippetAnnouncementCoalescer {
    last: Option<Instant>,
    pending: bool,
}

impl VaultKeySnippetAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= VAULT_KEY_SNIPPET_ANNOUNCEMENT_INTERVAL
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
                now.saturating_duration_since(last) >= VAULT_KEY_SNIPPET_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn stable_vault_key_snippet_value(value: &str) -> u128 {
    u128::from(stable_hash(&value))
}

pub fn vault_key_snippet_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}

pub fn row_semantic_node(id: VaultKeySnippetRowId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE | (stable_hash(&id) & NODE_MASK))
}

pub fn control_semantic_node(action: VaultKeySnippetAction) -> SemanticNodeId {
    semantic_id(CONTROL_NODE_BASE | (stable_hash(&action) & NODE_MASK))
}

fn private_row_message(kind: VaultKeySnippetRowKind) -> MessageId {
    match kind {
        VaultKeySnippetRowKind::Vault => MessageId::VaultPrivateRow,
        VaultKeySnippetRowKind::Member => MessageId::VaultMemberPrivateRow,
        VaultKeySnippetRowKind::Key | VaultKeySnippetRowKind::Identity => MessageId::KeyPrivateRow,
        VaultKeySnippetRowKind::Snippet => MessageId::SnippetPrivateRow,
        VaultKeySnippetRowKind::Host => MessageId::VaultKeySnippetPrivateHost,
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

    fn snapshot() -> VaultKeySnippetSemanticSnapshot {
        let row = VaultKeySnippetRow {
            id: VaultKeySnippetRowId::snippet(7, 41),
            parent: None,
            name: "deploy canary".to_string(),
            status: MessageId::SnippetRowSaved,
            detail: Some("production".to_string()),
            selected: true,
            disabled: false,
            activatable: true,
            destructive: false,
            position: 1,
            set_size: 1,
        };
        VaultKeySnippetSemanticSnapshot {
            screen: VaultKeySnippetScreen::Snippets,
            state: VaultKeySnippetSurfaceState::Ready,
            rows: vec![row.clone()],
            controls: vec![VaultKeySnippetControl {
                action: VaultKeySnippetAction::InsertSnippetAsText {
                    snippet: row.id,
                    pane_id: 9,
                },
                parent: Some(row.id),
                role: VaultKeySnippetControlRole::Button,
                name: MessageId::SnippetInsertAction,
                value: Some("terminal canary".to_string()),
                secret: None,
                selected: false,
                disabled: false,
                invalid: false,
                destructive: false,
            }],
            recording_friendly: false,
        }
    }

    #[test]
    fn exact_snippet_and_pane_identity_survive_routing() {
        let snapshot = snapshot();
        let expected = snapshot.controls[0].action;
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command == VaultKeySnippetAccessibilityCommand::ActivateControl(expected)
        }));
    }

    #[test]
    fn secret_controls_can_never_carry_secret_bytes() {
        let mut snapshot = snapshot();
        snapshot.controls.push(VaultKeySnippetControl {
            action: VaultKeySnippetAction::ConfirmKeyOperation,
            parent: None,
            role: VaultKeySnippetControlRole::TextField,
            name: MessageId::KeyPassphraseField,
            value: Some("secret-canary".to_string()),
            secret: Some(SecretFieldState::Masked),
            selected: false,
            disabled: false,
            invalid: false,
            destructive: false,
        });
        assert_eq!(
            snapshot.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );
    }

    #[test]
    fn recording_mode_masks_names_details_and_target_values() {
        let mut snapshot = snapshot();
        snapshot.recording_friendly = true;
        let rendered = format!("{:?}", snapshot.try_nodes(semantic_id(9)).unwrap());
        assert!(!rendered.contains("deploy canary"));
        assert!(!rendered.contains("production"));
        assert!(!rendered.contains("terminal canary"));
    }

    #[test]
    fn stale_selection_never_crosses_vault_or_row_kind() {
        let selected = VaultKeySnippetRowId::member(1, 3);
        let same_vault = VaultKeySnippetRowId::member(1, 4);
        let other_vault = VaultKeySnippetRowId::member(2, 3);
        let key = VaultKeySnippetRowId::key(1, 3);
        let result = reconcile_vault_key_snippet_selection(
            &[selected],
            &[other_vault, key, same_vault],
            Some(selected),
        );
        assert_eq!(result.selected, Some(same_vault));
    }

    #[test]
    fn every_state_locale_theme_and_scale_is_defined() {
        for state in VaultKeySnippetSurfaceState::ALL {
            assert!(MessageId::ALL.contains(&state.message()));
        }
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            assert!(
                !localizer
                    .format_static(MessageId::VaultKeySnippetStateReady)
                    .unwrap()
                    .is_empty()
            );
        }
        for theme in ThemeKind::ALL {
            assert!(DesignTokens::new(theme).focus_ring_width().0 >= 2.0);
        }
        for scale in [100, 150, 200, 300, 400] {
            assert!(vault_key_snippet_responsive_layout(scale).is_some());
        }
        assert!(vault_key_snippet_responsive_layout(99).is_none());
        assert!(vault_key_snippet_responsive_layout(401).is_none());
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
        oversized.rows = vec![row; MAX_VAULT_KEY_SNIPPET_ROWS + 1];
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn announcements_are_bounded_but_final_results_are_immediate() {
        let start = Instant::now();
        let mut coalescer = VaultKeySnippetAnnouncementCoalescer::default();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(coalescer.record_change(start + Duration::from_millis(20), true));
    }

    #[test]
    fn snippet_insert_limit_is_explicit_and_bounded() {
        assert_eq!(MAX_SNIPPET_INSERT_BYTES, 65_536);
    }

    #[test]
    fn destructive_controls_have_a_non_color_warning() {
        let mut snapshot = snapshot();
        snapshot.controls[0].destructive = true;
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        assert!(nodes.iter().any(|node| {
            node.description
                == Some(SemanticText::Message(
                    MessageId::SensitiveDestructiveActionWarning,
                ))
        }));
    }
}
