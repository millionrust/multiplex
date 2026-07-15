use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClipboardItem, Context, CursorStyle, Div, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, PathBuilder, Point, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled, Window, canvas as paint_canvas, div,
    point, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::agents::{
    AgentApprovalRequest, AgentEvent, AgentExecutableStatus, AgentRunState, CodexSessionConfig,
    CodexSessionHandle, HeadlessSessionConfig, HeadlessSessionHandle, SchedulableAgent,
    build_context_handoff, build_interactive_launch_spec, build_remote_interactive_arguments,
    create_managed_worktree, detect_agent_executable, managed_worktree_status, provider_descriptor,
    remove_managed_worktree, schedule_dependency_dag, spawn_codex_session, spawn_headless_session,
};
use crate::models::{
    AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, AuthConfig, AuthMode,
    CANVAS_DEFAULT_NODE_HEIGHT, CANVAS_DEFAULT_NODE_WIDTH, CANVAS_MAX_ZOOM, CANVAS_MIN_NODE_HEIGHT,
    CANVAS_MIN_NODE_WIDTH, CANVAS_MIN_ZOOM, CanvasEdgeId, CanvasEdgeKind, CanvasNodeId,
    ConnectRequest, ConnectionKind, HostProfile, LocalShellConfig, SavedAgentDefinition,
    SavedCanvasEdge, SavedCanvasNode, SavedCanvasNodeKind, SavedCanvasState, SavedCanvasViewport,
    SavedWorktreePolicy, WorkspaceLayoutMode, default_persistent_session_name_from_id,
};
use crate::ui::app::{TermiRustApp, WorkspaceViewMode};
use crate::ui::shell::shell_single_quote;
use crate::ui::theme;
use crate::{storage::managed_agent_worktree_dir, ui::util::current_unix_millis};

pub(super) const CANVAS_TOOLBAR_HEIGHT: f32 = 44.0;
pub(super) const CANVAS_NODE_HEADER_HEIGHT: f32 = 34.0;
pub(super) const CANVAS_NODE_GUTTER: f32 = 28.0;
#[cfg(test)]
pub(super) const CANVAS_V1_SUPPORTED_NODE_COUNT: usize = 20;
#[cfg(test)]
pub(super) const CANVAS_V1_SUPPORTED_EDGE_COUNT: usize = 40;
const CANVAS_PLACEMENT_STEP_X: f32 = CANVAS_DEFAULT_NODE_WIDTH + CANVAS_NODE_GUTTER;
const CANVAS_PLACEMENT_STEP_Y: f32 = CANVAS_DEFAULT_NODE_HEIGHT + CANVAS_NODE_GUTTER;
const CANVAS_FIT_PADDING: f32 = 48.0;

#[derive(Clone, Debug)]
pub(super) struct AgentCreationState {
    definition: SavedAgentDefinition,
    executable_status: AgentExecutableStatus,
}

#[derive(Clone, Debug)]
pub(super) struct ContextHandoffReview {
    pub edge_id: CanvasEdgeId,
    pub target: CanvasNodeId,
    pub source_label: String,
    pub redaction_count: usize,
    pub truncated: bool,
}

pub(super) enum StructuredAgentHandle {
    Codex(CodexSessionHandle),
    Headless(HeadlessSessionHandle),
}

impl StructuredAgentHandle {
    fn try_recv(&self) -> Result<AgentEvent, std::sync::mpsc::TryRecvError> {
        match self {
            Self::Codex(handle) => handle.event_rx.try_recv(),
            Self::Headless(handle) => handle.event_rx.try_recv(),
        }
    }

    fn send_prompt(&self, prompt: String) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.send_prompt(prompt),
            Self::Headless(handle) => handle.send_prompt(prompt),
        }
    }

    fn cancel(&self) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.cancel(),
            Self::Headless(handle) => handle.cancel(),
        }
    }

    fn respond_to_approval(&self, request_id: &str, allow: bool) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.respond_to_approval(request_id, allow),
            Self::Headless(_) => anyhow::bail!("This provider did not expose an approval request"),
        }
    }
}

pub(super) struct StructuredAgentRuntime {
    pub handle: StructuredAgentHandle,
    pub state: AgentRunState,
    pub transcript: String,
    pub approval: Option<AgentApprovalRequest>,
    pub diagnostic: Option<String>,
    pub queued_prompt: Option<String>,
}

impl StructuredAgentRuntime {
    const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

    fn new(handle: StructuredAgentHandle) -> Self {
        Self {
            handle,
            state: AgentRunState::Starting,
            transcript: String::new(),
            approval: None,
            diagnostic: None,
            queued_prompt: None,
        }
    }

