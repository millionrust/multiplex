use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    MessageId, SemanticAction, SemanticError, SemanticErrorCode, SemanticNode, SemanticNodeId,
    SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_PRODUCT_SESSION_ROWS: usize = 8_000;
pub const MAX_PRODUCT_SESSION_CONTROLS: usize = 8_000;
const MAX_PRODUCT_SESSION_NODES: usize = 9_980;
pub const PRODUCT_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const PRODUCT_ROOT_NODE: u64 = 100_000;
const PRODUCT_STATUS_NODE: u64 = 100_001;
const PRODUCT_LIST_NODE: u64 = 100_002;
const PRODUCT_DIALOG_NODE: u64 = 100_003;
const PRODUCT_DIALOG_SAFE_NODE: u64 = 100_004;
const PRODUCT_DIALOG_CONFIRM_NODE: u64 = 100_005;
const PRODUCT_ROW_NODE_BASE: u64 = 1_u64 << 62;
const PRODUCT_ROW_NODE_MASK: u64 = (1_u64 << 60) - 1;
const PRODUCT_CONTROL_NODE_BASE: u64 = 1_u64 << 61;
const PRODUCT_CONTROL_NODE_MASK: u64 = (1_u64 << 61) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AccessibleRowKind {
    Project,
    Group,
    Session,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AccessibleRowId {
    pub kind: AccessibleRowKind,
    pub value: u128,
}

impl AccessibleRowId {
    pub const fn project(value: u128) -> Self {
        Self {
            kind: AccessibleRowKind::Project,
            value,
        }
    }

    pub const fn group(value: u128) -> Self {
        Self {
            kind: AccessibleRowKind::Group,
            value,
        }
    }

    pub const fn session(value: u128) -> Self {
        Self {
            kind: AccessibleRowKind::Session,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HierarchyLevel {
    Project,
    Group,
    Session,
}

impl HierarchyLevel {
    pub const fn depth(self) -> u8 {
        match self {
            Self::Project => 1,
            Self::Group => 2,
            Self::Session => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessibleCollectionRow {
    pub id: AccessibleRowId,
    pub parent: Option<AccessibleRowId>,
    pub level: HierarchyLevel,
    pub name: String,
    pub status: MessageId,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub unread: bool,
    pub disabled: bool,
    pub position: usize,
    pub set_size: usize,
}

impl AccessibleCollectionRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > crate::MAX_SEMANTIC_TEXT_CHARS
            || self.position == 0
            || self.set_size == 0
            || self.position > self.set_size
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        let expected_kind = match self.level {
            HierarchyLevel::Project => AccessibleRowKind::Project,
            HierarchyLevel::Group => AccessibleRowKind::Group,
            HierarchyLevel::Session => AccessibleRowKind::Session,
        };
        if self.id.kind != expected_kind {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        let valid_parent = matches!(
            (self.level, self.parent.map(|parent| parent.kind)),
            (HierarchyLevel::Project, None)
                | (HierarchyLevel::Group, Some(AccessibleRowKind::Project))
                | (
                    HierarchyLevel::Session,
                    Some(AccessibleRowKind::Project | AccessibleRowKind::Group)
                )
        );
        if !valid_parent {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductSessionScreen {
    Projects,
    Sessions,
}

impl ProductSessionScreen {
    const fn title(self) -> MessageId {
        match self {
            Self::Projects => MessageId::ProjectsTitle,
            Self::Sessions => MessageId::SessionSidebarTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductSessionSurfaceState {
    Ready,
    Loading,
    Empty,
    FilterEmpty,
    Partial,
    Unavailable,
    Offline,
    PermissionDenied,
    StaleRevision,
    DiskLimit,
    Error,
    Cancelled,
    Recovery,
}

impl ProductSessionSurfaceState {
    pub const ALL: [Self; 13] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::FilterEmpty,
        Self::Partial,
        Self::Unavailable,
        Self::Offline,
        Self::PermissionDenied,
        Self::StaleRevision,
        Self::DiskLimit,
        Self::Error,
        Self::Cancelled,
        Self::Recovery,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::ProductSurfaceStateReady,
            Self::Loading => MessageId::ProductSurfaceStateLoading,
            Self::Empty => MessageId::ProductSurfaceStateEmpty,
            Self::FilterEmpty => MessageId::ProductSurfaceStateFilterEmpty,
            Self::Partial => MessageId::ProductSurfaceStatePartial,
            Self::Unavailable => MessageId::ProductSurfaceStateUnavailable,
            Self::Offline => MessageId::ProductSurfaceStateOffline,
            Self::PermissionDenied => MessageId::ProductSurfaceStatePermission,
            Self::StaleRevision => MessageId::ProductSurfaceStateStale,
            Self::DiskLimit => MessageId::ProductSurfaceStateDiskLimit,
            Self::Error => MessageId::ProductSurfaceStateError,
            Self::Cancelled => MessageId::ProductSurfaceStateCancelled,
            Self::Recovery => MessageId::ProductSurfaceStateRecovery,
        }
    }

    const fn is_busy(self) -> bool {
        matches!(self, Self::Loading)
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::Unavailable
                | Self::PermissionDenied
                | Self::StaleRevision
                | Self::DiskLimit
                | Self::Error
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DestructiveActionKind {
    RemoveProject,
    RemoveGroup,
    StopAndArchive,
    RemoveSessionData,
}

impl DestructiveActionKind {
    const fn title(self) -> MessageId {
        match self {
            Self::RemoveProject => MessageId::ProjectRemoveAction,
            Self::RemoveGroup => MessageId::GroupRemoveTitle,
            Self::StopAndArchive => MessageId::SessionLibraryStopArchiveAction,
            Self::RemoveSessionData => MessageId::SessionLibraryRemoveTitle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DestructiveActionPresentation {
    pub kind: DestructiveActionKind,
    pub target: AccessibleRowId,
    pub revision: u64,
    pub confirm_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductMoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductSessionAction {
    RetryProjects,
    AddProject,
    SetProjectName,
    ConfirmProjectAdd,
    CancelProjectAdd,
    RemoveProject(AccessibleRowId),
    UndoProjectRemoval,
    AddGroup(AccessibleRowId),
    RenameGroup(AccessibleRowId),
    SetGroupName,
    SaveGroup,
    CancelGroup,
    ToggleGroup(AccessibleRowId),
    MoveGroup(AccessibleRowId, ProductMoveDirection),
    RemoveGroup(AccessibleRowId),
    RemoveGroupTo(AccessibleRowId, Option<AccessibleRowId>),
    UndoOrganization,
    ShowActiveSessions,
    ShowArchivedSessions,
    FilterAllSessions,
    FilterUnreadSessions,
    FilterPinnedSessions,
    RenameSession(AccessibleRowId),
    SetSessionTitle,
    SaveSessionTitle,
    CancelSessionTitle,
    ToggleSessionPin(AccessibleRowId),
    ToggleSessionRead(AccessibleRowId),
    OpenSession(AccessibleRowId),
    StopSession(AccessibleRowId),
    ResumeSession(AccessibleRowId),
    ArchiveOrStopSession(AccessibleRowId),
    RestoreSession(AccessibleRowId),
    BeginSessionRemoval(AccessibleRowId),
    SetSessionRemovalConfirmation,
    MoveSession(AccessibleRowId, ProductMoveDirection),
    MoveSessionToRoot(AccessibleRowId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductControlRole {
    Button,
    TextField,
    Tab,
}

impl ProductControlRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Button => SemanticRole::Button,
            Self::TextField => SemanticRole::TextField,
            Self::Tab => SemanticRole::Tab,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductSessionControl {
    pub action: ProductSessionAction,
    pub parent: Option<AccessibleRowId>,
    pub role: ProductControlRole,
    pub name: MessageId,
    pub value: Option<String>,
    pub selected: bool,
    pub disabled: bool,
    pub in_dialog: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductTextScale(u16);

impl ProductTextScale {
    pub const fn try_new(percent: u16) -> Option<Self> {
        if percent >= 100 && percent <= 400 {
            Some(Self(percent))
        } else {
            None
        }
    }

    pub const fn layout(self) -> ProductResponsiveLayout {
        match self.0 {
            100..=150 => ProductResponsiveLayout::ListDetail,
            151..=200 => ProductResponsiveLayout::CompactListDetail,
            _ => ProductResponsiveLayout::Stacked,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductResponsiveLayout {
    ListDetail,
    CompactListDetail,
    Stacked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductSessionSemanticSnapshot {
    pub screen: ProductSessionScreen,
    pub state: ProductSessionSurfaceState,
    pub rows: Vec<AccessibleCollectionRow>,
    pub controls: Vec<ProductSessionControl>,
    pub dialog: Option<DestructiveActionPresentation>,
    pub recording_friendly: bool,
}

impl ProductSessionSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.rows.len() > MAX_PRODUCT_SESSION_ROWS
            || self.controls.len() > MAX_PRODUCT_SESSION_CONTROLS
            || self.rows.len() + self.controls.len() + 6 > MAX_PRODUCT_SESSION_NODES
        {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        let mut row_ids = HashSet::with_capacity(self.rows.len());
        let mut node_ids = HashSet::with_capacity(self.rows.len());
        let mut semantic_ids = HashMap::with_capacity(self.rows.len());
        for row in &self.rows {
            row.validate()?;
            if !row_ids.insert(row.id) {
                return Err(SemanticError::new(SemanticErrorCode::DuplicateNode, None));
            }
            let node = product_row_semantic_node(row.id);
            if !node_ids.insert(node) || semantic_ids.insert(row.id, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }
        let mut control_ids = HashMap::with_capacity(self.controls.len());
        let mut control_node_ids = HashSet::with_capacity(self.controls.len());
        for control in &self.controls {
            if control.value.as_ref().is_some_and(|value| {
                value.chars().count() > crate::MAX_SEMANTIC_ACTION_VALUE_CHARS
                    || value.chars().any(|character| character == '\0')
            }) {
                return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
            }
            let node = product_control_semantic_node(control.action);
            if !control_node_ids.insert(node)
                || node_ids.contains(&node)
                || control_ids.insert(control.action, node).is_some()
            {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(node),
                ));
            }
        }

        let root_id = product_root_semantic_node();
        let mut root = named_node(root_id, SemanticRole::Landmark, self.screen.title());
        root.parent = Some(parent);
        let dialog_open = self.dialog.is_some();
        root.state.busy = self.state.is_busy();

        let status_id = semantic_id(PRODUCT_STATUS_NODE);
        let mut status = named_node(status_id, SemanticRole::Status, self.state.message());
        status.parent = Some(root_id);
        status.state.live = Some(if self.state.is_error() {
            crate::LiveRegionPoliteness::Immediate
        } else {
            crate::LiveRegionPoliteness::Polite
        });

        let list_id = semantic_id(PRODUCT_LIST_NODE);
        let mut list = SemanticNode::new(list_id, SemanticRole::List);
        list.parent = Some(root_id);
        list.state.hidden = dialog_open;

        let mut nodes = vec![root, status, list];
        for row in &self.rows {
            let node_id = semantic_ids[&row.id];
            let mut node = SemanticNode::new(node_id, SemanticRole::ListItem);
            node.parent = match row.parent {
                Some(parent_id) => Some(*semantic_ids.get(&parent_id).ok_or_else(|| {
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
            node.value = Some(SemanticValue::Number {
                current: row.position as i64,
                minimum: 1,
                maximum: row.set_size as i64,
            });
            node.state = SemanticState {
                disabled: row.disabled,
                selected: row.selected,
                expanded: row.expanded,
                checked: row.unread.then_some(true),
                hidden: dialog_open,
                ..SemanticState::default()
            };
            if !dialog_open {
                node.actions.insert(SemanticAction::Focus);
                if !row.disabled {
                    node.actions.insert(SemanticAction::Activate);
                }
            }
            nodes.push(node);
        }

        for control in &self.controls {
            let node_id = control_ids[&control.action];
            let mut node = named_node(node_id, control.role.semantic_role(), control.name);
            node.parent = if control.in_dialog {
                if !dialog_open {
                    return Err(SemanticError::new(
                        SemanticErrorCode::MissingParent,
                        Some(node_id),
                    ));
                }
                Some(product_dialog_semantic_node())
            } else {
                match control.parent {
                    Some(parent_id) => Some(*semantic_ids.get(&parent_id).ok_or_else(|| {
                        SemanticError::new(SemanticErrorCode::MissingParent, Some(node_id))
                    })?),
                    None => Some(root_id),
                }
            };
            let hidden = dialog_open && !control.in_dialog;
            node.state.disabled = control.disabled;
            node.state.selected = control.selected;
            node.state.hidden = hidden;
            if let Some(value) = control.value.as_ref() {
                node.value = Some(if self.recording_friendly {
                    SemanticValue::PublicText(SemanticText::Message(private_control_value_message(
                        control.action,
                    )))
                } else {
                    SemanticValue::PublicText(SemanticText::user_text(bidi_isolate(value))?)
                });
            }
            if !hidden {
                node.actions.insert(SemanticAction::Focus);
                if !control.disabled {
                    match control.role {
                        ProductControlRole::Button | ProductControlRole::Tab => {
                            node.actions.insert(SemanticAction::Activate);
                        }
                        ProductControlRole::TextField => {
                            node.actions.insert(SemanticAction::SetValue);
                        }
                    }
                }
            }
            nodes.push(node);
        }

        if let Some(dialog) = self.dialog {
            let dialog_id = product_dialog_semantic_node();
            let mut node = named_node(dialog_id, SemanticRole::Dialog, dialog.kind.title());
            node.parent = Some(root_id);
            node.actions
                .extend([SemanticAction::Dismiss, SemanticAction::Cancel]);
            nodes.push(node);

            let safe_id = product_dialog_safe_semantic_node();
            let mut safe = named_node(safe_id, SemanticRole::Button, MessageId::CommonCancel);
            safe.parent = Some(dialog_id);
            safe.actions
                .extend([SemanticAction::Focus, SemanticAction::Activate]);
            nodes.push(safe);

            let confirm_id = product_dialog_confirm_semantic_node();
            let mut confirm = named_node(confirm_id, SemanticRole::Button, dialog.kind.title());
            confirm.parent = Some(dialog_id);
            confirm.state.disabled = !dialog.confirm_enabled;
            confirm.actions.insert(SemanticAction::Focus);
            if dialog.confirm_enabled {
                confirm.actions.insert(SemanticAction::Activate);
            }
            nodes.push(confirm);
        }
        Ok(nodes)
    }

    pub fn routes(
        &self,
    ) -> Vec<(
        (SemanticNodeId, SemanticAction),
        ProductSessionAccessibilityCommand,
    )> {
        let mut routes = Vec::with_capacity(self.rows.len() * 2 + self.controls.len() * 2 + 6);
        if self.dialog.is_none() {
            for row in &self.rows {
                let node = product_row_semantic_node(row.id);
                routes.push((
                    (node, SemanticAction::Focus),
                    ProductSessionAccessibilityCommand::FocusRow(row.id),
                ));
                if !row.disabled {
                    routes.push((
                        (node, SemanticAction::Activate),
                        ProductSessionAccessibilityCommand::ActivateRow(row.id),
                    ));
                }
            }
        }
        for control in &self.controls {
            if self.dialog.is_some() && !control.in_dialog {
                continue;
            }
            let node = product_control_semantic_node(control.action);
            routes.push((
                (node, SemanticAction::Focus),
                ProductSessionAccessibilityCommand::FocusControl(control.action),
            ));
            if !control.disabled {
                let (semantic_action, command) = match control.role {
                    ProductControlRole::Button | ProductControlRole::Tab => (
                        SemanticAction::Activate,
                        ProductSessionAccessibilityCommand::ActivateControl(control.action),
                    ),
                    ProductControlRole::TextField => (
                        SemanticAction::SetValue,
                        ProductSessionAccessibilityCommand::SetControlValue(control.action),
                    ),
                };
                routes.push(((node, semantic_action), command));
            }
        }
        if self.dialog.is_some() {
            routes.extend([
                (
                    (product_dialog_semantic_node(), SemanticAction::Dismiss),
                    ProductSessionAccessibilityCommand::CancelDialog,
                ),
                (
                    (product_dialog_semantic_node(), SemanticAction::Cancel),
                    ProductSessionAccessibilityCommand::CancelDialog,
                ),
                (
                    (product_dialog_safe_semantic_node(), SemanticAction::Focus),
                    ProductSessionAccessibilityCommand::FocusSafeAction,
                ),
                (
                    (
                        product_dialog_safe_semantic_node(),
                        SemanticAction::Activate,
                    ),
                    ProductSessionAccessibilityCommand::CancelDialog,
                ),
            ]);
            if self.dialog.is_some_and(|dialog| dialog.confirm_enabled) {
                routes.extend([
                    (
                        (
                            product_dialog_confirm_semantic_node(),
                            SemanticAction::Focus,
                        ),
                        ProductSessionAccessibilityCommand::FocusConfirmAction,
                    ),
                    (
                        (
                            product_dialog_confirm_semantic_node(),
                            SemanticAction::Activate,
                        ),
                        ProductSessionAccessibilityCommand::ConfirmDialog,
                    ),
                ]);
            } else {
                routes.push((
                    (
                        product_dialog_confirm_semantic_node(),
                        SemanticAction::Focus,
                    ),
                    ProductSessionAccessibilityCommand::FocusConfirmAction,
                ));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductSessionAccessibilityCommand {
    FocusRow(AccessibleRowId),
    ActivateRow(AccessibleRowId),
    FocusControl(ProductSessionAction),
    ActivateControl(ProductSessionAction),
    SetControlValue(ProductSessionAction),
    FocusSafeAction,
    FocusConfirmAction,
    ConfirmDialog,
    CancelDialog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionReconciliation {
    pub selected: Option<AccessibleRowId>,
    pub focus_heading: bool,
}

pub fn reconcile_collection_selection(
    previous: &[AccessibleRowId],
    next: &[AccessibleRowId],
    selected: Option<AccessibleRowId>,
) -> SelectionReconciliation {
    let Some(selected) = selected else {
        return SelectionReconciliation {
            selected: None,
            focus_heading: next.is_empty(),
        };
    };
    if next.contains(&selected) {
        return SelectionReconciliation {
            selected: Some(selected),
            focus_heading: false,
        };
    }
    let previous_index = previous
        .iter()
        .position(|candidate| *candidate == selected)
        .unwrap_or(0);
    SelectionReconciliation {
        selected: next
            .get(previous_index.min(next.len().saturating_sub(1)))
            .copied(),
        focus_heading: next.is_empty(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProductAnnouncementCoalescer {
    last: Option<Instant>,
    pending: bool,
}

impl ProductAnnouncementCoalescer {
    pub fn request(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= PRODUCT_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            true
        } else {
            self.pending = true;
            false
        }
    }

    pub fn flush(&mut self, now: Instant) -> bool {
        if self.pending
            && self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= PRODUCT_ANNOUNCEMENT_INTERVAL
            })
        {
            self.pending = false;
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

pub fn product_root_semantic_node() -> SemanticNodeId {
    semantic_id(PRODUCT_ROOT_NODE)
}

pub fn product_dialog_semantic_node() -> SemanticNodeId {
    semantic_id(PRODUCT_DIALOG_NODE)
}

pub fn product_dialog_safe_semantic_node() -> SemanticNodeId {
    semantic_id(PRODUCT_DIALOG_SAFE_NODE)
}

pub fn product_dialog_confirm_semantic_node() -> SemanticNodeId {
    semantic_id(PRODUCT_DIALOG_CONFIRM_NODE)
}

fn product_row_semantic_node(id: AccessibleRowId) -> SemanticNodeId {
    let folded = (id.value as u64) ^ ((id.value >> 64) as u64);
    let kind = match id.kind {
        AccessibleRowKind::Project => 0_u64,
        AccessibleRowKind::Group => 1_u64,
        AccessibleRowKind::Session => 2_u64,
    };
    semantic_id(PRODUCT_ROW_NODE_BASE | (kind << 60) | (folded & PRODUCT_ROW_NODE_MASK))
}

fn product_control_semantic_node(action: ProductSessionAction) -> SemanticNodeId {
    let mut hasher = StableActionHasher::default();
    action.hash(&mut hasher);
    semantic_id(PRODUCT_CONTROL_NODE_BASE | (hasher.finish() & PRODUCT_CONTROL_NODE_MASK))
}

struct StableActionHasher(u64);

impl Default for StableActionHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for StableActionHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

fn private_row_message(kind: AccessibleRowKind) -> MessageId {
    match kind {
        AccessibleRowKind::Project => MessageId::ProductPrivateProjectRow,
        AccessibleRowKind::Group => MessageId::ProductPrivateGroupRow,
        AccessibleRowKind::Session => MessageId::ProductPrivateSessionRow,
    }
}

fn private_control_value_message(action: ProductSessionAction) -> MessageId {
    match action {
        ProductSessionAction::SetProjectName => MessageId::ProductPrivateProjectRow,
        ProductSessionAction::SetGroupName | ProductSessionAction::RemoveGroupTo(_, _) => {
            MessageId::ProductPrivateGroupRow
        }
        _ => MessageId::ProductPrivateSessionRow,
    }
}

fn bidi_isolate(value: &str) -> String {
    format!("\u{2068}{value}\u{2069}")
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("product semantic IDs are non-zero"))
}

fn named_node(id: SemanticNodeId, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(id, role);
    node.name = Some(SemanticText::Message(name));
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignTokens, Locale, Localizer, SemanticTree, ThemeKind};

    fn row(id: AccessibleRowId, level: HierarchyLevel, name: &str) -> AccessibleCollectionRow {
        AccessibleCollectionRow {
            id,
            parent: None,
            level,
            name: name.to_string(),
            status: MessageId::ProductSurfaceStateReady,
            selected: false,
            expanded: None,
            unread: false,
            disabled: false,
            position: 1,
            set_size: 1,
        }
    }

    fn control(action: ProductSessionAction, role: ProductControlRole) -> ProductSessionControl {
        ProductSessionControl {
            action,
            parent: None,
            role,
            name: MessageId::CommonOpen,
            value: None,
            selected: false,
            disabled: false,
            in_dialog: false,
        }
    }

    #[test]
    fn hierarchy_semantics_preserve_names_state_position_and_actions() {
        let project_id = AccessibleRowId::project(1);
        let group_id = AccessibleRowId::group(2);
        let session_id = AccessibleRowId::session(3);
        let project = row(project_id, HierarchyLevel::Project, "Project Alpha");
        let mut group = row(group_id, HierarchyLevel::Group, "Review");
        group.parent = Some(project_id);
        group.expanded = Some(true);
        let mut session = row(session_id, HierarchyLevel::Session, "Agent one");
        session.parent = Some(group_id);
        session.selected = true;
        session.unread = true;
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![project, group, session],
            controls: Vec::new(),
            dialog: None,
            recording_friendly: false,
        };
        let shell = semantic_id(1);
        let mut nodes = vec![SemanticNode::new(shell, SemanticRole::Application)];
        nodes.extend(snapshot.try_nodes(shell).unwrap());
        let tree = SemanticTree::try_new(7, shell, nodes).unwrap();
        let session = tree.node(product_row_semantic_node(session_id)).unwrap();
        assert!(session.state.selected);
        assert_eq!(session.state.checked, Some(true));
        assert!(session.actions.contains(&SemanticAction::Activate));
        assert_eq!(session.parent, Some(product_row_semantic_node(group_id)));
    }

    #[test]
    fn recording_friendly_semantics_mask_all_user_titles() {
        let project_id = AccessibleRowId::project(8);
        let mut session = row(
            AccessibleRowId::session(9),
            HierarchyLevel::Session,
            "secret/customer/path",
        );
        session.parent = Some(project_id);
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Sessions,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![
                row(project_id, HierarchyLevel::Project, "Private project"),
                session,
            ],
            controls: Vec::new(),
            dialog: None,
            recording_friendly: true,
        };
        assert!(
            snapshot
                .try_nodes(semantic_id(1))
                .unwrap()
                .iter()
                .all(|node| {
                    !matches!(node.name, Some(SemanticText::UserText(_)))
                        && !matches!(node.description, Some(SemanticText::UserText(_)))
                })
        );
    }

    #[test]
    fn destructive_dialog_exposes_only_safe_default_until_reviewed() {
        let project = AccessibleRowId::project(4);
        let target = AccessibleRowId::session(5);
        let mut session = row(target, HierarchyLevel::Session, "Build");
        session.parent = Some(project);
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Sessions,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![row(project, HierarchyLevel::Project, "Project"), session],
            controls: Vec::new(),
            dialog: Some(DestructiveActionPresentation {
                kind: DestructiveActionKind::RemoveSessionData,
                target,
                revision: 4,
                confirm_enabled: false,
            }),
            recording_friendly: false,
        };
        let nodes = snapshot.try_nodes(semantic_id(1)).unwrap();
        let dialog = nodes
            .iter()
            .find(|node| node.id == product_dialog_semantic_node())
            .unwrap();
        assert_eq!(dialog.role, SemanticRole::Dialog);
        let background_row = nodes
            .iter()
            .find(|node| node.id == product_row_semantic_node(target))
            .unwrap();
        assert!(background_row.state.hidden);
        assert!(background_row.actions.is_empty());
        let routes = snapshot.routes();
        assert!(
            routes.iter().any(|(_, command)| {
                *command == ProductSessionAccessibilityCommand::CancelDialog
            })
        );
        assert!(!routes.iter().any(|(_, command)| {
            matches!(command, ProductSessionAccessibilityCommand::ActivateRow(id) if *id == target)
        }));
        assert!(
            !routes.iter().any(|(_, command)| {
                *command == ProductSessionAccessibilityCommand::ConfirmDialog
            })
        );

        let enabled = ProductSessionSemanticSnapshot {
            dialog: snapshot.dialog.map(|mut dialog| {
                dialog.confirm_enabled = true;
                dialog
            }),
            ..snapshot
        };
        assert!(
            enabled.routes().iter().any(|(_, command)| {
                *command == ProductSessionAccessibilityCommand::ConfirmDialog
            })
        );
    }

    #[test]
    fn editable_controls_route_values_and_mask_private_content() {
        let mut field = control(
            ProductSessionAction::SetProjectName,
            ProductControlRole::TextField,
        );
        field.name = MessageId::ProjectLabelField;
        field.value = Some("customer-secret".to_string());
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: Vec::new(),
            controls: vec![field],
            dialog: None,
            recording_friendly: false,
        };
        let node = snapshot
            .try_nodes(semantic_id(1))
            .unwrap()
            .into_iter()
            .find(|node| {
                node.id == product_control_semantic_node(ProductSessionAction::SetProjectName)
            })
            .unwrap();
        assert!(node.actions.contains(&SemanticAction::SetValue));
        assert!(matches!(
            node.value,
            Some(SemanticValue::PublicText(SemanticText::UserText(ref value)))
                if value.contains("customer-secret")
        ));
        assert!(snapshot.routes().iter().any(|(_, command)| {
            *command
                == ProductSessionAccessibilityCommand::SetControlValue(
                    ProductSessionAction::SetProjectName,
                )
        }));

        let masked = ProductSessionSemanticSnapshot {
            recording_friendly: true,
            ..snapshot
        };
        let node = masked
            .try_nodes(semantic_id(1))
            .unwrap()
            .into_iter()
            .find(|node| {
                node.id == product_control_semantic_node(ProductSessionAction::SetProjectName)
            })
            .unwrap();
        assert_eq!(
            node.value,
            Some(SemanticValue::PublicText(SemanticText::Message(
                MessageId::ProductPrivateProjectRow,
            )))
        );
    }

    #[test]
    fn modal_controls_hide_background_actions_and_keep_review_choices_available() {
        let project = AccessibleRowId::project(21);
        let group = AccessibleRowId::group(22);
        let mut group_row = row(group, HierarchyLevel::Group, "Source");
        group_row.parent = Some(project);
        let mut background = control(
            ProductSessionAction::AddGroup(project),
            ProductControlRole::Button,
        );
        background.parent = Some(project);
        let mut review = control(
            ProductSessionAction::RemoveGroupTo(group, None),
            ProductControlRole::Button,
        );
        review.in_dialog = true;
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![row(project, HierarchyLevel::Project, "Project"), group_row],
            controls: vec![background, review],
            dialog: Some(DestructiveActionPresentation {
                kind: DestructiveActionKind::RemoveGroup,
                target: group,
                revision: 9,
                confirm_enabled: false,
            }),
            recording_friendly: false,
        };
        let nodes = snapshot.try_nodes(semantic_id(1)).unwrap();
        let background = nodes
            .iter()
            .find(|node| {
                node.id == product_control_semantic_node(ProductSessionAction::AddGroup(project))
            })
            .unwrap();
        assert!(background.state.hidden);
        assert!(background.actions.is_empty());
        let review = nodes
            .iter()
            .find(|node| {
                node.id
                    == product_control_semantic_node(ProductSessionAction::RemoveGroupTo(
                        group, None,
                    ))
            })
            .unwrap();
        assert!(!review.state.hidden);
        assert!(review.actions.contains(&SemanticAction::Activate));
    }

    #[test]
    fn selection_reconciliation_preserves_identity_then_chooses_neighbor_or_heading() {
        let a = AccessibleRowId::project(1);
        let b = AccessibleRowId::project(2);
        let c = AccessibleRowId::project(3);
        assert_eq!(
            reconcile_collection_selection(&[a, b, c], &[c, b, a], Some(b)).selected,
            Some(b)
        );
        assert_eq!(
            reconcile_collection_selection(&[a, b, c], &[a, c], Some(b)).selected,
            Some(c)
        );
        assert!(reconcile_collection_selection(&[a], &[], Some(a)).focus_heading);
    }

    #[test]
    fn every_state_locale_theme_and_required_scale_has_a_complete_contract() {
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            for state in ProductSessionSurfaceState::ALL {
                assert!(!localizer.format_static(state.message()).unwrap().is_empty());
            }
        }
        for theme in ThemeKind::ALL {
            let tokens = DesignTokens::new(theme);
            assert!(tokens.color_bg_surface().alpha > 0);
            assert!(tokens.color_focus().alpha > 0);
        }
        assert_eq!(
            ProductTextScale::try_new(200).unwrap().layout(),
            ProductResponsiveLayout::CompactListDetail
        );
        assert_eq!(
            ProductTextScale::try_new(400).unwrap().layout(),
            ProductResponsiveLayout::Stacked
        );
        assert!(ProductTextScale::try_new(99).is_none());
    }

    #[test]
    fn loading_and_reorder_announcements_coalesce_but_final_state_is_immediate() {
        let start = Instant::now();
        let mut coalescer = ProductAnnouncementCoalescer::default();
        assert!(coalescer.request(start, false));
        assert!(!coalescer.request(start + Duration::from_millis(5), false));
        assert!(!coalescer.flush(start + Duration::from_millis(249)));
        assert!(coalescer.flush(start + Duration::from_millis(250)));
        assert!(coalescer.request(start + Duration::from_millis(251), true));
    }

    #[test]
    fn malformed_hierarchy_and_resource_overflow_fail_closed() {
        let mut child = row(
            AccessibleRowId::session(2),
            HierarchyLevel::Session,
            "Child",
        );
        child.parent = Some(AccessibleRowId::group(999));
        let snapshot = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Sessions,
            state: ProductSessionSurfaceState::Error,
            rows: vec![child],
            controls: Vec::new(),
            dialog: None,
            recording_friendly: false,
        };
        assert_eq!(
            snapshot.try_nodes(semantic_id(1)).unwrap_err().code,
            SemanticErrorCode::MissingParent
        );

        let project = row(
            AccessibleRowId::project(1),
            HierarchyLevel::Project,
            "Project",
        );
        let overflow = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![project; MAX_PRODUCT_SESSION_ROWS + 1],
            controls: Vec::new(),
            dialog: None,
            recording_friendly: false,
        };
        assert_eq!(
            overflow.try_nodes(semantic_id(1)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );

        let duplicate = control(ProductSessionAction::AddProject, ProductControlRole::Button);
        let duplicate_controls = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: Vec::new(),
            controls: vec![duplicate.clone(), duplicate.clone()],
            dialog: None,
            recording_friendly: false,
        };
        assert_eq!(
            duplicate_controls
                .try_nodes(semantic_id(1))
                .unwrap_err()
                .code,
            SemanticErrorCode::DuplicateNode
        );

        let control_overflow = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Projects,
            state: ProductSessionSurfaceState::Ready,
            rows: Vec::new(),
            controls: vec![duplicate; MAX_PRODUCT_SESSION_CONTROLS + 1],
            dialog: None,
            recording_friendly: false,
        };
        assert_eq!(
            control_overflow.try_nodes(semantic_id(1)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }
}
