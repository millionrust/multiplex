use std::collections::{HashMap, HashSet};

use crate::models::{
    CANVAS_DEFAULT_NODE_HEIGHT, CANVAS_DEFAULT_NODE_WIDTH, CANVAS_MAX_ZOOM, CANVAS_MIN_NODE_HEIGHT,
    CANVAS_MIN_NODE_WIDTH, CANVAS_MIN_ZOOM, CanvasEdgeId, CanvasEdgeKind, CanvasNodeId,
    SavedAgentDefinition, SavedCanvasEdge, SavedCanvasNode, SavedCanvasNodeKind, SavedCanvasState,
    SavedCanvasViewport,
};

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