    fn push_text(&mut self, text: &str) {
        self.transcript.push_str(text);
        if self.transcript.len() > Self::MAX_TRANSCRIPT_BYTES {
            let mut start = self.transcript.len() - Self::MAX_TRANSCRIPT_BYTES;
            while !self.transcript.is_char_boundary(start) {
                start += 1;
            }
            self.transcript.drain(..start);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CanvasPoint {
    pub x: f32,
    pub y: f32,
}

impl CanvasPoint {
    pub(super) fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CanvasRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CanvasRect {
    pub(super) fn contains(self, point: CanvasPoint) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x <= self.x + self.width
            && point.y <= self.y + self.height
    }

    fn intersects_with_gutter(self, other: Self, gutter: f32) -> bool {
        self.x < other.x + other.width + gutter
            && self.x + self.width + gutter > other.x
            && self.y < other.y + other.height + gutter
            && self.y + self.height + gutter > other.y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CanvasTransform {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl Default for CanvasTransform {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl From<SavedCanvasViewport> for CanvasTransform {
    fn from(viewport: SavedCanvasViewport) -> Self {
        Self {
            pan_x: viewport.pan_x,
            pan_y: viewport.pan_y,
            zoom: viewport.zoom,
        }
    }
}

impl From<CanvasTransform> for SavedCanvasViewport {
    fn from(transform: CanvasTransform) -> Self {
        Self {
            pan_x: transform.pan_x,
            pan_y: transform.pan_y,
            zoom: transform.zoom,
        }
    }
}

impl CanvasTransform {
    pub(super) fn world_to_screen(self, point: CanvasPoint) -> CanvasPoint {
        CanvasPoint::new(
            point.x * self.zoom + self.pan_x,
            point.y * self.zoom + self.pan_y,
        )
    }

    pub(super) fn screen_to_world(self, point: CanvasPoint) -> CanvasPoint {
        let zoom = self.zoom.max(f32::EPSILON);
        CanvasPoint::new((point.x - self.pan_x) / zoom, (point.y - self.pan_y) / zoom)
    }

    pub(super) fn screen_rect(self, rect: CanvasRect) -> CanvasRect {
        let origin = self.world_to_screen(CanvasPoint::new(rect.x, rect.y));
        CanvasRect {
            x: origin.x,
            y: origin.y,
            width: rect.width * self.zoom,
            height: rect.height * self.zoom,
        }
    }

    pub(super) fn zoom_around(self, cursor: CanvasPoint, requested_zoom: f32) -> Self {
        let world = self.screen_to_world(cursor);
        let zoom = requested_zoom.clamp(CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM);
        Self {
            pan_x: cursor.x - world.x * zoom,
            pan_y: cursor.y - world.y * zoom,
            zoom,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CanvasNodeKind {
    Terminal {
        pane_id: u64,
    },
    Agent {
        pane_id: Option<u64>,
        definition: SavedAgentDefinition,
    },
}

impl CanvasNodeKind {
    pub(super) fn pane_id(&self) -> Option<u64> {
        match self {
            Self::Terminal { pane_id } => Some(*pane_id),
            Self::Agent { pane_id, .. } => *pane_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CanvasNode {
    pub id: CanvasNodeId,
    pub kind: CanvasNodeKind,
    pub rect: CanvasRect,
    pub z_index: i32,
    pub title: Option<String>,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CanvasEdge {
    pub id: CanvasEdgeId,
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    pub kind: CanvasEdgeKind,
    pub enabled: bool,
    pub context_policy: Option<crate::models::SavedContextPolicy>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CanvasWorkspaceState {
    pub transform: CanvasTransform,
    pub nodes: Vec<CanvasNode>,
    pub edges: Vec<CanvasEdge>,
    pub selected_node_id: Option<CanvasNodeId>,
    next_z_index: i32,
}

impl Default for CanvasWorkspaceState {
    fn default() -> Self {
        Self {
            transform: CanvasTransform::default(),
            nodes: Vec::new(),
            edges: Vec::new(),
            selected_node_id: None,
            next_z_index: 1,
        }
    }
}

impl CanvasWorkspaceState {
    pub(super) fn from_saved(saved: Option<&SavedCanvasState>, pane_ids: &[u64]) -> Self {
        let Some(saved) = saved else {
            let mut state = Self::default();
            state.ensure_terminal_nodes(pane_ids, CanvasPoint::default());
            return state;
        };

        let mut nodes = Vec::with_capacity(saved.nodes.len());
        for node in &saved.nodes {
            let kind = match &node.kind {
                SavedCanvasNodeKind::Terminal { pane_index } => {
                    let Some(pane_id) = pane_ids.get(*pane_index).copied() else {
                        continue;
                    };
                    CanvasNodeKind::Terminal { pane_id }
                }
                SavedCanvasNodeKind::Agent {
                    pane_index,
                    definition,
                } => CanvasNodeKind::Agent {
                    pane_id: pane_index.and_then(|index| pane_ids.get(index).copied()),
                    definition: definition.clone(),
                },
            };
            nodes.push(CanvasNode {
                id: node.id.clone(),
                kind,
                rect: CanvasRect {
                    x: node.x,
                    y: node.y,
                    width: node.width,
                    height: node.height,
                },
                z_index: node.z_index,
                title: node.title.clone(),
                collapsed: node.collapsed,
            });
        }

        let node_ids: HashSet<_> = nodes.iter().map(|node| node.id.clone()).collect();
        let edges = saved
            .edges
            .iter()
            .filter(|edge| node_ids.contains(&edge.source) && node_ids.contains(&edge.target))
            .map(|edge| CanvasEdge {
                id: edge.id.clone(),
                source: edge.source.clone(),
                target: edge.target.clone(),
                kind: edge.kind,
                enabled: edge.enabled,
                context_policy: edge.context_policy.clone(),
            })
            .collect();
        let next_z_index = nodes
            .iter()
            .map(|node| node.z_index)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let mut state = Self {
            transform: saved.viewport.into(),
            nodes,
            edges,
            selected_node_id: None,
            next_z_index,
        };
        state.ensure_terminal_nodes(pane_ids, CanvasPoint::default());
        state
    }

    pub(super) fn to_saved(&self, pane_indices: &HashMap<u64, usize>) -> SavedCanvasState {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        let mut saved_ids = HashSet::new();
        for node in &self.nodes {
            let kind = match &node.kind {
                CanvasNodeKind::Terminal { pane_id } => {
                    let Some(pane_index) = pane_indices.get(pane_id).copied() else {
                        continue;
                    };
                    SavedCanvasNodeKind::Terminal { pane_index }
                }
                CanvasNodeKind::Agent {
                    pane_id,
                    definition,
                } => SavedCanvasNodeKind::Agent {
                    pane_index: pane_id.and_then(|id| pane_indices.get(&id).copied()),
                    definition: definition.clone(),
                },
            };
            saved_ids.insert(node.id.clone());
            nodes.push(SavedCanvasNode {
                id: node.id.clone(),
                kind,
                x: node.rect.x,
                y: node.rect.y,
                width: node.rect.width,
                height: node.rect.height,
                z_index: node.z_index,
                title: node.title.clone(),
                collapsed: node.collapsed,
            });
        }

        let edges = self
            .edges
            .iter()
            .filter(|edge| saved_ids.contains(&edge.source) && saved_ids.contains(&edge.target))
            .map(|edge| SavedCanvasEdge {
                id: edge.id.clone(),
                source: edge.source.clone(),
                target: edge.target.clone(),
                kind: edge.kind,
                enabled: edge.enabled,
                context_policy: edge.context_policy.clone(),
            })
            .collect();

        SavedCanvasState {
            viewport: self.transform.into(),
            nodes,
            edges,
            ..SavedCanvasState::default()
        }
    }

    pub(super) fn ensure_terminal_nodes(&mut self, pane_ids: &[u64], viewport_center: CanvasPoint) {
        let existing: HashSet<u64> = self
            .nodes
            .iter()
            .filter_map(|node| node.kind.pane_id())
            .collect();
        for pane_id in pane_ids.iter().copied() {
            if existing.contains(&pane_id) {
                continue;
            }
            self.add_terminal_node(pane_id, viewport_center);
        }
    }

    pub(super) fn add_terminal_node(
        &mut self,
        pane_id: u64,
        viewport_center: CanvasPoint,
    ) -> CanvasNodeId {
        let id = unique_node_id(&self.nodes, format!("canvas-node-{pane_id}"));
        let position = find_non_overlapping_position(
            &self.nodes,
            CANVAS_DEFAULT_NODE_WIDTH,
            CANVAS_DEFAULT_NODE_HEIGHT,
            viewport_center,
        );
        self.nodes.push(CanvasNode {
            id: id.clone(),
            kind: CanvasNodeKind::Terminal { pane_id },
            rect: CanvasRect {
                x: position.x,
                y: position.y,
                width: CANVAS_DEFAULT_NODE_WIDTH,
                height: CANVAS_DEFAULT_NODE_HEIGHT,
            },
            z_index: self.next_z_index,
            title: None,
            collapsed: false,
        });
        self.next_z_index = self.next_z_index.saturating_add(1);
        id
    }

    pub(super) fn add_agent_node(
        &mut self,
        pane_id: Option<u64>,
        definition: SavedAgentDefinition,
        viewport_center: CanvasPoint,
    ) -> CanvasNodeId {
        let id = unique_node_id(
            &self.nodes,
            pane_id
                .map(|pane_id| format!("agent-node-{pane_id}"))
                .unwrap_or_else(|| format!("agent-node-{}", current_unix_millis())),
        );
        let position = find_non_overlapping_position(
            &self.nodes,
            CANVAS_DEFAULT_NODE_WIDTH,
            CANVAS_DEFAULT_NODE_HEIGHT,
            viewport_center,
        );
        let title = Some(definition.provider.label().to_string());
        self.nodes.push(CanvasNode {
            id: id.clone(),
            kind: CanvasNodeKind::Agent {
                pane_id,
                definition,
            },
            rect: CanvasRect {
                x: position.x,
                y: position.y,
                width: CANVAS_DEFAULT_NODE_WIDTH,
                height: CANVAS_DEFAULT_NODE_HEIGHT,
            },
            z_index: self.next_z_index,
            title,
            collapsed: false,
        });
        self.next_z_index = self.next_z_index.saturating_add(1);
        id
    }

    pub(super) fn remove_node(&mut self, node_id: &CanvasNodeId) {
        self.nodes.retain(|node| &node.id != node_id);
        self.edges
            .retain(|edge| &edge.source != node_id && &edge.target != node_id);
        if self.selected_node_id.as_ref() == Some(node_id) {
            self.selected_node_id = None;
        }
    }

    pub(super) fn add_context_edge(
        &mut self,
        source: CanvasNodeId,
        target: CanvasNodeId,
    ) -> anyhow::Result<CanvasEdgeId> {
        if source == target {
            anyhow::bail!("Choose a different target node");
        }
        if !self.nodes.iter().any(|node| node.id == source)
            || !self.nodes.iter().any(|node| node.id == target)
        {
            anyhow::bail!("Both context-link nodes must exist");
        }
        if self.edges.iter().any(|edge| {
            edge.source == source && edge.target == target && edge.kind == CanvasEdgeKind::Context
        }) {
            anyhow::bail!("That context link already exists");
        }
        let mut ordinal = self.edges.len() + 1;
        let id = loop {
            let candidate = CanvasEdgeId::new(format!("context-edge-{ordinal}"));
            if !self.edges.iter().any(|edge| edge.id == candidate) {
                break candidate;
            }
            ordinal += 1;
        };
        self.edges.push(CanvasEdge {
            id: id.clone(),
            source,
            target,
            kind: CanvasEdgeKind::Context,
            enabled: true,
            context_policy: Some(crate::models::SavedContextPolicy::default()),
        });
        Ok(id)
    }

    pub(super) fn add_dependency_edge(
        &mut self,
        source: CanvasNodeId,
        target: CanvasNodeId,
    ) -> anyhow::Result<CanvasEdgeId> {
        if source == target {
            anyhow::bail!("Choose a different dependency target");
        }
        if !self.nodes.iter().any(|node| node.id == source)
            || !self.nodes.iter().any(|node| node.id == target)
        {
            anyhow::bail!("Both dependency nodes must exist");
        }
        if self.edges.iter().any(|edge| {
            edge.source == source
                && edge.target == target
                && edge.kind == CanvasEdgeKind::Dependency
        }) {
            anyhow::bail!("That dependency already exists");
        }
        let mut adjacency: HashMap<CanvasNodeId, Vec<CanvasNodeId>> = HashMap::new();
        for edge in self
            .edges
            .iter()
            .filter(|edge| edge.enabled && edge.kind == CanvasEdgeKind::Dependency)
        {
            adjacency
                .entry(edge.source.clone())
                .or_default()
                .push(edge.target.clone());
        }
        let mut pending = vec![target.clone()];
        let mut visited = HashSet::new();
        while let Some(node) = pending.pop() {
            if node == source {
                anyhow::bail!("That dependency would create a cycle");
            }
            if visited.insert(node.clone()) {
                pending.extend(adjacency.get(&node).into_iter().flatten().cloned());
            }
        }
        let mut ordinal = self.edges.len() + 1;
        let id = loop {
            let candidate = CanvasEdgeId::new(format!("dependency-edge-{ordinal}"));
            if !self.edges.iter().any(|edge| edge.id == candidate) {
                break candidate;
            }
            ordinal += 1;
        };
        self.edges.push(CanvasEdge {
            id: id.clone(),
            source,
            target,
            kind: CanvasEdgeKind::Dependency,
            enabled: true,
            context_policy: None,
        });
        Ok(id)
    }

    pub(super) fn remove_pane(&mut self, pane_id: u64) {
        let removed_ids: HashSet<_> = self
            .nodes
            .iter()
            .filter(|node| node.kind.pane_id() == Some(pane_id))
            .map(|node| node.id.clone())
            .collect();
        self.nodes
            .retain(|node| node.kind.pane_id() != Some(pane_id));
        self.edges.retain(|edge| {
            !removed_ids.contains(&edge.source) && !removed_ids.contains(&edge.target)
        });
        if self
            .selected_node_id
            .as_ref()
            .is_some_and(|id| removed_ids.contains(id))
        {
            self.selected_node_id = None;
        }
    }

    pub(super) fn node(&self, node_id: &CanvasNodeId) -> Option<&CanvasNode> {
        self.nodes.iter().find(|node| &node.id == node_id)
    }

    pub(super) fn node_mut(&mut self, node_id: &CanvasNodeId) -> Option<&mut CanvasNode> {
        self.nodes.iter_mut().find(|node| &node.id == node_id)
    }

    pub(super) fn select_and_raise(&mut self, node_id: &CanvasNodeId) {
        self.selected_node_id = Some(node_id.clone());
        let next_z = self.next_z_index;
        if let Some(node) = self.node_mut(node_id) {
            node.z_index = next_z;
        }
        self.next_z_index = self.next_z_index.saturating_add(1);
    }

    pub(super) fn node_at_screen(&self, point: CanvasPoint) -> Option<&CanvasNode> {
        self.nodes
            .iter()
            .filter(|node| self.transform.screen_rect(node.rect).contains(point))
            .max_by_key(|node| node.z_index)
    }

    pub(super) fn fit_to_content(&mut self, viewport_width: f32, viewport_height: f32) {
        self.transform = fit_transform(
            &self.nodes,
            viewport_width,
            viewport_height,
            CANVAS_FIT_PADDING,
        );
    }
}

fn unique_node_id(nodes: &[CanvasNode], base: String) -> CanvasNodeId {
    let existing: HashSet<_> = nodes.iter().map(|node| node.id.as_str()).collect();
    if !existing.contains(base.as_str()) {
        return CanvasNodeId::new(base);
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing.contains(candidate.as_str()) {
            return CanvasNodeId::new(candidate);
        }
        suffix += 1;
    }
}

pub(super) fn find_non_overlapping_position(
    nodes: &[CanvasNode],
    width: f32,
    height: f32,
    center: CanvasPoint,
) -> CanvasPoint {
    let origin = CanvasPoint::new(center.x - width / 2.0, center.y - height / 2.0);
    let candidate_rect = |column: i32, row: i32| CanvasRect {
        x: origin.x + column as f32 * CANVAS_PLACEMENT_STEP_X,
        y: origin.y + row as f32 * CANVAS_PLACEMENT_STEP_Y,
        width,
        height,
    };
    let is_free = |candidate: CanvasRect| {
        nodes
            .iter()
            .all(|node| !candidate.intersects_with_gutter(node.rect, CANVAS_NODE_GUTTER))
    };

    if is_free(candidate_rect(0, 0)) {
        return origin;
    }
    for ring in 1..=64 {
        for column in -ring..=ring {
            for row in [-ring, ring] {
                let candidate = candidate_rect(column, row);
                if is_free(candidate) {
                    return CanvasPoint::new(candidate.x, candidate.y);
                }
            }
        }
        for row in (-ring + 1)..=(ring - 1) {
            for column in [-ring, ring] {
                let candidate = candidate_rect(column, row);
                if is_free(candidate) {
                    return CanvasPoint::new(candidate.x, candidate.y);
                }
            }
        }
    }

    CanvasPoint::new(
        origin.x + nodes.len() as f32 * CANVAS_PLACEMENT_STEP_X,
        origin.y,
    )
}

pub(super) fn fit_transform(
    nodes: &[CanvasNode],
    viewport_width: f32,
    viewport_height: f32,
    padding: f32,
) -> CanvasTransform {
    let Some(first) = nodes.first() else {
        return CanvasTransform::default();
    };
    let mut min_x = first.rect.x;
    let mut min_y = first.rect.y;
    let mut max_x = first.rect.x + first.rect.width;
    let mut max_y = first.rect.y + first.rect.height;
    for node in &nodes[1..] {
        min_x = min_x.min(node.rect.x);
        min_y = min_y.min(node.rect.y);
        max_x = max_x.max(node.rect.x + node.rect.width);
        max_y = max_y.max(node.rect.y + node.rect.height);
    }

    let content_width = (max_x - min_x).max(1.0);
    let content_height = (max_y - min_y).max(1.0);
    let available_width = (viewport_width - padding * 2.0).max(1.0);
    let available_height = (viewport_height - padding * 2.0).max(1.0);
    let zoom = (available_width / content_width)
        .min(available_height / content_height)
        .clamp(CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM);
    let rendered_width = content_width * zoom;
    let rendered_height = content_height * zoom;

    CanvasTransform {
        pan_x: (viewport_width - rendered_width) / 2.0 - min_x * zoom,
        pan_y: (viewport_height - rendered_height) / 2.0 - min_y * zoom,
        zoom,
    }
}

pub(super) fn clamp_node_rect(mut rect: CanvasRect) -> CanvasRect {
    if !rect.x.is_finite() {
        rect.x = 0.0;
    }
    if !rect.y.is_finite() {
        rect.y = 0.0;
    }
    rect.width = if rect.width.is_finite() {
        rect.width.max(CANVAS_MIN_NODE_WIDTH)
    } else {
        CANVAS_DEFAULT_NODE_WIDTH
    };
    rect.height = if rect.height.is_finite() {
        rect.height.max(CANVAS_MIN_NODE_HEIGHT)
    } else {
        CANVAS_DEFAULT_NODE_HEIGHT
    };
    rect
}

#[derive(Clone, Debug)]
pub(super) enum CanvasInteraction {
    Pan {
        workspace_id: u64,
        start: CanvasPoint,
        start_pan: CanvasPoint,
    },
    MoveNode {
        workspace_id: u64,
        node_id: CanvasNodeId,
        start: CanvasPoint,
        start_rect: CanvasRect,
    },
    ResizeNode {
        workspace_id: u64,
        node_id: CanvasNodeId,
        start: CanvasPoint,
        start_rect: CanvasRect,
    },
}

impl CanvasInteraction {
    fn workspace_id(&self) -> u64 {
        match self {
            Self::Pan { workspace_id, .. }
            | Self::MoveNode { workspace_id, .. }
            | Self::ResizeNode { workspace_id, .. } => *workspace_id,
        }
    }
}

fn point_from_pixels(position: Point<gpui::Pixels>) -> CanvasPoint {
    CanvasPoint::new(position.x.into(), position.y.into())
}

impl TermiRustApp {
    pub(super) fn set_workspace_layout_mode(
        &mut self,
        mode: WorkspaceLayoutMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let screen_center = CanvasPoint::new(
            viewport_width / 2.0,
            (viewport_height - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT).max(1.0) / 2.0,
        );

        let Some(workspace) = self.workspace_mut(workspace_id) else {
            return;
        };
        if workspace.layout_mode == mode {
            return;
        }
        if mode == WorkspaceLayoutMode::Split && workspace.pane_ids.len() > super::MAX_SPLIT_PANES {
            self.error_message = format!(
                "Split view supports up to {} panes. Close or detach extra canvas nodes first.",
                super::MAX_SPLIT_PANES
            );
            cx.notify();
            return;
        }

        if mode == WorkspaceLayoutMode::Canvas {
            let world_center = workspace.canvas.transform.screen_to_world(screen_center);
            let pane_ids = workspace.pane_ids.clone();
            workspace
                .canvas
                .ensure_terminal_nodes(&pane_ids, world_center);
            workspace.search_visible = false;
        } else {
            let split_ids = workspace
                .layout
                .as_ref()
                .map(|layout| layout.leaf_ids())
                .unwrap_or_default();
            for pane_id in workspace.pane_ids.clone() {
                if split_ids.contains(&pane_id) {
                    continue;
                }
                if let Some(layout) = workspace.layout.as_mut() {
                    *layout = super::SplitNode::Split {
                        axis: crate::models::SplitAxis::Horizontal,
                        ratio: 0.65,
                        a: Box::new(layout.clone()),
                        b: Box::new(super::SplitNode::Leaf(pane_id)),
                    };
                } else {
                    workspace.layout = Some(super::SplitNode::Leaf(pane_id));
                }
            }
        }

        workspace.layout_mode = mode;
        workspace.view_mode = WorkspaceViewMode::Terminal;
        self.canvas_interaction = None;
        self.error_message.clear();
        self.persist_runtime_state();
        self.sync_terminal_layout(window, cx);
        cx.notify();
    }

    fn canvas_local_point(&self, position: Point<gpui::Pixels>) -> CanvasPoint {
        let point = point_from_pixels(position);
        CanvasPoint::new(
            point.x,
            point.y - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT,
        )
    }

    fn start_canvas_pan(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let local = self.canvas_local_point(event.position);
        if workspace.canvas.node_at_screen(local).is_some() {
            return;
        }
        self.canvas_interaction = Some(CanvasInteraction::Pan {
            workspace_id: workspace.id,
            start: point_from_pixels(event.position),
            start_pan: CanvasPoint::new(
                workspace.canvas.transform.pan_x,
                workspace.canvas.transform.pan_y,
            ),
        });
        cx.notify();
    }

    fn start_canvas_node_move(
        &mut self,
        workspace_id: u64,
        node_id: CanvasNodeId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(start_rect) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.canvas.node(&node_id))
            .map(|node| node.rect)
        else {
            return;
        };
        let pane_id = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.canvas.node(&node_id))
            .and_then(|node| node.kind.pane_id());
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.canvas.select_and_raise(&node_id);
            if let Some(pane_id) = pane_id {
                workspace.active_pane_id = pane_id;
            }
        }
        self.canvas_interaction = Some(CanvasInteraction::MoveNode {
            workspace_id,
            node_id,
            start: point_from_pixels(event.position),
            start_rect,
        });
        if let Some(pane_id) = pane_id {
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
        }
        cx.notify();
    }

    fn start_canvas_node_resize(
        &mut self,
        workspace_id: u64,
        node_id: CanvasNodeId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(start_rect) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.canvas.node(&node_id))
            .map(|node| node.rect)
        else {
            return;
        };
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.canvas.select_and_raise(&node_id);
        }
        self.canvas_interaction = Some(CanvasInteraction::ResizeNode {
            workspace_id,
            node_id,
            start: point_from_pixels(event.position),
            start_rect,
        });
        cx.notify();
    }

    pub(super) fn handle_canvas_interaction_move(
        &mut self,
        position: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(interaction) = self.canvas_interaction.clone() else {
            return false;
        };
        let current = point_from_pixels(position);
        let Some(workspace) = self.workspace_mut(interaction.workspace_id()) else {
            self.canvas_interaction = None;
            return false;
        };
        let zoom = workspace.canvas.transform.zoom.max(f32::EPSILON);
        match interaction {
            CanvasInteraction::Pan {
                start, start_pan, ..
            } => {
                workspace.canvas.transform.pan_x = start_pan.x + current.x - start.x;
                workspace.canvas.transform.pan_y = start_pan.y + current.y - start.y;
            }
            CanvasInteraction::MoveNode {
                node_id,
                start,
                start_rect,
                ..
            } => {
                if let Some(node) = workspace.canvas.node_mut(&node_id) {
                    node.rect.x = start_rect.x + (current.x - start.x) / zoom;
                    node.rect.y = start_rect.y + (current.y - start.y) / zoom;
                    node.rect = clamp_node_rect(node.rect);
                }
            }
            CanvasInteraction::ResizeNode {
                node_id,
                start,
                start_rect,
                ..
            } => {
                if let Some(node) = workspace.canvas.node_mut(&node_id) {
                    node.rect.width = start_rect.width + (current.x - start.x) / zoom;
                    node.rect.height = start_rect.height + (current.y - start.y) / zoom;
                    node.rect = clamp_node_rect(node.rect);
                }
            }
        }
        self.sync_terminal_layout(window, cx);
        cx.notify();
        true
    }

    pub(super) fn finish_canvas_interaction(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.canvas_interaction.take().is_none() {
            return false;
        }
        self.persist_runtime_state();
        self.sync_terminal_layout(window, cx);
        cx.notify();
        true
    }

    fn handle_canvas_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.canvas_local_point(event.position);
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        if self
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.canvas.node_at_screen(local).is_some())
        {
            return;
        }
        let delta = event.delta.pixel_delta(px(16.0));
        let dx: f32 = delta.x.into();
        let dy: f32 = delta.y.into();
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            if event.modifiers.secondary() {
                let factor = (-dy * 0.0025).exp();
                workspace.canvas.transform = workspace
                    .canvas
                    .transform
                    .zoom_around(local, workspace.canvas.transform.zoom * factor);
            } else {
                workspace.canvas.transform.pan_x += dx;
                workspace.canvas.transform.pan_y += dy;
            }
        }
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    fn zoom_canvas(&mut self, factor: f32, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let center = CanvasPoint::new(
            f32::from(viewport.width) / 2.0,
            (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT) / 2.0,
        );
        if let Some(workspace) = self.active_workspace_mut() {
            workspace.canvas.transform = workspace
                .canvas
                .transform
                .zoom_around(center, workspace.canvas.transform.zoom * factor);
        }
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    fn reset_canvas_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let center = CanvasPoint::new(
            f32::from(viewport.width) / 2.0,
            (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT) / 2.0,
        );
        if let Some(workspace) = self.active_workspace_mut() {
            workspace.canvas.transform = workspace.canvas.transform.zoom_around(center, 1.0);
        }
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    fn fit_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        if let Some(workspace) = self.active_workspace_mut() {
            workspace.canvas.fit_to_content(
                f32::from(viewport.width),
                (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT)
                    .max(1.0),
            );
        }
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    fn add_request_to_canvas(
        &mut self,
        mut request: ConnectRequest,
        agent_definition: Option<SavedAgentDefinition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let Some(workspace_id) = self.active_workspace_id else {
            return None;
        };
        let is_canvas = self
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.layout_mode == WorkspaceLayoutMode::Canvas);
        if !is_canvas {
            return None;
        }

        request.session_id = self.next_session_id();
        let pane_id = self.spawn_pane(request, window, cx);
        let viewport = window.viewport_size();
        let screen_center = CanvasPoint::new(
            f32::from(viewport.width) / 2.0,
            (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT) / 2.0,
        );
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            let world_center = workspace.canvas.transform.screen_to_world(screen_center);
            workspace.pane_ids.push(pane_id);
            workspace.active_pane_id = pane_id;
            let node_id = if let Some(definition) = agent_definition {
                workspace
                    .canvas
                    .add_agent_node(Some(pane_id), definition, world_center)
            } else {
                workspace.canvas.add_terminal_node(pane_id, world_center)
            };
            workspace.canvas.select_and_raise(&node_id);
        }
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
        Some(pane_id)
    }

    fn add_local_terminal_to_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = ConnectRequest::local_shell_with_config(
            0,
            self.saved.settings.default_local_shell.clone(),
        );
        if self
            .add_request_to_canvas(request, None, window, cx)
            .is_none()
        {
            self.open_local_terminal(window, cx);
            return;
        }
        self.canvas_add_menu_open = false;
        self.status_message = "Opened a local terminal on the canvas.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn connect_request_for_saved_canvas_host(
        &self,
        profile: &HostProfile,
    ) -> anyhow::Result<ConnectRequest> {
        let auth = match profile.auth_mode {
            AuthMode::Password => {
                let Some(credential_id) = profile.password_credential_id.clone() else {
                    anyhow::bail!(
                        "{} needs a saved password. Open the host, enter its password, and save it first.",
                        profile.display_name()
                    );
                };
                AuthConfig::PasswordRef { credential_id }
            }
            AuthMode::PrivateKey => {
                if profile.key_path.trim().is_empty() {
                    anyhow::bail!(
                        "{} needs a private key file before it can be added.",
                        profile.display_name()
                    );
                }
                AuthConfig::PrivateKey {
                    key_path: profile.key_path.clone(),
                    passphrase: None,
                }
            }
        };
        let jump_host = profile
            .jump_host_id
            .as_deref()
            .map(|jump_host_id| {
                let mut visited = HashSet::from([profile.id.clone()]);
                self.resolve_jump_host_connection_recursive(jump_host_id, &mut visited)
            })
            .transpose()?;

        Ok(ConnectRequest {
            session_id: 0,
            title: profile.display_name(),
            kind: ConnectionKind::Ssh,
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth: Some(auth),
            jump_host,
            startup_directory: profile.startup_directory.clone(),
            startup_command: profile.startup_command.clone(),
            start_in_files: false,
            persistent_session: profile.persistent_session,
            persistent_session_name: profile.persistent_session_name.clone().or_else(|| {
                profile
                    .persistent_session
                    .then(|| default_persistent_session_name_from_id(&profile.id))
            }),
            persistent_session_detach_others: profile.persistent_session_detach_others,
            terminal_scrollback_rows: profile.terminal_scrollback_rows.unwrap_or(10_000) as usize,
            port_forward_rules: profile.effective_port_forward_rules(),
            local_shell: None,
            environment: profile.environment.clone(),
        })
    }

    pub(super) fn add_saved_host_to_canvas(
        &mut self,
        profile_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self
            .saved
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
        else {
            self.error_message = "That saved host no longer exists.".to_string();
            cx.notify();
            return;
        };
        let request = match self.connect_request_for_saved_canvas_host(&profile) {
            Ok(request) => request,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        if self
            .add_request_to_canvas(request, None, window, cx)
            .is_none()
        {
            self.error_message = "Open a Canvas workspace before adding a saved host.".to_string();
            cx.notify();
            return;
        }
        self.canvas_add_menu_open = false;
        self.status_message = format!("Connecting to {} on the canvas...", profile.display_name());
        self.error_message.clear();
        cx.notify();
    }

    fn default_agent_working_directory(&self) -> String {
        self.active_workspace()
            .and_then(|workspace| self.pane(workspace.active_pane_id))
            .and_then(|pane| {
                pane.request
                    .local_shell
                    .as_ref()
                    .and_then(|shell| shell.cwd.clone())
                    .or_else(|| pane.request.startup_directory.clone())
            })
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_default()
    }

    pub(super) fn open_agent_creation(
        &mut self,
        provider: AgentProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let working_directory = self.default_agent_working_directory();
        let definition = SavedAgentDefinition {
            provider,
            backend: AgentBackendKind::InteractivePty,
            location: AgentLocation::Local,
            working_directory: (!working_directory.is_empty()).then_some(working_directory.clone()),
            executable_override: None,
            arguments: Vec::new(),
            permission_policy: AgentPermissionPolicy::ProviderDefault,
            worktree: SavedWorktreePolicy::Isolated,
            managed_worktree: None,
        };
        Self::set_input_value(
            &self.shell_inputs.agent_working_directory,
            working_directory,
            window,
            cx,
        );
        Self::set_input_value(&self.shell_inputs.agent_executable, "", window, cx);
        Self::set_input_value(&self.shell_inputs.agent_arguments, "", window, cx);
        Self::set_input_value(&self.shell_inputs.agent_initial_prompt, "", window, cx);
        let executable_status = detect_agent_executable(&definition);
        self.agent_creation = Some(AgentCreationState {
            definition,
            executable_status,
        });
        self.canvas_add_menu_open = false;
        cx.notify();
    }

    fn set_agent_creation_provider(
        &mut self,
        provider: AgentProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut state) = self.agent_creation.take() else {
            return;
        };
        state.definition.provider = provider;
        state.definition.executable_override = None;
        Self::set_input_value(&self.shell_inputs.agent_executable, "", window, cx);
        if matches!(provider, AgentProvider::CustomCli | AgentProvider::GroqApi)
            && state.definition.backend == AgentBackendKind::Structured
        {
            state.definition.backend = AgentBackendKind::InteractivePty;
        }
        state.executable_status = detect_agent_executable(&state.definition);
        self.agent_creation = Some(state);
        cx.notify();
    }

    fn set_agent_creation_location(&mut self, location: AgentLocation, cx: &mut Context<Self>) {
        if let Some(state) = self.agent_creation.as_mut() {
            state.definition.location = location;
        }
        cx.notify();
    }

    pub(super) fn set_agent_backend(&mut self, backend: AgentBackendKind, cx: &mut Context<Self>) {
        if let Some(state) = self.agent_creation.as_mut() {
            if backend == AgentBackendKind::Structured
                && matches!(
                    state.definition.provider,
                    AgentProvider::CustomCli | AgentProvider::GroqApi
                )
            {
                self.error_message =
                    "Structured mode is available for Codex, Claude Code, and Gemini CLI."
                        .to_string();
                cx.notify();
                return;
            }
            if backend == AgentBackendKind::Structured
                && !matches!(state.definition.location, AgentLocation::Local)
            {
                self.error_message =
                    "Structured Codex currently runs locally. Use Interactive terminal for SSH hosts."
                        .to_string();
                cx.notify();
                return;
            }
            state.definition.backend = backend;
            self.error_message.clear();
        }
        cx.notify();
    }

    fn set_agent_permission_policy(
        &mut self,
        policy: AgentPermissionPolicy,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.agent_creation.as_mut() {
            state.definition.permission_policy = policy;
        }
        cx.notify();
    }

    pub(super) fn set_agent_worktree_policy(
        &mut self,
        policy: SavedWorktreePolicy,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.agent_creation.as_mut() {
            state.definition.worktree = policy;
        }
        cx.notify();
    }

    fn sync_agent_definition_from_inputs(&self, definition: &mut SavedAgentDefinition, cx: &App) {
        let working_directory = self
            .shell_inputs
            .agent_working_directory
            .read(cx)
            .value()
            .trim()
            .to_string();
        definition.working_directory = (!working_directory.is_empty()).then_some(working_directory);
        let executable = self
            .shell_inputs
            .agent_executable
            .read(cx)
            .value()
            .trim()
            .to_string();
        definition.executable_override = (!executable.is_empty()).then_some(executable);
        definition.arguments = self
            .shell_inputs
            .agent_arguments
            .read(cx)
            .value()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToString::to_string)
            .collect();
    }

    fn check_agent_executable(&mut self, cx: &mut Context<Self>) {
        let Some(mut state) = self.agent_creation.take() else {
            return;
        };
        self.sync_agent_definition_from_inputs(&mut state.definition, cx);
        state.executable_status = detect_agent_executable(&state.definition);
        self.agent_creation = Some(state);
        cx.notify();
    }

    fn append_initial_prompt(
        provider: AgentProvider,
        arguments: &mut Vec<String>,
        prompt: &str,
    ) -> anyhow::Result<()> {
        if prompt.trim().is_empty() {
            return Ok(());
        }
        match provider {
            AgentProvider::Codex | AgentProvider::ClaudeCode => {
                arguments.push(prompt.to_string());
            }
            AgentProvider::Gemini => {
                arguments.push("--prompt-interactive".to_string());
                arguments.push(prompt.to_string());
            }
            AgentProvider::CustomCli => anyhow::bail!(
                "Launch the Custom CLI first, then send its initial prompt in the terminal. TermiRust does not guess a custom prompt flag."
            ),
            AgentProvider::GroqApi => anyhow::bail!("Groq API agents are not available yet"),
        }
        Ok(())
    }

    fn remote_agent_startup_script(
        definition: &SavedAgentDefinition,
        initial_prompt: &str,
    ) -> anyhow::Result<String> {
        let descriptor = provider_descriptor(definition.provider);
        let executable = definition
            .executable_override
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(descriptor.executable)
            .ok_or_else(|| anyhow::anyhow!("Choose an executable for this agent"))?;
        let mut arguments = build_remote_interactive_arguments(definition)?;
        Self::append_initial_prompt(definition.provider, &mut arguments, initial_prompt)?;
        let mut command = shell_single_quote(executable);
        for argument in &arguments {
            command.push(' ');
            command.push_str(&shell_single_quote(argument));
        }
        Ok(format!(
            "if command -v {executable} >/dev/null 2>&1; then exec {command}; else printf '%s\\n' {guidance} >&2; exec \"${{SHELL:-/bin/sh}}\"; fi",
            executable = shell_single_quote(executable),
            guidance = shell_single_quote(descriptor.install_guidance),
        ))
    }

    pub(super) fn launch_agent_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut state) = self.agent_creation.take() else {
            return;
        };
        self.sync_agent_definition_from_inputs(&mut state.definition, cx);
        let initial_prompt = self
            .shell_inputs
            .agent_initial_prompt
            .read(cx)
            .value()
            .to_string();
        let mut definition = state.definition.clone();
        if definition.worktree == SavedWorktreePolicy::ReadOnly {
            definition.permission_policy = AgentPermissionPolicy::ReadOnly;
        }
        if definition.backend == AgentBackendKind::Structured {
            self.launch_structured_agent_creation(state, definition, initial_prompt, window, cx);
            return;
        }

        let request = match &definition.location {
            AgentLocation::Local => {
                let mut launch = match build_interactive_launch_spec(&definition) {
                    Ok(launch) => launch,
                    Err(error) => {
                        state.executable_status = detect_agent_executable(&definition);
                        self.agent_creation = Some(state);
                        self.error_message = error.to_string();
                        cx.notify();
                        return;
                    }
                };
                if definition.worktree == SavedWorktreePolicy::Isolated {
                    let Some(source_directory) = launch.working_directory.as_deref() else {
                        self.agent_creation = Some(state);
                        self.error_message =
                            "Choose a Git repository before creating an isolated worktree."
                                .to_string();
                        cx.notify();
                        return;
                    };
                    let managed_root = match managed_agent_worktree_dir() {
                        Ok(path) => path,
                        Err(error) => {
                            self.agent_creation = Some(state);
                            self.error_message = error.to_string();
                            cx.notify();
                            return;
                        }
                    };
                    let managed = match create_managed_worktree(
                        source_directory,
                        &managed_root,
                        &format!("{}", current_unix_millis()),
                        definition.provider.label(),
                    ) {
                        Ok(worktree) => worktree,
                        Err(error) => {
                            self.agent_creation = Some(state);
                            self.error_message = error.to_string();
                            cx.notify();
                            return;
                        }
                    };
                    definition.working_directory = Some(managed.path.clone());
                    self.saved.register_managed_agent_worktree(managed.clone());
                    self.persist_runtime_state();
                    definition.managed_worktree = Some(managed);
                    launch = match build_interactive_launch_spec(&definition) {
                        Ok(launch) => launch,
                        Err(error) => {
                            self.agent_creation = Some(state);
                            self.error_message = format!(
                                "The isolated worktree was kept, but the agent could not launch: {error}"
                            );
                            cx.notify();
                            return;
                        }
                    };
                }
                let arguments_result: anyhow::Result<Vec<String>> = launch
                    .arguments
                    .drain(..)
                    .map(|argument| {
                        argument.into_string().map_err(|_| {
                            anyhow::anyhow!("Agent argument contains unsupported non-UTF-8 data")
                        })
                    })
                    .collect();
                let mut arguments = match arguments_result {
                    Ok(arguments) => arguments,
                    Err(error) => {
                        self.agent_creation = Some(state);
                        self.error_message = error.to_string();
                        cx.notify();
                        return;
                    }
                };
                if let Err(error) = Self::append_initial_prompt(
                    definition.provider,
                    &mut arguments,
                    &initial_prompt,
                ) {
                    self.agent_creation = Some(state);
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
                let Some(program) = launch.executable.to_str().map(ToString::to_string) else {
                    self.agent_creation = Some(state);
                    self.error_message = "Agent executable path is not valid UTF-8.".to_string();
                    cx.notify();
                    return;
                };
                ConnectRequest::local_shell_with_config(
                    0,
                    LocalShellConfig {
                        program,
                        args: arguments,
                        cwd: launch
                            .working_directory
                            .map(|path| path.display().to_string()),
                    },
                )
            }
            AgentLocation::SavedHost { profile_id } => {
                if definition.worktree == SavedWorktreePolicy::Isolated {
                    self.agent_creation = Some(state);
                    self.error_message = "Remote worktree creation is not automatic. Choose Shared directory or Read only for this host.".to_string();
                    cx.notify();
                    return;
                }
                let Some(profile) = self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .cloned()
                else {
                    self.agent_creation = Some(state);
                    self.error_message = "The selected remote host no longer exists.".to_string();
                    cx.notify();
                    return;
                };
                let mut request = match self.connect_request_for_saved_canvas_host(&profile) {
                    Ok(request) => request,
                    Err(error) => {
                        self.agent_creation = Some(state);
                        self.error_message = error.to_string();
                        cx.notify();
                        return;
                    }
                };
                request.title = format!(
                    "{} on {}",
                    definition.provider.label(),
                    profile.display_name()
                );
                request.startup_directory = definition.working_directory.clone();
                request.startup_command =
                    match Self::remote_agent_startup_script(&definition, &initial_prompt) {
                        Ok(command) => Some(command),
                        Err(error) => {
                            self.agent_creation = Some(state);
                            self.error_message = error.to_string();
                            cx.notify();
                            return;
                        }
                    };
                if request.persistent_session {
                    request.persistent_session_name = Some(format!(
                        "tr-agent-{}-{}",
                        definition
                            .provider
                            .label()
                            .to_ascii_lowercase()
                            .replace(' ', "-"),
                        current_unix_millis()
                    ));
                    request.persistent_session_detach_others = false;
                }
                request
            }
        };

        let mut request = request;
        request.title = definition.provider.label().to_string();
        if self
            .add_request_to_canvas(request, Some(definition.clone()), window, cx)
            .is_none()
        {
            self.agent_creation = Some(state);
            self.error_message = "Open a Canvas workspace before launching an agent.".to_string();
            cx.notify();
            return;
        }
        self.canvas_add_menu_open = false;
        self.status_message = format!("Launching {}...", definition.provider.label());
        self.error_message.clear();
        cx.notify();
    }

    fn launch_structured_agent_creation(
        &mut self,
        mut creation: AgentCreationState,
        mut definition: SavedAgentDefinition,
        initial_prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(definition.location, AgentLocation::Local) {
            self.agent_creation = Some(creation);
            self.error_message = "Structured agents currently run locally.".to_string();
            cx.notify();
            return;
        }
        let executable = match detect_agent_executable(&definition) {
            AgentExecutableStatus::Available { path, .. } => path,
            missing @ AgentExecutableStatus::Missing { .. } => {
                creation.executable_status = missing;
                self.agent_creation = Some(creation);
                self.error_message =
                    "Codex CLI is not installed or could not be resolved.".to_string();
                cx.notify();
                return;
            }
        };
        let Some(mut working_directory) = definition
            .working_directory
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from)
        else {
            self.agent_creation = Some(creation);
            self.error_message = "Choose a Git repository or working directory.".to_string();
            cx.notify();
            return;
        };
        if !working_directory.is_dir() {
            self.agent_creation = Some(creation);
            self.error_message = format!(
                "Working directory does not exist: {}",
                working_directory.display()
            );
            cx.notify();
            return;
        }
        if definition.worktree == SavedWorktreePolicy::Isolated {
            let managed_root = match managed_agent_worktree_dir() {
                Ok(path) => path,
                Err(error) => {
                    self.agent_creation = Some(creation);
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
            };
            let managed = match create_managed_worktree(
                &working_directory,
                &managed_root,
                &format!("{}", current_unix_millis()),
                definition.provider.label(),
            ) {
                Ok(worktree) => worktree,
                Err(error) => {
                    self.agent_creation = Some(creation);
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
            };
            working_directory = std::path::PathBuf::from(&managed.path);
            definition.working_directory = Some(managed.path.clone());
            self.saved.register_managed_agent_worktree(managed.clone());
            self.persist_runtime_state();
            definition.managed_worktree = Some(managed);
        }
        let initial_prompt = (!initial_prompt.trim().is_empty()).then_some(initial_prompt);
        let handle_result = match definition.provider {
            AgentProvider::Codex => spawn_codex_session(CodexSessionConfig {
                executable,
                working_directory,
                permission_policy: definition.permission_policy,
                initial_prompt,
            })
            .map(StructuredAgentHandle::Codex),
            AgentProvider::ClaudeCode | AgentProvider::Gemini => {
                spawn_headless_session(HeadlessSessionConfig {
                    provider: definition.provider,
                    executable,
                    working_directory,
                    permission_policy: definition.permission_policy,
                    arguments: definition.arguments.clone(),
                    initial_prompt,
                })
                .map(StructuredAgentHandle::Headless)
            }
            AgentProvider::CustomCli | AgentProvider::GroqApi => {
                Err(anyhow::anyhow!("This provider has no structured adapter"))
            }
        };
        let handle = match handle_result {
            Ok(handle) => handle,
            Err(error) => {
                self.agent_creation = Some(creation);
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        let Some(workspace_id) = self.active_workspace_id else {
            self.agent_creation = Some(creation);
            self.error_message = "Open a Canvas workspace before launching an agent.".to_string();
            cx.notify();
            return;
        };
        let viewport = window.viewport_size();
        let screen_center = CanvasPoint::new(
            f32::from(viewport.width) / 2.0,
            (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT) / 2.0,
        );
        let Some(workspace) = self.workspace_mut(workspace_id) else {
            self.agent_creation = Some(creation);
            return;
        };
        let world_center = workspace.canvas.transform.screen_to_world(screen_center);
        let node_id = workspace
            .canvas
            .add_agent_node(None, definition, world_center);
        workspace.canvas.select_and_raise(&node_id);
        self.structured_agents
            .insert(node_id, StructuredAgentRuntime::new(handle));
        self.canvas_add_menu_open = false;
        self.status_message = "Starting structured Codex session...".to_string();
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    pub(super) fn process_structured_agent_events(&mut self) -> bool {
        let mut queued = Vec::new();
        for (node_id, runtime) in &self.structured_agents {
            while let Ok(event) = runtime.handle.try_recv() {
                queued.push((node_id.clone(), event));
            }
        }
        let changed = !queued.is_empty();
        for (node_id, event) in queued {
            let Some(runtime) = self.structured_agents.get_mut(&node_id) else {
                continue;
            };
            match event {
                AgentEvent::StateChanged(state) => {
                    runtime.state = state;
                    if state != AgentRunState::WaitingForApproval {
                        runtime.approval = None;
                    }
                }
                AgentEvent::MessageDelta { text, .. } => runtime.push_text(&text),
                AgentEvent::ApprovalRequested(approval) => {
                    runtime.state = AgentRunState::WaitingForApproval;
                    runtime.approval = Some(approval);
                }
                AgentEvent::Failed { error } => {
                    runtime.state = AgentRunState::Failed;
                    runtime.diagnostic = Some(error);
                }
                AgentEvent::Diagnostic { message } => runtime.diagnostic = Some(message),
                AgentEvent::ToolStarted(call) => runtime.push_text(&format!(
                    "\n[{}] {}\n",
                    call.name,
                    call.summary.unwrap_or_default()
                )),
                AgentEvent::ToolFinished { .. }
                | AgentEvent::SessionReady { .. }
                | AgentEvent::Completed { .. } => {}
            }
        }
        if changed && self.orchestration_active {
            self.dispatch_ready_agent_tasks();
        }
        changed
    }

    fn send_structured_agent_prompt(
        &mut self,
        node_id: CanvasNodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .shell_inputs
            .structured_agent_prompt
            .read(cx)
            .value()
            .to_string();
        let result = self
            .structured_agents
            .get(&node_id)
            .ok_or_else(|| anyhow::anyhow!("Structured agent is not running"))
            .and_then(|runtime| runtime.handle.send_prompt(prompt.clone()));
        match result {
            Ok(()) => {
                if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
                    runtime.push_text(&format!("\nYou: {}\n", prompt.trim()));
                    runtime.state = AgentRunState::Running;
                }
                Self::set_input_value(&self.shell_inputs.structured_agent_prompt, "", window, cx);
                self.error_message.clear();
            }
            Err(error) => self.error_message = error.to_string(),
        }
        cx.notify();
    }

    fn cancel_structured_agent(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        if let Some(runtime) = self.structured_agents.get(&node_id) {
            if let Err(error) = runtime.handle.cancel() {
                self.error_message = error.to_string();
            }
        }
        cx.notify();
    }

    fn restart_structured_agent(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        if self.structured_agents.contains_key(&node_id) {
            return;
        }
        let definition = self.workspaces.iter().find_map(|workspace| {
            workspace.canvas.nodes.iter().find_map(|node| {
                if node.id != node_id {
                    return None;
                }
                match &node.kind {
                    CanvasNodeKind::Agent {
                        pane_id: None,
                        definition,
                    } => Some(definition.clone()),
                    _ => None,
                }
            })
        });
        let Some(definition) = definition else {
            self.error_message = "The structured agent definition is unavailable.".to_string();
            cx.notify();
            return;
        };
        if !matches!(definition.location, AgentLocation::Local) {
            self.error_message = "Structured agents currently run locally.".to_string();
            cx.notify();
            return;
        }
        let executable = match detect_agent_executable(&definition) {
            AgentExecutableStatus::Available { path, .. } => path,
            AgentExecutableStatus::Missing {
                requested,
                guidance,
            } => {
                self.error_message = format!(
                    "Agent executable '{}' is unavailable. {guidance}",
                    requested.to_string_lossy()
                );
                cx.notify();
                return;
            }
        };
        let Some(working_directory) = definition
            .working_directory
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from)
        else {
            self.error_message = "The agent has no working directory.".to_string();
            cx.notify();
            return;
        };
        if !working_directory.is_dir() {
            self.error_message = format!(
                "The agent working directory no longer exists: {}",
                working_directory.display()
            );
            cx.notify();
            return;
        }
        let result = match definition.provider {
            AgentProvider::Codex => spawn_codex_session(CodexSessionConfig {
                executable,
                working_directory,
                permission_policy: definition.permission_policy,
                initial_prompt: None,
            })
            .map(StructuredAgentHandle::Codex),
            AgentProvider::ClaudeCode | AgentProvider::Gemini => {
                spawn_headless_session(HeadlessSessionConfig {
                    provider: definition.provider,
                    executable,
                    working_directory,
                    permission_policy: definition.permission_policy,
                    arguments: definition.arguments,
                    initial_prompt: None,
                })
                .map(StructuredAgentHandle::Headless)
            }
            AgentProvider::CustomCli | AgentProvider::GroqApi => {
                Err(anyhow::anyhow!("This provider has no structured adapter"))
            }
        };
        match result {
            Ok(handle) => {
                self.structured_agents
                    .insert(node_id, StructuredAgentRuntime::new(handle));
                self.status_message = "Structured agent restarted.".to_string();
                self.error_message.clear();
            }
            Err(error) => self.error_message = format!("Unable to restart agent: {error:#}"),
        }
        cx.notify();
    }

    fn respond_structured_agent_approval(
        &mut self,
        node_id: CanvasNodeId,
        allow: bool,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .structured_agents
            .get(&node_id)
            .and_then(|runtime| {
                runtime
                    .approval
                    .as_ref()
                    .map(|approval| (runtime, approval))
            })
            .ok_or_else(|| anyhow::anyhow!("Approval request is no longer active"))
            .and_then(|(runtime, approval)| {
                runtime
                    .handle
                    .respond_to_approval(&approval.request_id, allow)
            });
        match result {
            Ok(()) => {
                if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
                    runtime.approval = None;
                    runtime.state = AgentRunState::Running;
                }
                self.error_message.clear();
            }
            Err(error) => self.error_message = error.to_string(),
        }
        cx.notify();
    }

    fn close_structured_agent(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        self.structured_agents.remove(&node_id);
        for workspace in &mut self.workspaces {
            workspace.canvas.remove_node(&node_id);
        }
        self.persist_runtime_state();
        self.status_message = "Structured agent closed. Its worktree was kept.".to_string();
        cx.notify();
    }

    fn toggle_canvas_node_collapsed(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        for workspace in &mut self.workspaces {
            if let Some(node) = workspace
                .canvas
                .nodes
                .iter_mut()
                .find(|node| node.id == node_id)
            {
                node.collapsed = !node.collapsed;
                break;
            }
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn canvas_node_execution_host(&self, node_id: &CanvasNodeId) -> Option<String> {
        let node = self
            .active_workspace()?
            .canvas
            .nodes
            .iter()
            .find(|node| &node.id == node_id)?;
        match &node.kind {
            CanvasNodeKind::Agent { definition, .. } => match &definition.location {
                AgentLocation::Local => Some("local".to_string()),
                AgentLocation::SavedHost { profile_id } => self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .map(|profile| {
                        format!("ssh:{}@{}:{}", profile.username, profile.host, profile.port)
                    }),
            },
            CanvasNodeKind::Terminal { pane_id } => self.pane(*pane_id).map(|pane| {
                if pane.request.is_local_shell() {
                    "local".to_string()
                } else {
                    format!(
                        "ssh:{}@{}:{}",
                        pane.request.username, pane.request.host, pane.request.port
                    )
                }
            }),
        }
    }

    fn ensure_same_execution_host(
        &self,
        source: &CanvasNodeId,
        target: &CanvasNodeId,
    ) -> anyhow::Result<()> {
        let source_host = self
            .canvas_node_execution_host(source)
            .ok_or_else(|| anyhow::anyhow!("The source node execution host is unavailable"))?;
        let target_host = self
            .canvas_node_execution_host(target)
            .ok_or_else(|| anyhow::anyhow!("The target node execution host is unavailable"))?;
        if source_host != target_host {
            anyhow::bail!(
                "Cross-host links are not executable in v1. Choose two nodes on the same local or SSH host."
            );
        }
        Ok(())
    }

    fn link_canvas_node(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        let Some(source) = self.pending_context_source.take() else {
            self.pending_context_source = Some(node_id);
            self.status_message =
                "Context source selected. Use the link action on a target node.".to_string();
            self.error_message.clear();
            cx.notify();
            return;
        };
        let result = self
            .ensure_same_execution_host(&source, &node_id)
            .and_then(|()| {
                self.active_workspace_mut()
                    .ok_or_else(|| anyhow::anyhow!("No active canvas workspace"))
                    .and_then(|workspace| workspace.canvas.add_context_edge(source, node_id))
            });
        match result {
            Ok(_) => {
                self.status_message =
                    "Context link created. Select the target and choose Review context."
                        .to_string();
                self.error_message.clear();
                self.persist_runtime_state();
            }
            Err(error) => self.error_message = error.to_string(),
        }
        cx.notify();
    }

    fn link_canvas_dependency(&mut self, node_id: CanvasNodeId, cx: &mut Context<Self>) {
        let Some(source) = self.pending_dependency_source.take() else {
            self.pending_dependency_source = Some(node_id);
            self.status_message =
                "Dependency source selected. Choose the node that must run after it.".to_string();
            self.error_message.clear();
            cx.notify();
            return;
        };
        let result = self
            .ensure_same_execution_host(&source, &node_id)
            .and_then(|()| {
                self.active_workspace_mut()
                    .ok_or_else(|| anyhow::anyhow!("No active canvas workspace"))
                    .and_then(|workspace| workspace.canvas.add_dependency_edge(source, node_id))
            });
        match result {
            Ok(_) => {
                self.status_message = "Dependency created.".to_string();
                self.error_message.clear();
                self.persist_runtime_state();
            }
            Err(error) => self.error_message = error.to_string(),
        }
        cx.notify();
    }

    fn queue_structured_agent_task(
        &mut self,
        node_id: CanvasNodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .shell_inputs
            .structured_agent_prompt
            .read(cx)
            .value()
            .trim()
            .to_string();
        if prompt.is_empty() {
            self.error_message = "Enter a task before queuing it.".to_string();
            cx.notify();
            return;
        }
        if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
            runtime.queued_prompt = Some(prompt);
            if runtime.state == AgentRunState::Blocked {
                runtime.state = AgentRunState::Idle;
            }
            Self::set_input_value(&self.shell_inputs.structured_agent_prompt, "", window, cx);
            self.status_message = "Task queued for dependency orchestration.".to_string();
            self.error_message.clear();
        }
        cx.notify();
    }

    fn start_dependency_orchestration(&mut self, cx: &mut Context<Self>) {
        let dependency_endpoints: Vec<_> = self
            .active_workspace()
            .map(|workspace| {
                workspace
                    .canvas
                    .edges
                    .iter()
                    .filter(|edge| edge.enabled && edge.kind == CanvasEdgeKind::Dependency)
                    .map(|edge| (edge.source.clone(), edge.target.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(error) = dependency_endpoints
            .iter()
            .find_map(|(source, target)| self.ensure_same_execution_host(source, target).err())
        {
            self.error_message = error.to_string();
            self.orchestration_active = false;
            cx.notify();
            return;
        }
        self.orchestration_active = true;
        if !self.dispatch_ready_agent_tasks() {
            self.status_message =
                "No queued task is ready. Check dependency states and queued prompts.".to_string();
        }
        cx.notify();
    }

    fn dispatch_ready_agent_tasks(&mut self) -> bool {
        let agents: Vec<_> = self
            .structured_agents
            .iter()
            .map(|(node_id, runtime)| SchedulableAgent {
                node_id: node_id.clone(),
                state: runtime.state,
                has_queued_task: runtime.queued_prompt.is_some(),
            })
            .collect();
        let edges: Vec<_> = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.canvas.edges.iter())
            .filter(|edge| edge.kind == CanvasEdgeKind::Dependency)
            .map(|edge| SavedCanvasEdge {
                id: edge.id.clone(),
                source: edge.source.clone(),
                target: edge.target.clone(),
                kind: edge.kind,
                enabled: edge.enabled,
                context_policy: None,
            })
            .collect();
        let schedule = schedule_dependency_dag(&agents, &edges, 2);
        if schedule.cycle_detected {
            self.error_message = "Dependency graph contains a cycle and cannot run.".to_string();
            self.orchestration_active = false;
            return false;
        }
        for node_id in schedule.blocked {
            if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
                runtime.state = AgentRunState::Blocked;
                runtime.diagnostic = Some("A prerequisite failed or was cancelled.".to_string());
            }
        }
        let mut dispatched = false;
        for node_id in schedule.ready {
            let prompt = self
                .structured_agents
                .get_mut(&node_id)
                .and_then(|runtime| runtime.queued_prompt.take());
            let Some(prompt) = prompt else {
                continue;
            };
            let result = self
                .structured_agents
                .get(&node_id)
                .expect("scheduled agent should exist")
                .handle
                .send_prompt(prompt.clone());
            if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
                match result {
                    Ok(()) => {
                        runtime.state = AgentRunState::Starting;
                        runtime.push_text(&format!("\nQueued task: {}\n", prompt.trim()));
                        dispatched = true;
                    }
                    Err(error) => {
                        runtime.state = AgentRunState::Failed;
                        runtime.diagnostic = Some(error.to_string());
                    }
                }
            }
        }
        let pending = self
            .structured_agents
            .values()
            .any(|runtime| runtime.queued_prompt.is_some());
        let running = self.structured_agents.values().any(|runtime| {
            matches!(
                runtime.state,
                AgentRunState::Starting | AgentRunState::Running
            )
        });
        if !pending && !running {
            self.orchestration_active = false;
            self.status_message = "Dependency run finished.".to_string();
        } else if dispatched {
            self.status_message = "Dependency run started ready tasks.".to_string();
        }
        dispatched
    }

    fn open_context_review_for_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let Some(target) = workspace.canvas.selected_node_id.clone() else {
            self.error_message = "Select a target node first.".to_string();
            cx.notify();
            return;
        };
        let Some(edge) = workspace
            .canvas
            .edges
            .iter()
            .find(|edge| {
                edge.enabled && edge.kind == CanvasEdgeKind::Context && edge.target == target
            })
            .cloned()
        else {
            self.error_message =
                "The selected node has no incoming context link. Link a source to it first."
                    .to_string();
            cx.notify();
            return;
        };
        if let Err(error) = self.ensure_same_execution_host(&edge.source, &target) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }
        let Some(source_node) = workspace
            .canvas
            .nodes
            .iter()
            .find(|node| node.id == edge.source)
            .cloned()
        else {
            return;
        };
        let source_label = source_node
            .title
            .clone()
            .or_else(|| {
                source_node
                    .kind
                    .pane_id()
                    .and_then(|pane_id| self.pane(pane_id).map(|pane| pane.title.clone()))
            })
            .unwrap_or_else(|| "Canvas node".to_string());
        let source_text = if let Some(runtime) = self.structured_agents.get(&source_node.id) {
            runtime.transcript.clone()
        } else if let Some(pane) = source_node
            .kind
            .pane_id()
            .and_then(|pane_id| self.pane(pane_id))
        {
            pane.terminal.all_rows_text().join("\n")
        } else {
            String::new()
        };
        let policy = edge.context_policy.clone().unwrap_or_default();
        let preview =
            build_context_handoff(&source_label, &source_text, &policy, current_unix_millis());
        Self::set_input_value(
            &self.shell_inputs.context_handoff_preview,
            preview.text,
            window,
            cx,
        );
        self.context_handoff_review = Some(ContextHandoffReview {
            edge_id: edge.id,
            target,
            source_label,
            redaction_count: preview.redaction_count,
            truncated: preview.truncated,
        });
        self.error_message.clear();
        cx.notify();
    }

    fn send_context_handoff(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.context_handoff_review.clone() else {
            return;
        };
        let text = self
            .shell_inputs
            .context_handoff_preview
            .read(cx)
            .value()
            .to_string();
        let edge_source = self.active_workspace().and_then(|workspace| {
            workspace
                .canvas
                .edges
                .iter()
                .find(|edge| edge.id == review.edge_id)
                .map(|edge| edge.source.clone())
        });
        let Some(edge_source) = edge_source else {
            self.error_message = "The reviewed context link no longer exists.".to_string();
            cx.notify();
            return;
        };
        if let Err(error) = self.ensure_same_execution_host(&edge_source, &review.target) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }
        let policy = self
            .active_workspace()
            .and_then(|workspace| {
                workspace
                    .canvas
                    .edges
                    .iter()
                    .find(|edge| edge.id == review.edge_id)
            })
            .and_then(|edge| edge.context_policy.clone())
            .unwrap_or_default();
        if text.len() > policy.max_bytes {
            self.error_message = format!(
                "Reviewed context is {} bytes; this link allows at most {} bytes.",
                text.len(),
                policy.max_bytes
            );
            cx.notify();
            return;
        }
        if let Some(runtime) = self.structured_agents.get(&review.target) {
            if let Err(error) = runtime.handle.send_prompt(text) {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
            self.status_message = "Reviewed context sent to the structured agent.".to_string();
        } else {
            let target_pane_id = self.active_workspace().and_then(|workspace| {
                workspace
                    .canvas
                    .nodes
                    .iter()
                    .find(|node| node.id == review.target)
                    .and_then(|node| node.kind.pane_id())
            });
            let Some(pane_id) = target_pane_id else {
                self.error_message = "The target agent is not running.".to_string();
                cx.notify();
                return;
            };
            self.pending_paste = Some(super::PendingPaste { pane_id, text });
            self.status_message =
                "Context is ready. Confirm the guarded paste to deliver it.".to_string();
        }
        self.context_handoff_review = None;
        self.error_message.clear();
        cx.notify();
    }

    fn toggle_canvas_add_menu(&mut self, cx: &mut Context<Self>) {
        self.canvas_add_menu_open = !self.canvas_add_menu_open;
        cx.notify();
    }

    fn render_canvas_add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let profiles = self.saved.profiles.clone();
        v_flex()
            .id("canvas-add-menu")
            .absolute()
            .top(px(10.0))
            .left(px(12.0))
            .w(px(300.0))
            .max_h(px(480.0))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border_dark())
            .bg(theme::library_card())
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(40.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Add to canvas"),
                    )
                    .child(
                        Button::new("canvas-add-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.canvas_add_menu_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("canvas-add-list")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .gap_1()
                    .overflow_y_scroll()
                    .child(
                        Button::new("canvas-add-local")
                            .small()
                            .w_full()
                            .justify_start()
                            .icon(IconName::SquareTerminal)
                            .label("Local Terminal")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_local_terminal_to_canvas(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .px_2()
                            .pt_2()
                            .pb_1()
                            .text_size(px(10.0))
                            .font_semibold()
                            .text_color(theme::text_muted())
                            .child("CODING AGENTS"),
                    )
                    .children(
                        [
                            AgentProvider::Codex,
                            AgentProvider::ClaudeCode,
                            AgentProvider::Gemini,
                            AgentProvider::CustomCli,
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, provider)| {
                            Button::new(("canvas-add-agent", index))
                                .small()
                                .w_full()
                                .justify_start()
                                .icon(IconName::Bot)
                                .label(provider.label())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_agent_creation(provider, window, cx);
                                }))
                        }),
                    )
                    .when(!profiles.is_empty(), |list| {
                        list.child(
                            div()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .text_size(px(10.0))
                                .font_semibold()
                                .text_color(theme::text_muted())
                                .child("SAVED HOSTS"),
                        )
                    })
                    .children(profiles.into_iter().enumerate().map(|(index, profile)| {
                        let profile_id = profile.id.clone();
                        Button::new(("canvas-add-host", index))
                            .small()
                            .w_full()
                            .justify_start()
                            .icon(IconName::Globe)
                            .label(profile.display_name())
                            .tooltip(format!(
                                "{}@{}:{}",
                                profile.username, profile.host, profile.port
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_saved_host_to_canvas(&profile_id, window, cx);
                            }))
                    })),
            )
            .into_any_element()
    }

