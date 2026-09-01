use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_PRESET_RUNTIME_ROWS: usize = 1_024;
pub const MAX_PRESET_RUNTIME_CONTROLS: usize = 2_048;
const MAX_PRESET_RUNTIME_NODES: usize = 4_096;
pub const PRESET_RUNTIME_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const ROOT_NODE: u64 = 120_000;
const STATUS_NODE: u64 = 120_001;
const LIST_NODE: u64 = 120_002;
const ROW_NODE_BASE: u64 = 6_u64 << 60;
const CONTROL_NODE_BASE: u64 = 7_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PresetRuntimeRowKind {
    Preset,
    Runtime,
    Capability,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PresetRuntimeRowId {
    pub kind: PresetRuntimeRowKind,
    pub value: u128,
}

impl PresetRuntimeRowId {
    pub const fn preset(value: u128) -> Self {
        Self {
            kind: PresetRuntimeRowKind::Preset,
            value,
        }
    }

    pub const fn runtime(value: u128) -> Self {
        Self {
            kind: PresetRuntimeRowKind::Runtime,
            value,
        }
    }

    pub const fn capability(value: u128) -> Self {
        Self {
            kind: PresetRuntimeRowKind::Capability,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetRuntimeScreen {
    PresetsAndRuntimes,
    RuntimeInspector,
}

impl PresetRuntimeScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::PresetsAndRuntimes => MessageId::PresetsTitle,
            Self::RuntimeInspector => MessageId::SessionLibraryInspectorTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetRuntimeSurfaceState {
    Ready,
    Loading,
    Empty,
    Scanning,
    Partial,
    Cancelled,
    PermissionDenied,
    Timeout,
    Malformed,
    Recovery,
    Unavailable,
    Corrupt,
    NewerFormat,
    Unsupported,
    RiskReview,
    Error,
}

impl PresetRuntimeSurfaceState {
    pub const ALL: [Self; 16] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::Scanning,
        Self::Partial,
        Self::Cancelled,
        Self::PermissionDenied,
        Self::Timeout,
        Self::Malformed,
        Self::Recovery,
        Self::Unavailable,
        Self::Corrupt,
        Self::NewerFormat,
        Self::Unsupported,
        Self::RiskReview,
        Self::Error,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::PresetsReadyStatus,
            Self::Loading | Self::Scanning => MessageId::PresetsScanning,
            Self::Empty => MessageId::PresetsEmptyTitle,
            Self::Partial => MessageId::PresetsScanPartial,
            Self::Cancelled => MessageId::PresetsScanCancelled,
            Self::PermissionDenied => MessageId::PresetStatusPermission,
            Self::Timeout => MessageId::PresetStatusTimeout,
            Self::Malformed => MessageId::PresetStatusUnknown,
            Self::Recovery => MessageId::PresetStoreRecovered,
            Self::Unavailable => MessageId::PresetStoreUnavailable,
            Self::Corrupt => MessageId::PresetStoreCorrupt,
            Self::NewerFormat => MessageId::PresetStoreNewer,
            Self::Unsupported => MessageId::PresetStatusUnsupported,
            Self::RiskReview => MessageId::PresetRiskWarning,
            Self::Error => MessageId::PresetStatusFailed,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(self, Self::Loading | Self::Scanning)
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::PermissionDenied
                | Self::Timeout
                | Self::Malformed
                | Self::Unavailable
                | Self::Corrupt
                | Self::NewerFormat
                | Self::Error
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetRuntimeRow {
    pub id: PresetRuntimeRowId,
    pub parent: Option<PresetRuntimeRowId>,
    pub name: String,
    pub status: MessageId,
    pub detail: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub checked: Option<bool>,
    pub risky: bool,
    pub stale: bool,
    pub position: usize,
    pub set_size: usize,
}

impl PresetRuntimeRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > crate::MAX_SEMANTIC_TEXT_CHARS
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
        let valid_parent = matches!(
            (self.id.kind, self.parent.map(|parent| parent.kind)),
            (
                PresetRuntimeRowKind::Preset | PresetRuntimeRowKind::Runtime,
                None
            ) | (
                PresetRuntimeRowKind::Capability,
                Some(PresetRuntimeRowKind::Runtime)
            )
        );
        if !valid_parent {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetMoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetWorkingDirectoryChoice {
    ProjectRoot,
    PlatformHome,
    ContainedSubdirectory,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetPermissionChoice {
    AskAsNeeded,
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetRuntimeAction {
    RetryStore,
    StartScan,
    CancelScan,
    AddPreset,
    AcceptRuntime(PresetRuntimeRowId),
    MovePreset(PresetRuntimeRowId, PresetMoveDirection),
    TogglePresetEnabled(PresetRuntimeRowId),
    TogglePresetFavorite(PresetRuntimeRowId),
    EditPreset(PresetRuntimeRowId),
    DeletePreset(PresetRuntimeRowId),
    SetPresetLabel,
    SetPresetExecutable,
    SetPresetArgument(usize),
    AddPresetArgument,
    RemovePresetArgument(usize),
    SelectWorkingDirectory(PresetWorkingDirectoryChoice),
    SetPresetSubdirectory,
    SelectPermission(PresetPermissionChoice),
    ToggleEditorEnabled,
    ToggleEditorFavorite,
    ConfirmRisk,
    SavePreset,
    CancelPreset,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetRuntimeControlRole {
    Button,
    TextField,
    Checkbox,
    RadioButton,
}

impl PresetRuntimeControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField => SemanticRole::TextField,
            Self::Checkbox => SemanticRole::Checkbox,
            Self::RadioButton => SemanticRole::RadioButton,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetRuntimeControl {
    pub action: PresetRuntimeAction,
    pub parent: Option<PresetRuntimeRowId>,
    pub role: PresetRuntimeControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub invalid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresetRuntimeTextScale(u16);

impl PresetRuntimeTextScale {
    pub const fn try_new(percent: u16) -> Option<Self> {
        if percent >= 100 && percent <= 400 {
            Some(Self(percent))
        } else {
            None
        }
    }

    pub const fn layout(self) -> PresetRuntimeResponsiveLayout {
        match self.0 {
            100..=150 => PresetRuntimeResponsiveLayout::ListAndForm,
            151..=200 => PresetRuntimeResponsiveLayout::Compact,
            _ => PresetRuntimeResponsiveLayout::Stacked,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetRuntimeResponsiveLayout {
    ListAndForm,
    Compact,
    Stacked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetRuntimeSemanticSnapshot {
    pub screen: PresetRuntimeScreen,
    pub state: PresetRuntimeSurfaceState,
    pub rows: Vec<PresetRuntimeRow>,
    pub controls: Vec<PresetRuntimeControl>,
    pub recording_friendly: bool,
}

impl PresetRuntimeSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_PRESET_RUNTIME_ROWS
            || self.controls.len() > MAX_PRESET_RUNTIME_CONTROLS
            || self.rows.len() + self.controls.len() + 3 > MAX_PRESET_RUNTIME_NODES
        {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }

        let mut row_ids = HashSet::with_capacity(self.rows.len());
        let mut row_nodes = HashMap::with_capacity(self.rows.len());
        let mut semantic_ids = HashSet::with_capacity(self.rows.len() + self.controls.len());
        for row in &self.rows {
            row.validate()?;
            let node = preset_runtime_row_semantic_node(row.id);
            if !row_ids.insert(row.id)
                || !semantic_ids.insert(node)
                || row_nodes.insert(row.id, node).is_some()
            {
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
            let node = preset_runtime_control_semantic_node(control.action);
            if !semantic_ids.insert(node) || control_nodes.insert(control.action, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }

        let root_id = semantic_id(ROOT_NODE);
        let mut root = named_node(root_id, SemanticRole::Landmark, self.screen.title());
        root.parent = Some(parent);
        root.state.busy = self.state.is_busy();

        let status_id = semantic_id(STATUS_NODE);
        let mut status = named_node(status_id, SemanticRole::Status, self.state.message());
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
            node.value = if let Some(detail) = row.detail.as_ref() {
                Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(
                        MessageId::RuntimeCapabilitiesNone,
                    ))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(detail))?)
                })
            } else {
                Some(SemanticValue::Number {
                    current: row.position as i64,
                    minimum: 1,
                    maximum: row.set_size as i64,
                })
            };
            node.state = SemanticState {
                disabled: row.disabled,
                selected: row.selected,
                checked: row.checked,
                invalid: row.risky,
                busy: false,
                hidden: false,
                expanded: None,
                live: row.stale.then_some(LiveRegionPoliteness::Polite),
            };
            node.actions.insert(SemanticAction::Focus);
            if !row.disabled && row.id.kind == PresetRuntimeRowKind::Preset {
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
                PresetRuntimeControlRole::Checkbox | PresetRuntimeControlRole::RadioButton
            )
            .then_some(control.selected);
            node.state.invalid = control.invalid;
            if let Some(value) = control.value.as_ref() {
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
                match control.role {
                    PresetRuntimeControlRole::TextField => {
                        node.actions.insert(SemanticAction::SetValue);
                    }
                    PresetRuntimeControlRole::Button
                    | PresetRuntimeControlRole::Checkbox
                    | PresetRuntimeControlRole::RadioButton => {
                        node.actions.insert(SemanticAction::Activate);
                    }
                }
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    pub fn routes(
        &self,
    ) -> Vec<(
        (SemanticNodeId, SemanticAction),
        PresetRuntimeAccessibilityCommand,
    )> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2);
        for row in &self.rows {
            let node = preset_runtime_row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                PresetRuntimeAccessibilityCommand::FocusRow(row.id),
            ));
            if !row.disabled && row.id.kind == PresetRuntimeRowKind::Preset {
                routes.push((
                    (node, SemanticAction::Activate),
                    PresetRuntimeAccessibilityCommand::ActivateRow(row.id),
                ));
            }
        }
        for control in &self.controls {
            let node = preset_runtime_control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                PresetRuntimeAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                let (semantic_action, command) = match control.role {
                    PresetRuntimeControlRole::TextField => (
                        SemanticAction::SetValue,
                        PresetRuntimeAccessibilityCommand::SetControlValue(control.action),
                    ),
                    _ => (
                        SemanticAction::Activate,
                        PresetRuntimeAccessibilityCommand::ActivateControl(control.action),
                    ),
                };
                routes.push(((node, semantic_action), command));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetRuntimeAccessibilityCommand {
    FocusRow(PresetRuntimeRowId),
    ActivateRow(PresetRuntimeRowId),
    FocusControl(PresetRuntimeAction),
    SetControlValue(PresetRuntimeAction),
    ActivateControl(PresetRuntimeAction),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresetRuntimeSelectionResult {
    pub selected: Option<PresetRuntimeRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_preset_runtime_selection(
    previous: &[PresetRuntimeRowId],
    next: &[PresetRuntimeRowId],
    selected: Option<PresetRuntimeRowId>,
) -> PresetRuntimeSelectionResult {
    if let Some(selected) = selected
        && next.contains(&selected)
    {
        return PresetRuntimeSelectionResult {
            selected: Some(selected),
            focus_heading: false,
        };
    }
    let prior_index = selected
        .and_then(|selected| previous.iter().position(|candidate| *candidate == selected))
        .unwrap_or(0);
    PresetRuntimeSelectionResult {
        selected: next
            .get(prior_index.min(next.len().saturating_sub(1)))
            .copied(),
        focus_heading: next.is_empty(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct PresetRuntimeAnnouncementCoalescer {
    last_announcement: Option<Instant>,
    pending: bool,
}

impl PresetRuntimeAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last_announcement.is_none_or(|previous| {
                now.saturating_duration_since(previous) >= PRESET_RUNTIME_ANNOUNCEMENT_INTERVAL
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
                now.saturating_duration_since(previous) >= PRESET_RUNTIME_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last_announcement = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn stable_runtime_row_value(runtime_id: &str) -> u128 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in runtime_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    u128::from(hash.max(1))
}

pub fn stable_capability_row_value(runtime_id: &str, capability: MessageId) -> u128 {
    let mut hasher = StableHasher::default();
    runtime_id.hash(&mut hasher);
    capability.hash(&mut hasher);
    u128::from(hasher.finish().max(1))
}

pub fn preset_runtime_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}

pub fn preset_runtime_row_semantic_node(id: PresetRuntimeRowId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE | (stable_hash(&id) & NODE_MASK))
}

pub fn preset_runtime_control_semantic_node(action: PresetRuntimeAction) -> SemanticNodeId {
    semantic_id(CONTROL_NODE_BASE | (stable_hash(&action) & NODE_MASK))
}

fn private_row_message(kind: PresetRuntimeRowKind) -> MessageId {
    match kind {
        PresetRuntimeRowKind::Preset => MessageId::PresetLabelField,
        PresetRuntimeRowKind::Runtime => MessageId::RuntimeLabelGeneric,
        PresetRuntimeRowKind::Capability => MessageId::RuntimeInspectorCapabilitiesLabel,
    }
}

fn private_control_message(action: PresetRuntimeAction) -> MessageId {
    match action {
        PresetRuntimeAction::SetPresetExecutable => MessageId::PresetExecutableField,
        PresetRuntimeAction::SetPresetArgument(_) => MessageId::PresetArgumentsField,
        PresetRuntimeAction::SetPresetSubdirectory => MessageId::PresetSubdirectoryField,
        _ => MessageId::PresetLabelField,
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

    fn preset_row(value: u128) -> PresetRuntimeRow {
        PresetRuntimeRow {
            id: PresetRuntimeRowId::preset(value),
            parent: None,
            name: "Customer preset".to_string(),
            status: MessageId::PresetStatusSupported,
            detail: Some("codex, 2 arguments".to_string()),
            selected: true,
            disabled: false,
            checked: Some(true),
            risky: false,
            stale: false,
            position: 1,
            set_size: 1,
        }
    }

    fn snapshot() -> PresetRuntimeSemanticSnapshot {
        PresetRuntimeSemanticSnapshot {
            screen: PresetRuntimeScreen::PresetsAndRuntimes,
            state: PresetRuntimeSurfaceState::Ready,
            rows: vec![preset_row(7)],
            controls: vec![PresetRuntimeControl {
                action: PresetRuntimeAction::EditPreset(PresetRuntimeRowId::preset(7)),
                parent: Some(PresetRuntimeRowId::preset(7)),
                role: PresetRuntimeControlRole::Button,
                name: MessageId::PresetEditAction,
                value: None,
                selected: false,
                disabled: false,
                invalid: false,
            }],
            recording_friendly: false,
        }
    }

    #[test]
    fn preset_runtime_surface_rows_and_routes_preserve_typed_identity() {
        let snapshot = snapshot();
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        let row = preset_runtime_row_semantic_node(PresetRuntimeRowId::preset(7));
        assert!(
            nodes
                .iter()
                .any(|node| node.id == row && node.state.selected)
        );
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command
                == PresetRuntimeAccessibilityCommand::ActivateRow(PresetRuntimeRowId::preset(7))
        }));
    }

    #[test]
    fn preset_runtime_surface_capabilities_are_navigable_children() {
        let runtime = PresetRuntimeRowId::runtime(stable_runtime_row_value("codex"));
        let capability = PresetRuntimeRowId::capability(stable_capability_row_value(
            "codex",
            MessageId::RuntimeCapabilityResume,
        ));
        let mut snapshot = snapshot();
        snapshot.rows = vec![
            PresetRuntimeRow {
                id: runtime,
                parent: None,
                name: "Codex".to_string(),
                status: MessageId::PresetStatusSupported,
                detail: Some("1.0.7".to_string()),
                selected: false,
                disabled: false,
                checked: None,
                risky: false,
                stale: false,
                position: 1,
                set_size: 1,
            },
            PresetRuntimeRow {
                id: capability,
                parent: Some(runtime),
                name: "Resume".to_string(),
                status: MessageId::RuntimeConfidenceVerified,
                detail: None,
                selected: false,
                disabled: false,
                checked: Some(true),
                risky: false,
                stale: false,
                position: 1,
                set_size: 1,
            },
        ];
        snapshot.controls.clear();
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        let child = nodes
            .iter()
            .find(|node| node.id == preset_runtime_row_semantic_node(capability))
            .unwrap();
        assert_eq!(
            child.parent,
            Some(preset_runtime_row_semantic_node(runtime))
        );
    }

    #[test]
    fn preset_runtime_surface_risk_is_explicit_and_save_can_be_disabled() {
        let mut snapshot = snapshot();
        snapshot.state = PresetRuntimeSurfaceState::RiskReview;
        snapshot.controls.push(PresetRuntimeControl {
            action: PresetRuntimeAction::ConfirmRisk,
            parent: None,
            role: PresetRuntimeControlRole::Checkbox,
            name: MessageId::PresetRiskConfirmField,
            value: None,
            selected: false,
            disabled: false,
            invalid: true,
        });
        snapshot.controls.push(PresetRuntimeControl {
            action: PresetRuntimeAction::SavePreset,
            parent: None,
            role: PresetRuntimeControlRole::Button,
            name: MessageId::PresetSaveAction,
            value: None,
            selected: false,
            disabled: true,
            invalid: false,
        });
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        assert!(nodes.iter().any(|node| {
            node.id == preset_runtime_control_semantic_node(PresetRuntimeAction::ConfirmRisk)
                && node.state.invalid
                && node.state.checked == Some(false)
        }));
        assert!(!snapshot.routes().iter().any(|(_, command)| {
            *command
                == PresetRuntimeAccessibilityCommand::ActivateControl(
                    PresetRuntimeAction::SavePreset,
                )
        }));
    }

    #[test]
    fn preset_runtime_surface_recording_mode_masks_user_values() {
        let mut snapshot = snapshot();
        snapshot.recording_friendly = true;
        let nodes = snapshot.try_nodes(semantic_id(9)).unwrap();
        let rendered = format!("{nodes:?}");
        assert!(!rendered.contains("Customer preset"));
        assert!(!rendered.contains("codex, 2 arguments"));
    }

    #[test]
    fn preset_runtime_surface_covers_state_locale_theme_and_scale_contracts() {
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            for state in PresetRuntimeSurfaceState::ALL {
                assert!(!localizer.format_static(state.message()).unwrap().is_empty());
            }
        }
        for theme in ThemeKind::ALL {
            assert!(DesignTokens::new(theme).focus_ring_width().0 >= 2.0);
        }
        assert_eq!(
            PresetRuntimeTextScale::try_new(100).unwrap().layout(),
            PresetRuntimeResponsiveLayout::ListAndForm
        );
        assert_eq!(
            PresetRuntimeTextScale::try_new(200).unwrap().layout(),
            PresetRuntimeResponsiveLayout::Compact
        );
        assert_eq!(
            PresetRuntimeTextScale::try_new(400).unwrap().layout(),
            PresetRuntimeResponsiveLayout::Stacked
        );
        assert!(PresetRuntimeTextScale::try_new(401).is_none());
    }

    #[test]
    fn preset_runtime_surface_selection_and_announcements_are_stable() {
        let first = PresetRuntimeRowId::preset(1);
        let second = PresetRuntimeRowId::preset(2);
        let result = reconcile_preset_runtime_selection(&[first, second], &[second], Some(first));
        assert_eq!(result.selected, Some(second));
        let mut coalescer = PresetRuntimeAnnouncementCoalescer::default();
        let start = Instant::now();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(coalescer.flush(start + PRESET_RUNTIME_ANNOUNCEMENT_INTERVAL));
    }

    #[test]
    fn preset_runtime_surface_rejects_duplicates_bad_parents_and_resource_exhaustion() {
        let mut duplicate = snapshot();
        duplicate.rows.push(preset_row(7));
        assert_eq!(
            duplicate.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::DuplicateNode
        );

        let mut bad_parent = snapshot();
        bad_parent.rows[0].parent = Some(PresetRuntimeRowId::runtime(2));
        assert_eq!(
            bad_parent.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );

        let mut oversized = snapshot();
        oversized.rows = (0..=MAX_PRESET_RUNTIME_ROWS)
            .map(|index| preset_row(index as u128 + 1))
            .collect();
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn runtime_and_capability_ids_are_stable_and_distinct() {
        assert_eq!(
            stable_runtime_row_value("codex"),
            stable_runtime_row_value("codex")
        );
        assert_ne!(
            stable_runtime_row_value("codex"),
            stable_runtime_row_value("claude")
        );
        assert_ne!(
            stable_capability_row_value("codex", MessageId::RuntimeCapabilityResume),
            stable_capability_row_value("codex", MessageId::RuntimeCapabilityCancellation)
        );
    }
}
