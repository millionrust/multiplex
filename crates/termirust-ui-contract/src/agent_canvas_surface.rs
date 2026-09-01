use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroU64;

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticBounds, SemanticError,
    SemanticErrorCode, SemanticNode, SemanticNodeId, SemanticRelation, SemanticRelationKind,
    SemanticRole, SemanticState, SemanticText,
};

pub const MAX_CANVAS_NODES: usize = 1_000;
pub const MAX_CANVAS_EDGES: usize = 4_000;
pub const MAX_CANVAS_ACTIONS_PER_NODE: usize = 16;

const ROOT_NODE: u64 = 210_000;
const STATUS_NODE: u64 = 210_001;
const GRAPH_TAB_NODE: u64 = 210_002;
const LIST_TAB_NODE: u64 = 210_003;
const NODE_LIST: u64 = 210_004;
const EDGE_LIST: u64 = 210_005;
const ROW_NODE_BASE: u64 = 14_u64 << 60;
const EDGE_NODE_BASE: u64 = 15_u64 << 60;
const ACTION_NODE_BASE: u64 = 11_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 52) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanvasNodeSemanticId(u64);

impl CanvasNodeSemanticId {
    pub fn from_stable_key(value: &str) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in value.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self((hash & NODE_MASK).max(1))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanvasEdgeSemanticId(u64);

impl CanvasEdgeSemanticId {
    pub fn from_stable_key(value: &str) -> Self {
        let node = CanvasNodeSemanticId::from_stable_key(value);
        Self(node.get())
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneFocusPath {
    pub workspace_id: u64,
    pub node_id: CanvasNodeSemanticId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CanvasAlternativeNodeKind {
    Terminal,
    Agent,
    Note,
    Group,
}

impl CanvasAlternativeNodeKind {
    const fn message(self) -> MessageId {
        match self {
            Self::Terminal => MessageId::AgentCanvasNodeTerminal,
            Self::Agent => MessageId::AgentCanvasNodeAgent,
            Self::Note => MessageId::AgentCanvasNodeNote,
            Self::Group => MessageId::AgentCanvasNodeGroup,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CanvasAlternativeNodeState {
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
    Disconnected,
    Editing,
    Saved,
    Frame,
    Error,
}

impl CanvasAlternativeNodeState {
    const fn message(self) -> MessageId {
        match self {
            Self::Idle => MessageId::AgentCanvasNodeStateIdle,
            Self::Running => MessageId::AgentCanvasNodeStateRunning,
            Self::Succeeded => MessageId::AgentCanvasNodeStateSucceeded,
            Self::Failed => MessageId::AgentCanvasNodeStateFailed,
            Self::Cancelled => MessageId::AgentCanvasNodeStateCancelled,
            Self::Blocked => MessageId::AgentCanvasNodeStateBlocked,
            Self::Disconnected => MessageId::AgentCanvasNodeStateDisconnected,
            Self::Editing => MessageId::AgentCanvasNodeStateEditing,
            Self::Saved => MessageId::AgentCanvasNodeStateSaved,
            Self::Frame => MessageId::AgentCanvasNodeStateFrame,
            Self::Error => MessageId::AgentCanvasNodeStateError,
        }
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Blocked | Self::Disconnected | Self::Error
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CanvasNodeAction {
    Open,
    OpenMenu,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Rename,
    ToggleCollapsed,
    Remove,
}

impl CanvasNodeAction {
    #[cfg(test)]
    const ALL: [Self; 9] = [
        Self::Open,
        Self::OpenMenu,
        Self::MoveUp,
        Self::MoveDown,
        Self::MoveLeft,
        Self::MoveRight,
        Self::Rename,
        Self::ToggleCollapsed,
        Self::Remove,
    ];

    const fn message(self) -> MessageId {
        match self {
            Self::Open => MessageId::AgentCanvasActionOpen,
            Self::OpenMenu => MessageId::AgentCanvasActionMenu,
            Self::MoveUp => MessageId::AgentCanvasActionMoveUp,
            Self::MoveDown => MessageId::AgentCanvasActionMoveDown,
            Self::MoveLeft => MessageId::AgentCanvasActionMoveLeft,
            Self::MoveRight => MessageId::AgentCanvasActionMoveRight,
            Self::Rename => MessageId::AgentCanvasActionRename,
            Self::ToggleCollapsed => MessageId::AgentCanvasActionToggleCollapsed,
            Self::Remove => MessageId::AgentCanvasActionRemove,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasAlternativeRow {
    pub id: CanvasNodeSemanticId,
    pub explicit_order: Option<u32>,
    pub kind: CanvasAlternativeNodeKind,
    pub state: CanvasAlternativeNodeState,
    pub title: Option<String>,
    pub parent: Option<CanvasNodeSemanticId>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub selected: bool,
    pub collapsed: bool,
    pub actions: BTreeSet<CanvasNodeAction>,
}

impl CanvasAlternativeRow {
    fn validate(&self) -> Result<(), SemanticError> {
        if self.actions.len() > MAX_CANVAS_ACTIONS_PER_NODE {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        if let Some(title) = &self.title {
            SemanticText::user_text(title.clone())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasAlternativeEdgeKind {
    Context,
    Dependency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanvasAlternativeEdge {
    pub id: CanvasEdgeSemanticId,
    pub source: CanvasNodeSemanticId,
    pub target: CanvasNodeSemanticId,
    pub kind: CanvasAlternativeEdgeKind,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCanvasSurfaceState {
    Ready,
    Loading,
    Empty,
    Partial,
    Offline,
    PermissionDenied,
    Error,
    Recovery,
}

impl AgentCanvasSurfaceState {
    pub const ALL: [Self; 8] = [
        Self::Ready,
        Self::Loading,
        Self::Empty,
        Self::Partial,
        Self::Offline,
        Self::PermissionDenied,
        Self::Error,
        Self::Recovery,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::AgentCanvasStateReady,
            Self::Loading => MessageId::AgentCanvasStateLoading,
            Self::Empty => MessageId::AgentCanvasStateEmpty,
            Self::Partial => MessageId::AgentCanvasStatePartial,
            Self::Offline => MessageId::AgentCanvasStateOffline,
            Self::PermissionDenied => MessageId::AgentCanvasStatePermissionDenied,
            Self::Error => MessageId::AgentCanvasStateError,
            Self::Recovery => MessageId::AgentCanvasStateRecovery,
        }
    }

    const fn busy(self) -> bool {
        matches!(self, Self::Loading)
    }

    const fn error(self) -> bool {
        matches!(
            self,
            Self::Partial | Self::Offline | Self::PermissionDenied | Self::Error | Self::Recovery
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCanvasPresentationMode {
    Graph,
    ListInspector,
}

pub const fn agent_canvas_presentation_mode(
    scale_percent: u16,
) -> Option<AgentCanvasPresentationMode> {
    match scale_percent {
        100..=200 => Some(AgentCanvasPresentationMode::Graph),
        201..=400 => Some(AgentCanvasPresentationMode::ListInspector),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCanvasSemanticSnapshot {
    pub generation: u64,
    pub revision: u64,
    pub workspace_id: u64,
    pub state: AgentCanvasSurfaceState,
    pub mode: AgentCanvasPresentationMode,
    pub recording_friendly: bool,
    pub focused: Option<CanvasNodeSemanticId>,
    pub rows: Vec<CanvasAlternativeRow>,
    pub edges: Vec<CanvasAlternativeEdge>,
}

impl AgentCanvasSemanticSnapshot {
    pub fn ordered_rows(&self) -> Result<Vec<&CanvasAlternativeRow>, SemanticError> {
        self.validate()?;
        let mut rows = self.rows.iter().collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.explicit_order
                .is_none()
                .cmp(&right.explicit_order.is_none())
                .then_with(|| left.explicit_order.cmp(&right.explicit_order))
                .then_with(|| left.y.cmp(&right.y))
                .then_with(|| left.x.cmp(&right.x))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(rows)
    }

    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        let rows = self.ordered_rows()?;
        let compact_actions = self.rows.len() > 500;
        let root_id = semantic_id(ROOT_NODE);
        let mut root = named_node(root_id, SemanticRole::Landmark, MessageId::AgentCanvasTitle);
        root.parent = Some(parent);
        root.state.busy = self.state.busy();

        let mut status = named_node(
            semantic_id(STATUS_NODE),
            if self.state.error() {
                SemanticRole::Alert
            } else {
                SemanticRole::Status
            },
            self.state.message(),
        );
        status.parent = Some(root_id);
        status.state.live = Some(if self.state.error() {
            LiveRegionPoliteness::Immediate
        } else {
            LiveRegionPoliteness::Polite
        });

        let mut graph_tab = named_node(
            semantic_id(GRAPH_TAB_NODE),
            SemanticRole::Tab,
            MessageId::AgentCanvasGraphView,
        );
        graph_tab.parent = Some(root_id);
        graph_tab.state.selected = self.mode == AgentCanvasPresentationMode::Graph;
        graph_tab
            .actions
            .extend([SemanticAction::Focus, SemanticAction::Activate]);

        let mut list_tab = named_node(
            semantic_id(LIST_TAB_NODE),
            SemanticRole::Tab,
            MessageId::AgentCanvasListView,
        );
        list_tab.parent = Some(root_id);
        list_tab.state.selected = self.mode == AgentCanvasPresentationMode::ListInspector;
        list_tab
            .actions
            .extend([SemanticAction::Focus, SemanticAction::Activate]);

        let mut list = named_node(
            semantic_id(NODE_LIST),
            SemanticRole::List,
            MessageId::AgentCanvasNodeList,
        );
        list.parent = Some(root_id);
        let mut nodes = vec![root, status, graph_tab, list_tab, list];

        for row in rows {
            let id = row_semantic_node(row.id);
            let mut node = SemanticNode::new(id, SemanticRole::ListItem);
            node.parent = Some(
                row.parent
                    .map(row_semantic_node)
                    .unwrap_or(semantic_id(NODE_LIST)),
            );
            node.name = Some(if self.recording_friendly {
                SemanticText::Message(MessageId::AgentCanvasPrivateNode)
            } else if let Some(title) = &row.title {
                SemanticText::user_text(title.clone())?
            } else {
                SemanticText::Message(row.kind.message())
            });
            node.description = Some(SemanticText::Message(row.state.message()));
            node.bounds = SemanticBounds {
                x: row.x,
                y: row.y,
                width: row.width,
                height: row.height,
            };
            node.state = SemanticState {
                selected: row.selected || self.focused == Some(row.id),
                expanded: Some(!row.collapsed),
                invalid: row.state.is_error(),
                ..SemanticState::default()
            };
            node.actions
                .extend([SemanticAction::Focus, SemanticAction::Activate]);
            nodes.push(node);

            for action in exposed_actions(row, compact_actions) {
                let mut control = named_node(
                    action_semantic_node(row.id, action),
                    SemanticRole::Button,
                    action.message(),
                );
                control.parent = Some(id);
                control
                    .actions
                    .extend([SemanticAction::Focus, SemanticAction::Activate]);
                nodes.push(control);
            }
        }

        if !self.edges.is_empty() {
            let mut edge_list = named_node(
                semantic_id(EDGE_LIST),
                SemanticRole::List,
                MessageId::AgentCanvasEdgeList,
            );
            edge_list.parent = Some(root_id);
            nodes.push(edge_list);
            for edge in &self.edges {
                let mut node = named_node(
                    edge_semantic_node(edge.id),
                    SemanticRole::ListItem,
                    match edge.kind {
                        CanvasAlternativeEdgeKind::Context => MessageId::AgentCanvasEdgeContext,
                        CanvasAlternativeEdgeKind::Dependency => {
                            MessageId::AgentCanvasEdgeDependency
                        }
                    },
                );
                node.parent = Some(semantic_id(EDGE_LIST));
                node.state.disabled = !edge.enabled;
                node.relations.extend([
                    SemanticRelation {
                        kind: SemanticRelationKind::LabelledBy,
                        target: row_semantic_node(edge.source),
                    },
                    SemanticRelation {
                        kind: SemanticRelationKind::Controls,
                        target: row_semantic_node(edge.target),
                    },
                ]);
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    pub fn routes(&self) -> Result<Vec<AgentCanvasActionRoute>, SemanticError> {
        self.validate()?;
        let compact_actions = self.rows.len() > 500;
        let mut routes = vec![
            (
                (semantic_id(GRAPH_TAB_NODE), SemanticAction::Focus),
                AgentCanvasAccessibilityCommand::SetMode(AgentCanvasPresentationMode::Graph),
            ),
            (
                (semantic_id(GRAPH_TAB_NODE), SemanticAction::Activate),
                AgentCanvasAccessibilityCommand::SetMode(AgentCanvasPresentationMode::Graph),
            ),
            (
                (semantic_id(LIST_TAB_NODE), SemanticAction::Focus),
                AgentCanvasAccessibilityCommand::SetMode(
                    AgentCanvasPresentationMode::ListInspector,
                ),
            ),
            (
                (semantic_id(LIST_TAB_NODE), SemanticAction::Activate),
                AgentCanvasAccessibilityCommand::SetMode(
                    AgentCanvasPresentationMode::ListInspector,
                ),
            ),
        ];
        for row in &self.rows {
            let node = row_semantic_node(row.id);
            routes.push((
                (node, SemanticAction::Focus),
                AgentCanvasAccessibilityCommand::FocusNode(row.id),
            ));
            routes.push((
                (node, SemanticAction::Activate),
                AgentCanvasAccessibilityCommand::OpenNode(row.id),
            ));
            for action in exposed_actions(row, compact_actions) {
                let command = match action {
                    CanvasNodeAction::Open => AgentCanvasAccessibilityCommand::OpenNode(row.id),
                    CanvasNodeAction::OpenMenu => {
                        AgentCanvasAccessibilityCommand::OpenNodeMenu(row.id)
                    }
                    CanvasNodeAction::MoveUp => AgentCanvasAccessibilityCommand::MoveNode {
                        node: row.id,
                        direction: CanvasMoveDirection::Up,
                        expected_revision: self.revision,
                    },
                    CanvasNodeAction::MoveDown => AgentCanvasAccessibilityCommand::MoveNode {
                        node: row.id,
                        direction: CanvasMoveDirection::Down,
                        expected_revision: self.revision,
                    },
                    CanvasNodeAction::MoveLeft => AgentCanvasAccessibilityCommand::MoveNode {
                        node: row.id,
                        direction: CanvasMoveDirection::Left,
                        expected_revision: self.revision,
                    },
                    CanvasNodeAction::MoveRight => AgentCanvasAccessibilityCommand::MoveNode {
                        node: row.id,
                        direction: CanvasMoveDirection::Right,
                        expected_revision: self.revision,
                    },
                    CanvasNodeAction::Rename => AgentCanvasAccessibilityCommand::RenameNode(row.id),
                    CanvasNodeAction::ToggleCollapsed => {
                        AgentCanvasAccessibilityCommand::ToggleCollapsed {
                            node: row.id,
                            expected_revision: self.revision,
                        }
                    }
                    CanvasNodeAction::Remove => AgentCanvasAccessibilityCommand::RequestRemove {
                        node: row.id,
                        expected_revision: self.revision,
                    },
                };
                routes.push((
                    (action_semantic_node(row.id, action), SemanticAction::Focus),
                    AgentCanvasAccessibilityCommand::FocusNode(row.id),
                ));
                routes.push((
                    (
                        action_semantic_node(row.id, action),
                        SemanticAction::Activate,
                    ),
                    command,
                ));
            }
        }
        Ok(routes)
    }

    fn validate(&self) -> Result<(), SemanticError> {
        if self.rows.len() > MAX_CANVAS_NODES || self.edges.len() > MAX_CANVAS_EDGES {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        let mut ids = HashSet::with_capacity(self.rows.len());
        for row in &self.rows {
            row.validate()?;
            if !ids.insert(row.id) {
                return Err(SemanticError::new(SemanticErrorCode::DuplicateNode, None));
            }
        }
        if self.focused.is_some_and(|focused| !ids.contains(&focused)) {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        for row in &self.rows {
            if row
                .parent
                .is_some_and(|parent| !ids.contains(&parent) || parent == row.id)
            {
                return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
            }
        }
        let parents = self
            .rows
            .iter()
            .filter_map(|row| row.parent.map(|parent| (row.id, parent)))
            .collect::<HashMap<_, _>>();
        for row in &self.rows {
            let mut seen = HashSet::new();
            let mut cursor = Some(row.id);
            while let Some(id) = cursor {
                if !seen.insert(id) {
                    return Err(SemanticError::new(SemanticErrorCode::ParentCycle, None));
                }
                cursor = parents.get(&id).copied();
            }
        }
        let mut edge_ids = HashSet::with_capacity(self.edges.len());
        for edge in &self.edges {
            if !edge_ids.insert(edge.id)
                || edge.source == edge.target
                || !ids.contains(&edge.source)
                || !ids.contains(&edge.target)
            {
                return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasMoveDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCanvasAccessibilityCommand {
    SetMode(AgentCanvasPresentationMode),
    FocusNode(CanvasNodeSemanticId),
    OpenNode(CanvasNodeSemanticId),
    OpenNodeMenu(CanvasNodeSemanticId),
    MoveNode {
        node: CanvasNodeSemanticId,
        direction: CanvasMoveDirection,
        expected_revision: u64,
    },
    RenameNode(CanvasNodeSemanticId),
    ToggleCollapsed {
        node: CanvasNodeSemanticId,
        expected_revision: u64,
    },
    RequestRemove {
        node: CanvasNodeSemanticId,
        expected_revision: u64,
    },
}

pub type AgentCanvasActionRoute = (
    (SemanticNodeId, SemanticAction),
    AgentCanvasAccessibilityCommand,
);

pub fn reconcile_canvas_focus(
    previous: CanvasNodeSemanticId,
    rows: &[CanvasAlternativeRow],
    edges: &[CanvasAlternativeEdge],
) -> Option<CanvasNodeSemanticId> {
    let ids = rows.iter().map(|row| row.id).collect::<HashSet<_>>();
    if ids.contains(&previous) {
        return Some(previous);
    }
    let mut queue = VecDeque::from([previous]);
    let mut visited = HashSet::from([previous]);
    while let Some(current) = queue.pop_front() {
        for edge in edges.iter().filter(|edge| edge.enabled) {
            let next = if edge.source == current {
                Some(edge.target)
            } else if edge.target == current {
                Some(edge.source)
            } else {
                None
            };
            if let Some(next) = next {
                if ids.contains(&next) {
                    return Some(next);
                }
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }
        }
    }
    rows.iter()
        .find(|row| row.parent == Some(previous))
        .or_else(|| rows.iter().min_by_key(|row| row.id))
        .map(|row| row.id)
}

pub const fn canvas_revision_matches(expected: u64, current: u64) -> bool {
    expected == current
}

fn row_semantic_node(id: CanvasNodeSemanticId) -> SemanticNodeId {
    semantic_id(ROW_NODE_BASE + id.get())
}

fn exposed_actions(
    row: &CanvasAlternativeRow,
    compact: bool,
) -> impl Iterator<Item = CanvasNodeAction> + '_ {
    row.actions
        .iter()
        .copied()
        .filter(move |action| !compact || *action == CanvasNodeAction::OpenMenu)
}

fn edge_semantic_node(id: CanvasEdgeSemanticId) -> SemanticNodeId {
    semantic_id(EDGE_NODE_BASE + id.get())
}

fn action_semantic_node(id: CanvasNodeSemanticId, action: CanvasNodeAction) -> SemanticNodeId {
    let action_index = match action {
        CanvasNodeAction::Open => 0,
        CanvasNodeAction::OpenMenu => 1,
        CanvasNodeAction::MoveUp => 2,
        CanvasNodeAction::MoveDown => 3,
        CanvasNodeAction::MoveLeft => 4,
        CanvasNodeAction::MoveRight => 5,
        CanvasNodeAction::Rename => 6,
        CanvasNodeAction::ToggleCollapsed => 7,
        CanvasNodeAction::Remove => 8,
    };
    semantic_id(ACTION_NODE_BASE + id.get() * 16 + action_index)
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
}

fn named_node(id: SemanticNodeId, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(id, role);
    node.name = Some(SemanticText::Message(name));
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, x: f32, y: f32) -> CanvasAlternativeRow {
        CanvasAlternativeRow {
            id: CanvasNodeSemanticId::from_stable_key(key),
            explicit_order: None,
            kind: CanvasAlternativeNodeKind::Agent,
            state: CanvasAlternativeNodeState::Idle,
            title: Some(format!("Synthetic {key}")),
            parent: None,
            x: x.round() as i32,
            y: y.round() as i32,
            width: 640,
            height: 420,
            selected: false,
            collapsed: false,
            actions: CanvasNodeAction::ALL.into_iter().collect(),
        }
    }

    fn snapshot(rows: Vec<CanvasAlternativeRow>) -> AgentCanvasSemanticSnapshot {
        AgentCanvasSemanticSnapshot {
            generation: 1,
            revision: 7,
            workspace_id: 42,
            state: AgentCanvasSurfaceState::Ready,
            mode: AgentCanvasPresentationMode::Graph,
            recording_friendly: false,
            focused: None,
            rows,
            edges: Vec::new(),
        }
    }

    #[test]
    fn graph_and_list_use_one_deterministic_semantic_order() {
        let a = row("a", 800.0, 400.0);
        let b = row("b", 400.0, 100.0);
        let c = row("c", 100.0, 100.0);
        let graph = snapshot(vec![a.clone(), b.clone(), c.clone()]);
        let mut list = graph.clone();
        list.mode = AgentCanvasPresentationMode::ListInspector;
        let graph_ids = graph
            .ordered_rows()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        let list_ids = list
            .ordered_rows()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        assert_eq!(graph_ids, list_ids);
        assert_eq!(graph_ids, vec![c.id, b.id, a.id]);
    }

    #[test]
    fn explicit_workspace_order_precedes_spatial_fallback() {
        let mut later = row("later", 0.0, 0.0);
        later.explicit_order = Some(2);
        let mut first = row("first", 900.0, 900.0);
        first.explicit_order = Some(1);
        let fallback = row("fallback", -100.0, -100.0);
        let surface = snapshot(vec![later.clone(), fallback.clone(), first.clone()]);
        let ids = surface
            .ordered_rows()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![first.id, later.id, fallback.id]);
    }

    #[test]
    fn keyboard_routes_are_revision_bound_and_complete() {
        let node = row("agent", 0.0, 0.0);
        let id = node.id;
        let surface = snapshot(vec![node]);
        let routes = surface.routes().unwrap();
        assert!(routes.iter().any(|(_, command)| {
            *command
                == AgentCanvasAccessibilityCommand::MoveNode {
                    node: id,
                    direction: CanvasMoveDirection::Right,
                    expected_revision: 7,
                }
        }));
        assert!(canvas_revision_matches(7, 7));
        assert!(!canvas_revision_matches(7, 8));
    }

    #[test]
    fn removed_focus_uses_graph_neighbor_then_stable_first_row() {
        let removed = CanvasNodeSemanticId::from_stable_key("removed");
        let neighbor = row("neighbor", 0.0, 0.0);
        let other = row("other", 10.0, 10.0);
        let edge = CanvasAlternativeEdge {
            id: CanvasEdgeSemanticId::from_stable_key("edge"),
            source: removed,
            target: neighbor.id,
            kind: CanvasAlternativeEdgeKind::Dependency,
            enabled: true,
        };
        assert_eq!(
            reconcile_canvas_focus(removed, &[other.clone(), neighbor.clone()], &[edge]),
            Some(neighbor.id)
        );
        assert_eq!(
            reconcile_canvas_focus(removed, std::slice::from_ref(&other), &[]),
            Some(other.id)
        );
    }

    #[test]
    fn semantics_are_bounded_private_and_include_edge_relations() {
        let source = row("source", 0.0, 0.0);
        let target = row("target", 800.0, 0.0);
        let mut surface = snapshot(vec![source.clone(), target.clone()]);
        surface.recording_friendly = true;
        surface.edges.push(CanvasAlternativeEdge {
            id: CanvasEdgeSemanticId::from_stable_key("dependency"),
            source: source.id,
            target: target.id,
            kind: CanvasAlternativeEdgeKind::Dependency,
            enabled: true,
        });
        let parent = semantic_id(99);
        let nodes = surface.try_nodes(parent).unwrap();
        assert!(nodes.iter().all(|node| {
            !matches!(node.name, Some(SemanticText::UserText(_)))
                && !matches!(node.description, Some(SemanticText::UserText(_)))
        }));
        assert!(nodes.iter().any(|node| node.relations.len() == 2));

        let mut oversized = snapshot(
            (0..=MAX_CANVAS_NODES)
                .map(|index| row(&format!("node-{index}"), index as f32, 0.0))
                .collect(),
        );
        assert_eq!(
            oversized.try_nodes(parent).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
        oversized.rows.truncate(MAX_CANVAS_NODES);
        assert!(oversized.try_nodes(parent).is_ok());
    }

    #[test]
    fn invalid_edges_and_parent_cycles_fail_closed() {
        let mut a = row("a", 0.0, 0.0);
        let mut b = row("b", 0.0, 0.0);
        a.parent = Some(b.id);
        b.parent = Some(a.id);
        let surface = snapshot(vec![a, b]);
        assert!(surface.try_nodes(semantic_id(1)).is_err());
    }

    #[test]
    fn scale_reflows_to_list_at_four_hundred_percent() {
        assert_eq!(
            agent_canvas_presentation_mode(200),
            Some(AgentCanvasPresentationMode::Graph)
        );
        assert_eq!(
            agent_canvas_presentation_mode(400),
            Some(AgentCanvasPresentationMode::ListInspector)
        );
        assert_eq!(agent_canvas_presentation_mode(99), None);
        assert_eq!(agent_canvas_presentation_mode(401), None);
    }

    #[test]
    fn states_messages_and_tokens_cover_every_locale_and_theme() {
        for locale in crate::Locale::ALL {
            let localizer = crate::Localizer::try_new(locale.tag()).unwrap();
            for state in AgentCanvasSurfaceState::ALL {
                assert!(!localizer.format_static(state.message()).unwrap().is_empty());
            }
        }
        for theme in crate::ThemeKind::ALL {
            let tokens = crate::DesignTokens::new(theme);
            assert!(tokens.color_bg_canvas().alpha > 0);
            assert!(tokens.color_focus().alpha > 0);
        }
    }
}