    fn render_agent_creation_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.agent_creation.as_ref() else {
            return div().into_any_element();
        };
        let provider = state.definition.provider;
        let backend = state.definition.backend;
        let location = state.definition.location.clone();
        let permission_policy = state.definition.permission_policy;
        let worktree_policy = state.definition.worktree;
        let local_status = match &state.executable_status {
            AgentExecutableStatus::Available { path, version } => format!(
                "Available: {}{}",
                path.display(),
                version
                    .as_ref()
                    .map(|version| format!(" ({version})"))
                    .unwrap_or_default()
            ),
            AgentExecutableStatus::Missing {
                requested,
                guidance,
            } => {
                if requested.is_empty() {
                    (*guidance).to_string()
                } else {
                    format!("Not found: {}. {guidance}", requested.to_string_lossy())
                }
            }
        };
        let status_text = if matches!(location, AgentLocation::Local) {
            local_status
        } else {
            "The executable is checked on the remote host when the SSH session opens.".to_string()
        };
        let profiles = self.saved.profiles.clone();

        v_flex()
            .id("agent-creation-panel")
            .absolute()
            .top(px(10.0))
            .left(px(12.0))
            .w(px(520.0))
            .max_h(px(680.0))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border_dark())
            .bg(theme::library_card())
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(44.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::Bot)
                                    .size(px(15.0))
                                    .text_color(theme::accent()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child("New agent"),
                            ),
                    )
                    .child(
                        Button::new("agent-creation-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agent_creation = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("agent-creation-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Provider"),
                            )
                            .child(
                                h_flex().gap_1().children(
                                    [
                                        AgentProvider::Codex,
                                        AgentProvider::ClaudeCode,
                                        AgentProvider::Gemini,
                                        AgentProvider::CustomCli,
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, item)| {
                                        Button::new(("agent-provider", index))
                                            .xsmall()
                                            .custom(Self::segmented_button_style(
                                                provider == item,
                                                cx,
                                            ))
                                            .label(item.label())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.set_agent_creation_provider(item, window, cx);
                                            }))
                                    }),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Experience"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("agent-backend-interactive")
                                            .xsmall()
                                            .custom(Self::segmented_button_style(
                                                backend == AgentBackendKind::InteractivePty,
                                                cx,
                                            ))
                                            .label("Interactive terminal")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_agent_backend(
                                                    AgentBackendKind::InteractivePty,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("agent-backend-structured")
                                            .xsmall()
                                            .custom(Self::segmented_button_style(
                                                backend == AgentBackendKind::Structured,
                                                cx,
                                            ))
                                            .label("Structured")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_agent_backend(
                                                    AgentBackendKind::Structured,
                                                    cx,
                                                );
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Runs on"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("agent-location-local")
                                            .xsmall()
                                            .custom(Self::segmented_button_style(
                                                matches!(location, AgentLocation::Local),
                                                cx,
                                            ))
                                            .label("Local")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_agent_creation_location(
                                                    AgentLocation::Local,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .children(profiles.iter().enumerate().map(
                                        |(index, profile)| {
                                            let profile_id = profile.id.clone();
                                            let active = matches!(
                                                &location,
                                                AgentLocation::SavedHost { profile_id: selected }
                                                    if selected == &profile.id
                                            );
                                            Button::new(("agent-location-host", index))
                                                .xsmall()
                                                .custom(Self::segmented_button_style(active, cx))
                                                .label(profile.display_name())
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_agent_creation_location(
                                                        AgentLocation::SavedHost {
                                                            profile_id: profile_id.clone(),
                                                        },
                                                        cx,
                                                    );
                                                }))
                                        },
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Working directory"),
                            )
                            .child(Input::new(&self.shell_inputs.agent_working_directory).small()),
                    )
                    .when(provider == AgentProvider::CustomCli, |form| {
                        form.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_semibold()
                                        .text_color(theme::text_muted())
                                        .child("Executable"),
                                )
                                .child(Input::new(&self.shell_inputs.agent_executable).small()),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Arguments"),
                            )
                            .child(Input::new(&self.shell_inputs.agent_arguments)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Initial prompt"),
                            )
                            .child(Input::new(&self.shell_inputs.agent_initial_prompt)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Permission policy"),
                            )
                            .child(
                                h_flex().gap_1().children(
                                    [
                                        (AgentPermissionPolicy::ProviderDefault, "Ask as needed"),
                                        (AgentPermissionPolicy::ReadOnly, "Read only"),
                                        (AgentPermissionPolicy::WorkspaceWrite, "Workspace write"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (policy, label))| {
                                            Button::new(("agent-permission", index))
                                                .xsmall()
                                                .custom(Self::segmented_button_style(
                                                    permission_policy == policy,
                                                    cx,
                                                ))
                                                .label(label)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_agent_permission_policy(policy, cx);
                                                }))
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_semibold()
                                    .text_color(theme::text_muted())
                                    .child("Repository isolation"),
                            )
                            .child(
                                h_flex().gap_1().children(
                                    [
                                        (SavedWorktreePolicy::Isolated, "Isolated worktree"),
                                        (SavedWorktreePolicy::SharedDirectory, "Shared directory"),
                                        (SavedWorktreePolicy::ReadOnly, "Read only"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (policy, label))| {
                                            Button::new(("agent-worktree", index))
                                                .xsmall()
                                                .custom(Self::segmented_button_style(
                                                    worktree_policy == policy,
                                                    cx,
                                                ))
                                                .label(label)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_agent_worktree_policy(policy, cx);
                                                }))
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted())
                                    .child(status_text),
                            )
                            .child(
                                Button::new("agent-check-executable")
                                    .xsmall()
                                    .ghost()
                                    .label("Check again")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.check_agent_executable(cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("agent-launch")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .icon(IconName::ArrowRight)
                                .label("Launch Agent")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.launch_agent_creation(window, cx);
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn toggle_worktree_manager(&mut self, cx: &mut Context<Self>) {
        self.worktree_manager_open = !self.worktree_manager_open;
        if self.worktree_manager_open {
            self.canvas_add_menu_open = false;
            self.agent_creation = None;
            self.context_handoff_review = None;
        }
        cx.notify();
    }

    fn inspect_managed_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(worktree) = self
            .saved
            .managed_agent_worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .cloned()
        else {
            self.error_message = "That managed worktree is no longer registered.".to_string();
            cx.notify();
            return;
        };
        match managed_worktree_status(&worktree) {
            Ok(status) => {
                self.status_message = match (status.dirty, status.has_commits_after_base) {
                    (false, false) => format!("{} is clean and can be removed.", worktree.branch),
                    (true, false) => format!("{} has uncommitted changes.", worktree.branch),
                    (false, true) => {
                        format!("{} contains commits after its base.", worktree.branch)
                    }
                    (true, true) => format!(
                        "{} has uncommitted changes and commits after its base.",
                        worktree.branch
                    ),
                };
                self.error_message.clear();
            }
            Err(error) => self.error_message = format!("Unable to inspect worktree: {error:#}"),
        }
        cx.notify();
    }

    fn copy_managed_worktree_path(&mut self, path: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path));
        self.status_message = "Worktree path copied.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn open_managed_worktree_terminal(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !std::path::Path::new(&path).is_dir() {
            self.error_message = format!("Worktree directory does not exist: {path}");
            cx.notify();
            return;
        }
        let mut config = self.saved.settings.default_local_shell.clone();
        config.cwd = Some(path);
        let request = ConnectRequest::local_shell_with_config(0, config);
        if self
            .add_request_to_canvas(request, None, window, cx)
            .is_some()
        {
            self.worktree_manager_open = false;
            self.status_message = "Opened a terminal in the managed worktree.".to_string();
            self.error_message.clear();
        } else {
            self.error_message = "Open a Canvas workspace before opening the worktree.".to_string();
        }
        cx.notify();
    }

    fn remove_registered_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        let referenced_by_agent = self.workspaces.iter().any(|workspace| {
            workspace.canvas.nodes.iter().any(|node| {
                matches!(
                    &node.kind,
                    CanvasNodeKind::Agent { definition, .. }
                        if definition
                            .managed_worktree
                            .as_ref()
                            .is_some_and(|worktree| worktree.path == path)
                )
            })
        });
        let referenced_by_terminal = self.panes.iter().any(|pane| {
            pane.request
                .local_shell
                .as_ref()
                .and_then(|config| config.cwd.as_deref())
                == Some(path)
        });
        if referenced_by_agent || referenced_by_terminal {
            self.error_message =
                "Close every agent and terminal using this worktree before removing it."
                    .to_string();
            cx.notify();
            return;
        }
        let Some(worktree) = self
            .saved
            .managed_agent_worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .cloned()
        else {
            self.error_message = "That managed worktree is no longer registered.".to_string();
            cx.notify();
            return;
        };
        let managed_root = match managed_agent_worktree_dir() {
            Ok(path) => path,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        match remove_managed_worktree(&worktree, &managed_root) {
            Ok(()) => {
                self.saved.forget_managed_agent_worktree(path);
                self.persist_runtime_state();
                self.status_message = format!("Removed clean worktree {}.", worktree.branch);
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = format!("Worktree was not removed: {error:#}");
            }
        }
        cx.notify();
    }

    fn render_worktree_manager(&self, cx: &mut Context<Self>) -> AnyElement {
        let worktrees = self.saved.managed_agent_worktrees.clone();
        v_flex()
            .id("managed-worktree-panel")
            .absolute()
            .top(px(10.0))
            .left(px(12.0))
            .w(px(620.0))
            .max_h(px(620.0))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border_dark())
            .bg(theme::library_card())
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(42.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_semibold()
                            .text_color(theme::text_on_dark())
                            .child("Managed worktrees"),
                    )
                    .child(
                        Button::new("managed-worktree-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close worktree manager")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.worktree_manager_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("managed-worktree-list")
                    .p_3()
                    .gap_2()
                    .overflow_y_scroll()
                    .when(worktrees.is_empty(), |list| {
                        list.child(
                            div()
                                .py_4()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child("No isolated agent worktrees have been created."),
                        )
                    })
                    .children(worktrees.into_iter().enumerate().map(|(index, worktree)| {
                        let inspect_path = worktree.path.clone();
                        let copy_path = worktree.path.clone();
                        let open_path = worktree.path.clone();
                        let remove_path = worktree.path.clone();
                        h_flex()
                            .p_2()
                            .gap_2()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::border_dark())
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_semibold()
                                            .text_color(theme::text_on_dark())
                                            .child(worktree.branch),
                                    )
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(10.0))
                                            .text_color(theme::text_muted())
                                            .child(worktree.path),
                                    ),
                            )
                            .child(
                                Button::new(("worktree-inspect", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Inspector)
                                    .tooltip("Inspect Git status")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.inspect_managed_worktree(&inspect_path, cx);
                                    })),
                            )
                            .child(
                                Button::new(("worktree-copy", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .tooltip("Copy worktree path")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_managed_worktree_path(copy_path.clone(), cx);
                                    })),
                            )
                            .child(
                                Button::new(("worktree-open", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::SquareTerminal)
                                    .tooltip("Open terminal in worktree")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_managed_worktree_terminal(
                                            open_path.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("worktree-remove", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Delete)
                                    .tooltip("Remove clean unused worktree")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_registered_worktree(&remove_path, cx);
                                    })),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_canvas_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let zoom_percent = self
            .active_workspace()
            .map(|workspace| (workspace.canvas.transform.zoom * 100.0).round() as i32)
            .unwrap_or(100);
        h_flex()
            .h(px(CANVAS_TOOLBAR_HEIGHT))
            .w_full()
            .px_3()
            .gap_2()
            .items_center()
            .justify_between()
            .bg(theme::terminal_panel())
            .border_b_1()
            .border_color(theme::border_dark())
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("canvas-add-terminal")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::Plus)
                            .label("Add")
                            .tooltip("Add a terminal or coding agent")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_canvas_add_menu(cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-review-context")
                            .small()
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .label("Review context")
                            .tooltip("Review an incoming context link before sending")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_context_review_for_selected(window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-run-dependencies")
                            .small()
                            .ghost()
                            .icon(IconName::Building2)
                            .label("Run DAG")
                            .tooltip("Run queued tasks when dependencies are satisfied")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_dependency_orchestration(cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-worktrees")
                            .small()
                            .ghost()
                            .icon(IconName::GitHub)
                            .label(format!(
                                "Worktrees ({})",
                                self.saved.managed_agent_worktrees.len()
                            ))
                            .tooltip("Inspect and clean up isolated agent worktrees")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_worktree_manager(cx);
                            })),
                    )
                    .when(self.pending_context_source.is_some(), |toolbar| {
                        toolbar.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::warning())
                                .child("Choose link target"),
                        )
                    })
                    .when(self.pending_dependency_source.is_some(), |toolbar| {
                        toolbar.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::warning())
                                .child("Choose dependency target"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("canvas-zoom-out")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Minus)
                            .tooltip("Zoom out")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_canvas(0.85, window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-zoom-reset")
                            .xsmall()
                            .ghost()
                            .label(format!("{zoom_percent}%"))
                            .tooltip("Reset zoom")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_canvas_zoom(window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-zoom-in")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip("Zoom in")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_canvas(1.15, window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-fit")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Maximize)
                            .tooltip("Fit all nodes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.fit_canvas(window, cx);
                            })),
                    ),
            )
            .map(|toolbar| {
                let _ = window;
                toolbar
            })
    }

    fn render_canvas_edges(&self) -> AnyElement {
        let Some(workspace) = self.active_workspace() else {
            return div().into_any_element();
        };
        let transform = workspace.canvas.transform;
        let node_rects: HashMap<_, _> = workspace
            .canvas
            .nodes
            .iter()
            .map(|node| (node.id.clone(), transform.screen_rect(node.rect)))
            .collect();
        let paths: Vec<_> = workspace
            .canvas
            .edges
            .iter()
            .filter(|edge| edge.enabled)
            .filter_map(|edge| {
                let source = node_rects.get(&edge.source)?;
                let target = node_rects.get(&edge.target)?;
                Some((
                    CanvasPoint::new(source.x + source.width, source.y + source.height / 2.0),
                    CanvasPoint::new(target.x, target.y + target.height / 2.0),
                    edge.kind,
                ))
            })
            .collect();
        let context_color = theme::with_alpha(theme::accent(), 0.72);
        let dependency_color = theme::with_alpha(theme::warning(), 0.78);

        paint_canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                for (source, target, kind) in paths {
                    let color = if kind == CanvasEdgeKind::Context {
                        context_color
                    } else {
                        dependency_color
                    };
                    let offset = ((target.x - source.x).abs() * 0.45).max(48.0);
                    let mut builder = PathBuilder::stroke(px(2.0));
                    builder.move_to(point(px(source.x), px(source.y)));
                    builder.cubic_bezier_to(
                        point(px(target.x), px(target.y)),
                        point(px(source.x + offset), px(source.y)),
                        point(px(target.x - offset), px(target.y)),
                    );
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }

                    let mut arrow = PathBuilder::stroke(px(2.0));
                    arrow.move_to(point(px(target.x - 9.0), px(target.y - 6.0)));
                    arrow.line_to(point(px(target.x), px(target.y)));
                    arrow.line_to(point(px(target.x - 9.0), px(target.y + 6.0)));
                    if let Ok(path) = arrow.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    fn render_canvas_node(
        &self,
        workspace_id: u64,
        node: &CanvasNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(workspace) = self.workspace(workspace_id) else {
            return div().into_any_element();
        };
        let screen = workspace.canvas.transform.screen_rect(node.rect);
        let selected = workspace.canvas.selected_node_id.as_ref() == Some(&node.id);
        let node_id = node.id.clone();
        let pane_id = node.kind.pane_id();
        let title = node
            .title
            .clone()
            .or_else(|| pane_id.and_then(|id| self.pane(id).map(|pane| pane.title.clone())))
            .unwrap_or_else(|| "Agent".to_string());
        let subtitle = self
            .structured_agents
            .get(&node.id)
            .map(|runtime| runtime.state.label().to_string())
            .or_else(|| {
                pane_id
                    .and_then(|id| self.pane(id))
                    .map(|pane| pane.status.clone())
            })
            .unwrap_or_else(|| "Idle".to_string());
        let header_node_id = node_id.clone();
        let link_node_id = node_id.clone();
        let dependency_node_id = node_id.clone();
        let collapse_node_id = node_id.clone();
        let resize_node_id = node_id.clone();
        let close_pane_id = pane_id;
        let close_structured_node_id = (pane_id.is_none()).then_some(node_id.clone());

        let mut body =
            v_flex()
                .id(SharedString::from(format!(
                    "canvas-node-{}",
                    node_id.as_str()
                )))
                .absolute()
                .left(px(screen.x))
                .top(px(screen.y))
                .w(px(screen.width.max(180.0)))
                .h(px(if node.collapsed {
                    CANVAS_NODE_HEADER_HEIGHT
                } else {
                    screen.height.max(CANVAS_NODE_HEADER_HEIGHT + 80.0)
                }))
                .overflow_hidden()
                .rounded(px(7.0))
                .border_1()
                .border_color(if selected {
                    theme::focus_ring()
                } else {
                    theme::border_dark()
                })
                .bg(theme::terminal_panel())
                .child(
                    h_flex()
                        .id(SharedString::from(format!(
                            "canvas-node-header-{}",
                            node_id.as_str()
                        )))
                        .h(px(CANVAS_NODE_HEADER_HEIGHT))
                        .w_full()
                        .px_2()
                        .gap_2()
                        .items_center()
                        .justify_between()
                        .cursor(CursorStyle::OpenHand)
                        .bg(theme::terminal_panel())
                        .border_b_1()
                        .border_color(theme::border_dark())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.start_canvas_node_move(
                                    workspace_id,
                                    header_node_id.clone(),
                                    event,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(match node.kind {
                                        CanvasNodeKind::Terminal { .. } => IconName::SquareTerminal,
                                        CanvasNodeKind::Agent { .. } => IconName::Bot,
                                    })
                                    .size(px(14.0))
                                    .text_color(theme::accent()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(12.0))
                                        .font_semibold()
                                        .text_color(theme::text_on_dark())
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(theme::text_muted_dark())
                                        .child(subtitle),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "canvas-node-collapse-{}",
                                        collapse_node_id.as_str()
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .icon(if node.collapsed {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronUp
                                    })
                                    .tooltip(if node.collapsed {
                                        "Expand node"
                                    } else {
                                        "Collapse node"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_canvas_node_collapsed(
                                            collapse_node_id.clone(),
                                            cx,
                                        );
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "canvas-node-link-{}",
                                        link_node_id.as_str()
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::ArrowRight)
                                    .tooltip("Use as context source or target")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.link_canvas_node(link_node_id.clone(), cx);
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "canvas-node-dependency-{}",
                                        dependency_node_id.as_str()
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Building2)
                                    .tooltip("Use as dependency source or target")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.link_canvas_dependency(dependency_node_id.clone(), cx);
                                    })),
                                ),
                        )
                        .when_some(close_pane_id, |header, pane_id| {
                            header.child(
                                Button::new(("canvas-node-close", pane_id))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip("Close terminal")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_pane(pane_id, cx);
                                    })),
                            )
                        })
                        .when_some(close_structured_node_id, |header, node_id| {
                            header.child(
                                Button::new(SharedString::from(format!(
                                    "structured-node-close-{}",
                                    node_id.as_str()
                                )))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("Stop and close agent")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_structured_agent(node_id.clone(), cx);
                                    },
                                )),
                            )
                        }),
                );

        if !node.collapsed {
            body = body.when_some(pane_id.and_then(|id| self.pane(id)), |body, pane| {
                body.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_terminal_pane(pane, window, cx)),
                )
            });
            if pane_id.is_none() {
                body = body.child(self.render_structured_agent_body(node_id.clone(), selected, cx));
            }
            body = body.child(
                div()
                    .id(SharedString::from(format!(
                        "canvas-node-resize-{}",
                        node_id.as_str()
                    )))
                    .absolute()
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .size(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.start_canvas_node_resize(
                                workspace_id,
                                resize_node_id.clone(),
                                event,
                                cx,
                            );
                        }),
                    )
                    .child(
                        Icon::new(IconName::ResizeCorner)
                            .size(px(12.0))
                            .text_color(theme::text_muted_dark()),
                    ),
            );
        }

        body.into_any_element()
    }

    fn render_structured_agent_body(
        &self,
        node_id: CanvasNodeId,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(runtime) = self.structured_agents.get(&node_id) else {
            let restart_id = node_id.clone();
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme::text_muted_dark())
                        .child("Structured session is not running"),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "structured-restart-{}",
                        node_id.as_str()
                    )))
                    .small()
                    .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                    .icon(IconName::Redo2)
                    .label("Restart")
                    .tooltip("Start a new process from this saved agent definition")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.restart_structured_agent(restart_id.clone(), cx);
                    })),
                )
                .into_any_element();
        };
        let transcript = if runtime.transcript.trim().is_empty() {
            "Ready for a prompt.".to_string()
        } else {
            runtime.transcript.clone()
        };
        let diagnostic = runtime.diagnostic.clone();
        let approval = runtime.approval.clone();
        let task_queued = runtime.queued_prompt.is_some();
        let send_id = node_id.clone();
        let cancel_id = node_id.clone();
        let queue_id = node_id.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "structured-transcript-{}",
                        node_id.as_str()
                    )))
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .overflow_y_scroll()
                    .text_size(px(12.0))
                    .text_color(theme::text_on_dark())
                    .child(transcript),
            )
            .when_some(diagnostic, |body, diagnostic| {
                body.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .bg(theme::with_alpha(theme::danger(), 0.16))
                        .text_size(px(10.0))
                        .text_color(theme::danger())
                        .child(diagnostic),
                )
            })
            .when(task_queued, |body| {
                body.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .bg(theme::with_alpha(theme::accent(), 0.14))
                        .text_size(px(10.0))
                        .text_color(theme::accent())
                        .child("Task queued for dependency run"),
                )
            })
            .when_some(approval, |body, approval| {
                let allow_id = node_id.clone();
                let deny_id = node_id.clone();
                body.child(
                    v_flex()
                        .mx_2()
                        .mb_2()
                        .p_2()
                        .gap_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme::warning())
                        .bg(theme::with_alpha(theme::warning(), 0.12))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_semibold()
                                .text_color(theme::text_on_dark())
                                .child("Approval required"),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::text_muted_dark())
                                .child(approval.operation),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .justify_end()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "structured-deny-{}",
                                        deny_id.as_str()
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .label("Deny")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.respond_structured_agent_approval(
                                            deny_id.clone(),
                                            false,
                                            cx,
                                        );
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "structured-allow-{}",
                                        allow_id.as_str()
                                    )))
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Allow once")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.respond_structured_agent_approval(
                                            allow_id.clone(),
                                            true,
                                            cx,
                                        );
                                    })),
                                ),
                        ),
                )
            })
            .when(selected, |body| {
                body.child(
                    h_flex()
                        .p_2()
                        .gap_2()
                        .items_end()
                        .border_t_1()
                        .border_color(theme::border_dark())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.shell_inputs.structured_agent_prompt)),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "structured-queue-{}",
                                queue_id.as_str()
                            )))
                            .xsmall()
                            .ghost()
                            .label("Queue")
                            .tooltip("Queue task for dependency orchestration")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.queue_structured_agent_task(queue_id.clone(), window, cx);
                                },
                            )),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "structured-cancel-{}",
                                cancel_id.as_str()
                            )))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Cancel active turn")
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.cancel_structured_agent(cancel_id.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "structured-send-{}",
                                send_id.as_str()
                            )))
                            .xsmall()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::ArrowRight)
                            .tooltip("Send prompt")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.send_structured_agent_prompt(send_id.clone(), window, cx);
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_context_handoff_review(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(review) = self.context_handoff_review.as_ref() else {
            return div().into_any_element();
        };
        let details = match (review.redaction_count, review.truncated) {
            (0, false) => "No automatic redactions; snapshot fits the link limit.".to_string(),
            (count, false) => format!("{count} potential secret(s) redacted."),
            (0, true) => "Snapshot truncated to the link limit.".to_string(),
            (count, true) => {
                format!("{count} potential secret(s) redacted; snapshot truncated.")
            }
        };
        v_flex()
            .id("context-handoff-review")
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(560.0))
            .max_h(px(700.0))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border_dark())
            .bg(theme::library_card())
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(44.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(format!("Review context from {}", review.source_label)),
                    )
                    .child(
                        Button::new("context-review-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_handoff_review = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(details),
                    )
                    .child(Input::new(&self.shell_inputs.context_handoff_preview))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("context-review-cancel")
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.context_handoff_review = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("context-review-send")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .icon(IconName::ArrowRight)
                                    .label("Send reviewed context")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send_context_handoff(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_canvas_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex().flex_1();
        };
        let workspace_id = workspace.id;
        let mut node_indices: Vec<_> = (0..workspace.canvas.nodes.len()).collect();
        node_indices.sort_by_key(|index| workspace.canvas.nodes[*index].z_index);

        let mut body = div()
            .id("agent-canvas-body")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(theme::terminal_bg())
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.start_canvas_pan(event, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.handle_canvas_scroll(event, window, cx);
            }))
            .child(self.render_canvas_edges());

        for index in node_indices {
            body = body.child(self.render_canvas_node(
                workspace_id,
                &workspace.canvas.nodes[index],
                window,
                cx,
            ));
        }

        if workspace.canvas.nodes.is_empty() {
            body = body.child(
                v_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(
                        Icon::new(IconName::Map)
                            .size(px(26.0))
                            .text_color(theme::accent()),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_semibold()
                            .text_color(theme::text_on_dark())
                            .child("Add a terminal or coding agent"),
                    )
                    .child(
                        Button::new("canvas-empty-add")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::Plus)
                            .label("Add to Canvas")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_canvas_add_menu(cx);
                            })),
                    ),
            );
        }

        if self.canvas_add_menu_open {
            body = body.child(self.render_canvas_add_menu(cx));
        }
        if self.agent_creation.is_some() {
            body = body.child(self.render_agent_creation_panel(cx));
        }
        if self.context_handoff_review.is_some() {
            body = body.child(self.render_context_handoff_review(cx));
        }
        if self.worktree_manager_open {
            body = body.child(self.render_worktree_manager(cx));
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .bg(theme::terminal_bg())
            .child(self.render_canvas_toolbar(window, cx))
            .when_some(self.render_paste_confirmation(cx), |canvas, banner| {
                canvas.child(banner)
            })
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CANVAS_DEFAULT_NODE_HEIGHT, CANVAS_DEFAULT_NODE_WIDTH, CanvasNode, CanvasNodeKind,
        CanvasPoint, CanvasRect, CanvasTransform, CanvasWorkspaceState,
        find_non_overlapping_position, fit_transform,
    };
    use crate::models::{CanvasNodeId, SavedCanvasState};

    fn terminal_node(id: &str, pane_id: u64, x: f32, y: f32) -> CanvasNode {
        CanvasNode {
            id: CanvasNodeId::new(id),
            kind: CanvasNodeKind::Terminal { pane_id },
            rect: CanvasRect {
                x,
                y,
                width: CANVAS_DEFAULT_NODE_WIDTH,
                height: CANVAS_DEFAULT_NODE_HEIGHT,
            },
            z_index: 0,
            title: None,
            collapsed: false,
        }
    }

    #[test]
    fn transform_round_trips_world_and_screen_points() {
        let transform = CanvasTransform {
            pan_x: 120.0,
            pan_y: -45.0,
            zoom: 1.4,
        };
        let world = CanvasPoint::new(-50.0, 240.0);
        let decoded = transform.screen_to_world(transform.world_to_screen(world));
        assert!((decoded.x - world.x).abs() < 0.001);
        assert!((decoded.y - world.y).abs() < 0.001);
    }

    #[test]
    fn cursor_anchored_zoom_preserves_world_point() {
        let transform = CanvasTransform {
            pan_x: 40.0,
            pan_y: 70.0,
            zoom: 0.8,
        };
        let cursor = CanvasPoint::new(350.0, 280.0);
        let before = transform.screen_to_world(cursor);
        let zoomed = transform.zoom_around(cursor, 1.7);
        let after = zoomed.screen_to_world(cursor);
        assert!((before.x - after.x).abs() < 0.001);
        assert!((before.y - after.y).abs() < 0.001);
    }

    #[test]
    fn zoom_is_clamped() {
        let transform = CanvasTransform::default();
        assert_eq!(
            transform.zoom_around(CanvasPoint::default(), 0.01).zoom,
            0.35
        );
        assert_eq!(
            transform.zoom_around(CanvasPoint::default(), 20.0).zoom,
            2.0
        );
    }

    #[test]
    fn placement_is_deterministic_and_non_overlapping() {
        let center = CanvasPoint::new(500.0, 400.0);
        let first_position = find_non_overlapping_position(
            &[],
            CANVAS_DEFAULT_NODE_WIDTH,
            CANVAS_DEFAULT_NODE_HEIGHT,
            center,
        );
        let first = terminal_node("a", 1, first_position.x, first_position.y);
        let second_position = find_non_overlapping_position(
            std::slice::from_ref(&first),
            CANVAS_DEFAULT_NODE_WIDTH,
            CANVAS_DEFAULT_NODE_HEIGHT,
            center,
        );
        let repeated = find_non_overlapping_position(
            &[first],
            CANVAS_DEFAULT_NODE_WIDTH,
            CANVAS_DEFAULT_NODE_HEIGHT,
            center,
        );
        assert_eq!(second_position, repeated);
        assert_ne!(second_position, first_position);
    }

    #[test]
    fn context_edges_are_directed_unique_and_not_self_referential() {
        let mut canvas = CanvasWorkspaceState::default();
        canvas.nodes = vec![
            terminal_node("source", 1, 0.0, 0.0),
            terminal_node("target", 2, 800.0, 0.0),
        ];
        let edge_id = canvas
            .add_context_edge(CanvasNodeId::new("source"), CanvasNodeId::new("target"))
            .unwrap();
        assert_eq!(edge_id.0, "context-edge-1");
        assert_eq!(canvas.edges[0].source.0, "source");
        assert_eq!(canvas.edges[0].target.0, "target");
        assert!(canvas.edges[0].context_policy.is_some());
        assert!(
            canvas
                .add_context_edge(CanvasNodeId::new("source"), CanvasNodeId::new("target"))
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert!(
            canvas
                .add_context_edge(CanvasNodeId::new("source"), CanvasNodeId::new("source"))
                .unwrap_err()
                .to_string()
                .contains("different target")
        );
    }

    #[test]
    fn dependency_edges_reject_cycles() {
        let mut canvas = CanvasWorkspaceState::default();
        canvas.nodes = vec![
            terminal_node("a", 1, 0.0, 0.0),
            terminal_node("b", 2, 800.0, 0.0),
            terminal_node("c", 3, 1600.0, 0.0),
        ];
        canvas
            .add_dependency_edge(CanvasNodeId::new("a"), CanvasNodeId::new("b"))
            .unwrap();
        canvas
            .add_dependency_edge(CanvasNodeId::new("b"), CanvasNodeId::new("c"))
            .unwrap();
        assert!(
            canvas
                .add_dependency_edge(CanvasNodeId::new("c"), CanvasNodeId::new("a"))
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn fit_transform_handles_empty_single_and_multiple_nodes() {
        assert_eq!(
            fit_transform(&[], 1200.0, 800.0, 48.0),
            CanvasTransform::default()
        );

        let first = terminal_node("a", 1, 0.0, 0.0);
        let single = fit_transform(std::slice::from_ref(&first), 1200.0, 800.0, 48.0);
        let center = single.world_to_screen(CanvasPoint::new(
            CANVAS_DEFAULT_NODE_WIDTH / 2.0,
            CANVAS_DEFAULT_NODE_HEIGHT / 2.0,
        ));
        assert!((center.x - 600.0).abs() < 0.001);
        assert!((center.y - 400.0).abs() < 0.001);

        let second = terminal_node("b", 2, 1200.0, 900.0);
        let many = fit_transform(&[first, second], 1200.0, 800.0, 48.0);
        assert!((0.35..=2.0).contains(&many.zoom));
    }

    #[test]
    fn v1_capacity_geometry_handles_twenty_nodes_and_forty_edges() {
        let mut canvas = CanvasWorkspaceState::default();
        for index in 0..super::CANVAS_V1_SUPPORTED_NODE_COUNT {
            let column = (index % 5) as f32;
            let row = (index / 5) as f32;
            canvas.nodes.push(terminal_node(
                &format!("node-{index}"),
                index as u64 + 1,
                column * 760.0,
                row * 500.0,
            ));
        }
        for offset in [1, 2] {
            for source in 0..super::CANVAS_V1_SUPPORTED_NODE_COUNT {
                let target = (source + offset) % super::CANVAS_V1_SUPPORTED_NODE_COUNT;
                canvas
                    .add_context_edge(
                        CanvasNodeId::new(format!("node-{source}")),
                        CanvasNodeId::new(format!("node-{target}")),
                    )
                    .unwrap();
            }
        }

        assert_eq!(canvas.nodes.len(), super::CANVAS_V1_SUPPORTED_NODE_COUNT);
        assert_eq!(canvas.edges.len(), super::CANVAS_V1_SUPPORTED_EDGE_COUNT);
        let fitted = fit_transform(&canvas.nodes, 1440.0, 900.0, 48.0);
        assert!(fitted.pan_x.is_finite());
        assert!(fitted.pan_y.is_finite());
        assert!((0.35..=2.0).contains(&fitted.zoom));
    }

    #[test]
    fn runtime_saved_round_trip_preserves_node_identity_and_viewport() {
        let mut state = CanvasWorkspaceState::from_saved(None, &[11, 12]);
        state.transform = CanvasTransform {
            pan_x: 10.0,
            pan_y: 20.0,
            zoom: 1.25,
        };
        let indices = [(11, 0), (12, 1)].into_iter().collect();
        let mut saved = state.to_saved(&indices);
        saved.normalize(2);
        let restored = CanvasWorkspaceState::from_saved(Some(&saved), &[101, 102]);

        assert_eq!(restored.transform, state.transform);
        assert_eq!(restored.nodes.len(), 2);
        assert_eq!(restored.nodes[0].id, state.nodes[0].id);
        assert_eq!(restored.nodes[0].kind.pane_id(), Some(101));
        assert_eq!(restored.nodes[1].kind.pane_id(), Some(102));
    }

    #[test]
    fn future_saved_state_is_not_required_for_default_runtime() {
        let state = CanvasWorkspaceState::from_saved(Some(&SavedCanvasState::default()), &[]);
        assert!(state.nodes.is_empty());
        assert_eq!(state.transform, CanvasTransform::default());
    }
}
