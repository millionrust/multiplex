use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, CursorStyle, Div, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, PathBuilder, Point, ScrollWheelEvent, SharedString, Styled,
    Window, canvas as paint_canvas, div, point, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::{
    CANVAS_DEFAULT_NODE_HEIGHT, CANVAS_DEFAULT_NODE_WIDTH, CANVAS_MAX_ZOOM, CANVAS_MIN_NODE_HEIGHT,
    CANVAS_MIN_NODE_WIDTH, CANVAS_MIN_ZOOM, CanvasEdgeId, CanvasEdgeKind, CanvasNodeId,
    SavedAgentDefinition, SavedCanvasEdge, SavedCanvasNode, SavedCanvasNodeKind, SavedCanvasState,
    SavedCanvasViewport, WorkspaceLayoutMode,
};
use crate::ui::app::{TermiRustApp, WorkspaceViewMode};
use crate::ui::theme;

pub(super) const CANVAS_TOOLBAR_HEIGHT: f32 = 44.0;
pub(super) const CANVAS_NODE_HEADER_HEIGHT: f32 = 34.0;
pub(super) const CANVAS_NODE_GUTTER: f32 = 28.0;
const CANVAS_PLACEMENT_STEP_X: f32 = CANVAS_DEFAULT_NODE_WIDTH + CANVAS_NODE_GUTTER;
const CANVAS_PLACEMENT_STEP_Y: f32 = CANVAS_DEFAULT_NODE_HEIGHT + CANVAS_NODE_GUTTER;
const CANVAS_FIT_PADDING: f32 = 48.0;

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

    fn add_local_terminal_to_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            self.open_local_terminal(window, cx);
            return;
        };
        let is_canvas = self
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.layout_mode == WorkspaceLayoutMode::Canvas);
        if !is_canvas {
            self.open_local_terminal(window, cx);
            return;
        }

        let mut request = crate::models::ConnectRequest::local_shell_with_config(
            0,
            self.saved.settings.default_local_shell.clone(),
        );
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
            let node_id = workspace.canvas.add_terminal_node(pane_id, world_center);
            workspace.canvas.select_and_raise(&node_id);
        }
        self.status_message = "Opened a local terminal on the canvas.".to_string();
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
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
                h_flex().gap_1().items_center().child(
                    Button::new("canvas-add-terminal")
                        .small()
                        .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                        .icon(IconName::Plus)
                        .label("Add")
                        .tooltip("Add local terminal")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_local_terminal_to_canvas(window, cx);
                        })),
                ),
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
        let subtitle = pane_id
            .and_then(|id| self.pane(id))
            .map(|pane| pane.status.clone())
            .unwrap_or_else(|| "Idle".to_string());
        let header_node_id = node_id.clone();
        let resize_node_id = node_id.clone();
        let close_pane_id = pane_id;

        let mut body = v_flex()
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
                            .label("Local Terminal")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_local_terminal_to_canvas(window, cx);
                            })),
                    ),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .bg(theme::terminal_bg())
            .child(self.render_canvas_toolbar(window, cx))
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
