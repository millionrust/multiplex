use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClipboardItem, Context, CursorStyle, Div, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Point, ScrollHandle,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled, Window,
    canvas as paint_canvas, div, point, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable as _, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::agents::{
    AgentApprovalRequest, AgentEvent, AgentExecutableStatus, AgentRole, AgentRunState,
    CodexSessionConfig, CodexSessionHandle, HeadlessSessionConfig, HeadlessSessionHandle,
    RemoteCodexSessionConfig, RemoteHeadlessSessionConfig, RemoteHeadlessSessionHandle,
    SchedulableAgent, build_agent_context_handoff, build_context_handoff,
    build_interactive_launch_spec, build_remote_interactive_arguments, create_managed_worktree,
    detect_agent_executable, managed_worktree_status, provider_descriptor, remove_managed_worktree,
    schedule_dependency_dag, spawn_codex_session, spawn_headless_session,
    spawn_remote_codex_session, spawn_remote_headless_session,
};
use crate::models::{
    AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, AuthConfig, AuthMode,
    CANVAS_DEFAULT_NODE_HEIGHT, CANVAS_DEFAULT_NODE_WIDTH, CANVAS_MAX_ZOOM, CANVAS_MIN_NODE_HEIGHT,
    CANVAS_MIN_NODE_WIDTH, CANVAS_MIN_TERMINAL_NODE_WIDTH, CANVAS_MIN_ZOOM, CanvasEdgeId,
    CanvasEdgeKind, CanvasNodeId, ConnectRequest, ConnectionKind, HostProfile, LocalShellConfig,
    SavedAgentDefinition, SavedCanvasEdge, SavedCanvasNode, SavedCanvasNodeKind, SavedCanvasState,
    SavedCanvasViewport, SavedManagedWorktreeDisposition, SavedWorktreePolicy, WorkspaceLayoutMode,
    default_persistent_session_name_from_id,
};
use crate::ssh::SessionCommand;
use crate::ui::app::{TermiRustApp, WorkspaceViewMode};
use crate::ui::keys::TerminalCellPos;
use crate::ui::render_terminal::{
    SelectionRange, display_terminal_text, normalized_selection, selection_contains,
};
use crate::ui::shell::shell_single_quote;
use crate::ui::theme;
use crate::{storage::managed_agent_worktree_dir, ui::util::current_unix_millis};

pub(super) const CANVAS_TOOLBAR_HEIGHT: f32 = 44.0;
const CANVAS_RENDER_OVERSCAN: f32 = 96.0;
const CANVAS_KEYBOARD_REVEAL_PADDING: f32 = 24.0;
pub(super) const CANVAS_NODE_HEADER_HEIGHT: f32 = 34.0;
pub(super) const CANVAS_NODE_GUTTER: f32 = 28.0;
#[cfg(test)]
pub(super) const CANVAS_V1_SUPPORTED_NODE_COUNT: usize = 20;
#[cfg(test)]
pub(super) const CANVAS_V1_SUPPORTED_EDGE_COUNT: usize = 40;
const CANVAS_PLACEMENT_STEP_X: f32 = CANVAS_DEFAULT_NODE_WIDTH + CANVAS_NODE_GUTTER;
const CANVAS_PLACEMENT_STEP_Y: f32 = CANVAS_DEFAULT_NODE_HEIGHT + CANVAS_NODE_GUTTER;
const CANVAS_FIT_PADDING: f32 = 48.0;
const STRUCTURED_TRANSCRIPT_FONT_SIZE: f32 = 12.0;
const STRUCTURED_TRANSCRIPT_LINE_HEIGHT: f32 = 20.0;
const STRUCTURED_TRANSCRIPT_PADDING: f32 = 12.0;

#[derive(Clone, Debug)]
pub(super) struct AgentCreationState {
    definition: SavedAgentDefinition,
    executable_status: AgentExecutableStatus,
}

fn default_agent_backend(provider: AgentProvider) -> AgentBackendKind {
    if matches!(
        provider,
        AgentProvider::Codex | AgentProvider::ClaudeCode | AgentProvider::Gemini
    ) {
        AgentBackendKind::Structured
    } else {
        AgentBackendKind::InteractivePty
    }
}

fn agent_creation_can_launch(
    location: &AgentLocation,
    executable_status: &AgentExecutableStatus,
) -> bool {
    !matches!(location, AgentLocation::Local)
        || matches!(executable_status, AgentExecutableStatus::Available { .. })
}

#[derive(Clone, Debug)]
pub(super) struct ContextHandoffReview {
    pub edge_id: CanvasEdgeId,
    pub target: CanvasNodeId,
    pub source_label: String,
    pub redaction_count: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PendingTmuxClose {
    pub pane_id: u64,
    pub session_name: Option<String>,
    pub confirm_kill: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PendingCanvasPaneClose {
    pub pane_id: u64,
    pub title: String,
}

#[derive(Clone, Debug)]
pub(super) struct SplitPaneChooser {
    pub workspace_id: u64,
    pub selected_pane_ids: Vec<u64>,
}

pub(super) enum StructuredAgentHandle {
    Codex(CodexSessionHandle),
    Headless(HeadlessSessionHandle),
    RemoteHeadless(RemoteHeadlessSessionHandle),
}

impl StructuredAgentHandle {
    fn try_recv(&self) -> Result<AgentEvent, std::sync::mpsc::TryRecvError> {
        match self {
            Self::Codex(handle) => handle.event_rx.try_recv(),
            Self::Headless(handle) => handle.event_rx.try_recv(),
            Self::RemoteHeadless(handle) => handle.event_rx.try_recv(),
        }
    }

    fn send_prompt(&self, prompt: String) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.send_prompt(prompt),
            Self::Headless(handle) => handle.send_prompt(prompt),
            Self::RemoteHeadless(handle) => handle.send_prompt(prompt),
        }
    }

    fn cancel(&self) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.cancel(),
            Self::Headless(handle) => handle.cancel(),
            Self::RemoteHeadless(handle) => handle.cancel(),
        }
    }

    fn respond_to_approval(&self, request_id: &str, allow: bool) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.respond_to_approval(request_id, allow),
            Self::Headless(_) | Self::RemoteHeadless(_) => {
                anyhow::bail!("This provider did not expose an approval request")
            }
        }
    }
}

pub(super) struct StructuredAgentRuntime {
    pub handle: StructuredAgentHandle,
    pub state: AgentRunState,
    pub transcript: String,
    pub context_messages: Vec<(AgentRole, String)>,
    pub approval: Option<AgentApprovalRequest>,
    pub diagnostic: Option<String>,
    pub queued_prompt: Option<String>,
    pub transcript_scroll: ScrollHandle,
    pub follow_transcript: bool,
    pub selection: Option<SelectionRange>,
    pub dragging_selection: bool,
    pub transcript_focus: FocusHandle,
}

impl StructuredAgentRuntime {
    const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

    fn new(
        handle: StructuredAgentHandle,
        initial_prompt: Option<&str>,
        transcript_focus: FocusHandle,
    ) -> Self {
        let mut runtime = Self {
            handle,
            state: AgentRunState::Starting,
            transcript: String::new(),
            context_messages: Vec::new(),
            approval: None,
            diagnostic: None,
            queued_prompt: None,
            transcript_scroll: ScrollHandle::new(),
            follow_transcript: true,
            selection: None,
            dragging_selection: false,
            transcript_focus,
        };
        if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
            runtime.push_context_message(AgentRole::User, prompt.trim());
        }
        runtime
    }

    fn push_context_message(&mut self, role: AgentRole, text: &str) {
        if let Some((last_role, last_text)) = self.context_messages.last_mut()
            && *last_role == role
        {
            last_text.push_str(text);
        } else {
            self.context_messages.push((role, text.to_string()));
            if self.context_messages.len() > 100 {
                self.context_messages.remove(0);
            }
        }
        let mut excess = self
            .context_messages
            .iter()
            .map(|(_, text)| text.len())
            .sum::<usize>()
            .saturating_sub(Self::MAX_TRANSCRIPT_BYTES);
        while excess > 0 && !self.context_messages.is_empty() {
            let first_len = self.context_messages[0].1.len();
            if first_len <= excess {
                excess -= first_len;
                self.context_messages.remove(0);
                continue;
            }
            let mut drain_end = excess;
            while !self.context_messages[0].1.is_char_boundary(drain_end) {
                drain_end += 1;
            }
            self.context_messages[0].1.drain(..drain_end);
            excess = 0;
        }
    }

    pub(super) fn push_text(&mut self, text: &str) {
        self.transcript.push_str(text);
        if self.transcript.len() > Self::MAX_TRANSCRIPT_BYTES {
            let mut start = self.transcript.len() - Self::MAX_TRANSCRIPT_BYTES;
            while !self.transcript.is_char_boundary(start) {
                start += 1;
            }
            self.transcript.drain(..start);
            self.selection = None;
            self.dragging_selection = false;
        }
        if self.follow_transcript {
            self.transcript_scroll.scroll_to_bottom();
        }
    }
}

fn structured_transcript_lines(transcript: &str) -> Vec<&str> {
    let visible = transcript.trim_end_matches(['\r', '\n']);
    if visible.is_empty() {
        vec![""]
    } else {
        visible
            .split('\n')
            .map(|line| line.trim_end_matches('\r'))
            .collect()
    }
}

fn structured_transcript_selected_text(
    transcript: &str,
    selection: SelectionRange,
) -> Option<String> {
    let selection = normalized_selection(selection)?;
    let lines = structured_transcript_lines(transcript);
    let start_row = usize::from(selection.anchor.row);
    let end_row = usize::from(selection.head.row);
    if start_row >= lines.len() || end_row >= lines.len() {
        return None;
    }

    let mut selected = String::new();
    for row in start_row..=end_row {
        if row > start_row {
            selected.push('\n');
        }
        let line = lines[row];
        let start_col = if row == start_row {
            usize::from(selection.anchor.col)
        } else {
            0
        };
        let end_col = if row == end_row {
            usize::from(selection.head.col)
        } else {
            line.chars().count()
        };
        selected.extend(
            line.chars()
                .skip(start_col)
                .take(end_col.saturating_sub(start_col)),
        );
    }

    (!selected.is_empty()).then_some(selected)
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

fn canvas_node_render_rect(transform: CanvasTransform, node: &CanvasNode) -> CanvasRect {
    let mut screen = transform.screen_rect(node.rect);
    screen.width = screen.width.max(180.0);
    screen.height = if node.collapsed {
        CANVAS_NODE_HEADER_HEIGHT
    } else {
        screen.height.max(CANVAS_NODE_HEADER_HEIGHT + 80.0)
    };
    screen
}

fn canvas_rect_is_visible(
    rect: CanvasRect,
    viewport_width: f32,
    viewport_height: f32,
    overscan: f32,
) -> bool {
    rect.x + rect.width >= -overscan
        && rect.y + rect.height >= -overscan
        && rect.x <= viewport_width + overscan
        && rect.y <= viewport_height + overscan
}

fn canvas_reveal_delta(
    rect: CanvasRect,
    viewport_width: f32,
    viewport_height: f32,
    padding: f32,
) -> CanvasPoint {
    fn axis_delta(start: f32, extent: f32, viewport_extent: f32, padding: f32) -> f32 {
        let available = (viewport_extent - padding * 2.0).max(1.0);
        if extent > available {
            return viewport_extent / 2.0 - (start + extent / 2.0);
        }
        if start < padding {
            return padding - start;
        }
        let end = start + extent;
        let maximum = viewport_extent - padding;
        if end > maximum {
            return maximum - end;
        }
        0.0
    }

    CanvasPoint::new(
        axis_delta(rect.x, rect.width, viewport_width, padding),
        axis_delta(rect.y, rect.height, viewport_height, padding),
    )
}

fn agent_state_needs_attention(state: AgentRunState) -> bool {
    matches!(
        state,
        AgentRunState::WaitingForApproval
            | AgentRunState::Blocked
            | AgentRunState::Failed
            | AgentRunState::Disconnected
    )
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

    pub(super) fn set_node_title(
        &mut self,
        node_id: &CanvasNodeId,
        title: impl Into<String>,
    ) -> bool {
        let Some(node) = self.node_mut(node_id) else {
            return false;
        };
        let title = title.into();
        node.title = match title.trim() {
            "" => None,
            trimmed => Some(trimmed.to_string()),
        };
        true
    }

    pub(super) fn set_edge_enabled(&mut self, edge_id: &CanvasEdgeId, enabled: bool) -> bool {
        let Some(edge) = self.edges.iter_mut().find(|edge| &edge.id == edge_id) else {
            return false;
        };
        edge.enabled = enabled;
        true
    }

    pub(super) fn remove_edge(&mut self, edge_id: &CanvasEdgeId) -> bool {
        let previous_len = self.edges.len();
        self.edges.retain(|edge| &edge.id != edge_id);
        self.edges.len() != previous_len
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

    pub(super) fn select_adjacent_node(&mut self, delta: isize) -> Option<CanvasNodeId> {
        if self.nodes.is_empty() {
            self.selected_node_id = None;
            return None;
        }
        let current = self
            .selected_node_id
            .as_ref()
            .and_then(|selected| self.nodes.iter().position(|node| &node.id == selected))
            .unwrap_or_else(|| if delta < 0 { 0 } else { self.nodes.len() - 1 });
        let next = (current as isize + delta).rem_euclid(self.nodes.len() as isize) as usize;
        let node_id = self.nodes[next].id.clone();
        self.select_and_raise(&node_id);
        Some(node_id)
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

pub(super) fn clamp_node_rect(mut rect: CanvasRect, min_width: f32) -> CanvasRect {
    if !rect.x.is_finite() {
        rect.x = 0.0;
    }
    if !rect.y.is_finite() {
        rect.y = 0.0;
    }
    rect.width = if rect.width.is_finite() {
        rect.width.max(min_width)
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
    pub(super) fn request_canvas_pane_close(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some((persistent_session, session_name, connected, title)) =
            self.pane(pane_id).map(|pane| {
                (
                    pane.request.persistent_session,
                    pane.request.persistent_session_name.clone(),
                    pane.connected && !pane.closed,
                    pane.title.clone(),
                )
            })
        else {
            return;
        };
        if !persistent_session {
            if connected {
                self.pending_canvas_pane_close = Some(PendingCanvasPaneClose { pane_id, title });
                self.canvas_add_menu_open = false;
                self.canvas_links_open = false;
                self.worktree_manager_open = false;
                self.context_handoff_review = None;
                self.pending_tmux_close = None;
                self.split_pane_chooser = None;
                cx.notify();
            } else {
                self.close_pane(pane_id, cx);
            }
            return;
        }
        self.pending_tmux_close = Some(PendingTmuxClose {
            pane_id,
            session_name,
            confirm_kill: false,
        });
        self.canvas_add_menu_open = false;
        self.canvas_links_open = false;
        self.canvas_node_menu_id = None;
        self.worktree_manager_open = false;
        self.context_handoff_review = None;
        self.pending_canvas_pane_close = None;
        self.split_pane_chooser = None;
        cx.notify();
    }

    fn detach_tmux_node_from_canvas(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_tmux_close.take() else {
            return;
        };
        self.close_pane(pending.pane_id, cx);
        self.status_message =
            "Detached from canvas; the tmux session is still running.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn disconnect_tmux_client(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_tmux_close.take() else {
            return;
        };
        if let Some(pane) = self.pane_mut(pending.pane_id) {
            pane.user_closed = true;
            pane.auto_reconnect_at = None;
            let _ = pane.runtime.command_tx.send(SessionCommand::Disconnect);
            pane.connected = false;
            pane.closed = true;
            pane.status = "Disconnected".to_string();
        }
        self.persist_runtime_state();
        self.status_message =
            "Disconnected the client; use Reconnect to attach this node again.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn request_tmux_kill_confirmation(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.pending_tmux_close.as_mut() {
            pending.confirm_kill = true;
        }
        cx.notify();
    }

    fn confirm_tmux_session_kill(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_tmux_close.take() else {
            return;
        };
        let Some(session_name) = pending.session_name else {
            self.error_message =
                "This connection has no tmux session name, so TermiRust cannot kill it safely."
                    .to_string();
            cx.notify();
            return;
        };
        let sent = self.pane(pending.pane_id).is_some_and(|pane| {
            pane.runtime
                .command_tx
                .send(SessionCommand::KillTmuxSession {
                    session_name: session_name.clone(),
                })
                .is_ok()
        });
        if !sent {
            self.error_message = "The SSH session is no longer available.".to_string();
            cx.notify();
            return;
        }
        if let Some(pane) = self.pane_mut(pending.pane_id) {
            pane.user_closed = true;
            pane.auto_reconnect_at = None;
            pane.status = "Killing tmux".to_string();
        }
        self.status_message = format!("Requested deletion of tmux session {session_name}.");
        self.error_message.clear();
        cx.notify();
    }

    fn start_canvas_node_rename(
        &mut self,
        node_id: CanvasNodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(title) = self.active_workspace().and_then(|workspace| {
            workspace.canvas.node(&node_id).map(|node| {
                node.title
                    .clone()
                    .or_else(|| {
                        node.kind
                            .pane_id()
                            .and_then(|pane_id| self.pane(pane_id))
                            .map(|pane| pane.title.clone())
                    })
                    .unwrap_or_else(|| "Agent".to_string())
            })
        }) else {
            return;
        };
        self.canvas_node_rename_id = Some(node_id);
        Self::set_input_value(&self.canvas_node_rename_input, title, window, cx);
        self.canvas_node_rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn commit_canvas_node_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node_id) = self.canvas_node_rename_id.take() else {
            return;
        };
        let title = self.canvas_node_rename_input.read(cx).value().to_string();
        let renamed = self
            .active_workspace_mut()
            .is_some_and(|workspace| workspace.canvas.set_node_title(&node_id, title));
        if renamed {
            self.persist_runtime_state();
            self.status_message = "Canvas node title updated.".to_string();
            self.error_message.clear();
        }
        self.focus_canvas_node_terminal(&node_id, window);
        cx.notify();
    }

    pub(super) fn cancel_canvas_node_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node_id = self.canvas_node_rename_id.take();
        if let Some(node_id) = node_id {
            self.focus_canvas_node_terminal(&node_id, window);
        }
        cx.notify();
    }

    fn focus_canvas_node_terminal(&self, node_id: &CanvasNodeId, window: &mut Window) {
        let pane_id = self
            .active_workspace()
            .and_then(|workspace| workspace.canvas.node(node_id))
            .and_then(|node| node.kind.pane_id());
        if let Some(pane) = pane_id.and_then(|pane_id| self.pane(pane_id)) {
            pane.terminal_focus.focus(window);
        }
    }

    pub(super) fn handle_canvas_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_canvas = self
            .active_workspace()
            .is_some_and(|workspace| workspace.layout_mode == WorkspaceLayoutMode::Canvas);
        if !is_canvas || !event.keystroke.modifiers.secondary() {
            return false;
        }
        if !event.keystroke.modifiers.shift && event.keystroke.key.as_str() == "c" {
            let selected_node_id = self
                .active_workspace()
                .and_then(|workspace| workspace.canvas.selected_node_id.clone());
            return selected_node_id
                .as_ref()
                .is_some_and(|node_id| self.copy_structured_agent_transcript(node_id, cx));
        }
        if !event.keystroke.modifiers.shift {
            return false;
        }
        let delta = match event.keystroke.key.as_str() {
            "left" | "up" => -1,
            "right" | "down" => 1,
            _ => return false,
        };
        let selected = self
            .active_workspace_mut()
            .and_then(|workspace| workspace.canvas.select_adjacent_node(delta));
        if let Some(selected) = selected.as_ref() {
            let viewport = window.viewport_size();
            let viewport_width = f32::from(viewport.width);
            let viewport_height =
                (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT)
                    .max(1.0);
            if let Some(workspace) = self.active_workspace_mut()
                && let Some(node) = workspace.canvas.node(selected)
            {
                let screen = canvas_node_render_rect(workspace.canvas.transform, node);
                let reveal = canvas_reveal_delta(
                    screen,
                    viewport_width,
                    viewport_height,
                    CANVAS_KEYBOARD_REVEAL_PADDING,
                );
                workspace.canvas.transform.pan_x += reveal.x;
                workspace.canvas.transform.pan_y += reveal.y;
            }
        }
        let pane_id = selected.as_ref().and_then(|node_id| {
            self.active_workspace()
                .and_then(|workspace| workspace.canvas.node(node_id))
                .and_then(|node| node.kind.pane_id())
        });
        if let Some(pane_id) = pane_id {
            if let Some(workspace) = self.active_workspace_mut() {
                workspace.active_pane_id = pane_id;
            }
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
        }
        self.persist_runtime_state();
        cx.notify();
        selected.is_some()
    }

    fn open_split_pane_chooser(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        let pane_ids = workspace.pane_ids.clone();
        let active_pane_id = workspace.active_pane_id;
        let mut selected_pane_ids = workspace
            .layout
            .as_ref()
            .map(|layout| layout.leaf_ids())
            .unwrap_or_default()
            .into_iter()
            .filter(|pane_id| pane_ids.contains(pane_id))
            .take(super::MAX_SPLIT_PANES)
            .collect::<Vec<_>>();
        if selected_pane_ids.is_empty() && pane_ids.contains(&active_pane_id) {
            selected_pane_ids.push(active_pane_id);
        }
        for pane_id in pane_ids {
            if selected_pane_ids.len() >= super::MAX_SPLIT_PANES {
                break;
            }
            if !selected_pane_ids.contains(&pane_id) {
                selected_pane_ids.push(pane_id);
            }
        }
        self.split_pane_chooser = Some(SplitPaneChooser {
            workspace_id,
            selected_pane_ids,
        });
        self.canvas_add_menu_open = false;
        self.canvas_links_open = false;
        self.worktree_manager_open = false;
        self.context_handoff_review = None;
        self.pending_tmux_close = None;
        self.pending_canvas_pane_close = None;
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn confirm_canvas_pane_close(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_canvas_pane_close.take() else {
            return;
        };
        self.close_pane(pending.pane_id, cx);
        self.status_message = format!("Closed active terminal {}.", pending.title);
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn toggle_split_pane_choice(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some(chooser) = self.split_pane_chooser.as_mut() else {
            return;
        };
        if let Some(index) = chooser
            .selected_pane_ids
            .iter()
            .position(|selected| *selected == pane_id)
        {
            chooser.selected_pane_ids.remove(index);
            self.error_message.clear();
        } else if chooser.selected_pane_ids.len() >= super::MAX_SPLIT_PANES {
            self.error_message = format!(
                "Split view can show at most {} sessions. Deselect one first.",
                super::MAX_SPLIT_PANES
            );
        } else {
            chooser.selected_pane_ids.push(pane_id);
            self.error_message.clear();
        }
        cx.notify();
    }

    pub(super) fn confirm_split_pane_choice(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chooser) = self.split_pane_chooser.take() else {
            return;
        };
        if chooser.selected_pane_ids.is_empty() {
            self.error_message = "Choose at least one session for Split view.".to_string();
            self.split_pane_chooser = Some(chooser);
            cx.notify();
            return;
        }
        let Some((selected_count, total_count)) = self
            .workspace_mut(chooser.workspace_id)
            .and_then(|workspace| {
                let selected_pane_ids = chooser
                    .selected_pane_ids
                    .into_iter()
                    .filter(|pane_id| workspace.pane_ids.contains(pane_id))
                    .take(super::MAX_SPLIT_PANES)
                    .collect::<Vec<_>>();
                if selected_pane_ids.is_empty() {
                    return None;
                }
                workspace.layout =
                    super::flat_split(&selected_pane_ids, crate::models::SplitAxis::Horizontal);
                workspace.layout_mode = WorkspaceLayoutMode::Split;
                workspace.view_mode = WorkspaceViewMode::Terminal;
                if !selected_pane_ids.contains(&workspace.active_pane_id) {
                    workspace.active_pane_id = selected_pane_ids[0];
                }
                Some((selected_pane_ids.len(), workspace.pane_ids.len()))
            })
        else {
            self.error_message = "The selected sessions are no longer available.".to_string();
            cx.notify();
            return;
        };
        self.canvas_interaction = None;
        self.status_message = format!(
            "Showing {} of {} sessions in Split. All sessions remain available in Canvas.",
            selected_count, total_count
        );
        self.error_message.clear();
        self.persist_runtime_state();
        self.sync_terminal_layout(window, cx);
        cx.notify();
    }

    pub(super) fn set_workspace_layout_mode(
        &mut self,
        mode: WorkspaceLayoutMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some((current_mode, pane_count)) = self
            .workspace(workspace_id)
            .map(|workspace| (workspace.layout_mode, workspace.pane_ids.len()))
        else {
            return;
        };
        if current_mode == mode {
            return;
        }
        if mode == WorkspaceLayoutMode::Split && pane_count > super::MAX_SPLIT_PANES {
            self.open_split_pane_chooser(workspace_id, cx);
            return;
        }
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
        self.canvas_node_menu_id = None;
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
        let workspace_id = workspace.id;
        let start_pan = CanvasPoint::new(
            workspace.canvas.transform.pan_x,
            workspace.canvas.transform.pan_y,
        );
        self.canvas_node_menu_id = None;
        self.canvas_interaction = Some(CanvasInteraction::Pan {
            workspace_id,
            start: point_from_pixels(event.position),
            start_pan,
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
        if let Some(pane_id) = pane_id
            && let Some(pane) = self.pane(pane_id)
        {
            pane.terminal_focus.focus(window);
        }
        cx.notify();
    }

    pub(super) fn activate_canvas_node(
        &mut self,
        workspace_id: u64,
        node_id: CanvasNodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let completes_dependency = self
            .pending_dependency_source
            .as_ref()
            .is_some_and(|source| source != &node_id);
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
        if let Some(pane) = pane_id.and_then(|pane_id| self.pane(pane_id)) {
            pane.terminal_focus.focus(window);
        }
        if completes_dependency {
            self.link_canvas_dependency(node_id, cx);
            return;
        }
        self.persist_runtime_state();
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
                    let min_width = if node.kind.pane_id().is_some() {
                        CANVAS_MIN_TERMINAL_NODE_WIDTH
                    } else {
                        CANVAS_MIN_NODE_WIDTH
                    };
                    node.rect = clamp_node_rect(node.rect, min_width);
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
                    let min_width = if node.kind.pane_id().is_some() {
                        CANVAS_MIN_TERMINAL_NODE_WIDTH
                    } else {
                        CANVAS_MIN_NODE_WIDTH
                    };
                    node.rect = clamp_node_rect(node.rect, min_width);
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

    pub(super) fn fit_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn add_request_to_canvas(
        &mut self,
        mut request: ConnectRequest,
        agent_definition: Option<SavedAgentDefinition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let workspace_id = self.active_workspace_id?;
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
        let backend = default_agent_backend(provider);
        let definition = SavedAgentDefinition {
            provider,
            backend,
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
        self.canvas_links_open = false;
        self.worktree_manager_open = false;
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

    pub(super) fn set_agent_creation_location(
        &mut self,
        location: AgentLocation,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.agent_creation.as_mut() {
            state.definition.location = location;
            if !matches!(state.definition.location, AgentLocation::Local)
                && state.definition.worktree == SavedWorktreePolicy::Isolated
            {
                state.definition.worktree = SavedWorktreePolicy::SharedDirectory;
            }
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
        let working_directory = definition
            .working_directory
            .as_deref()
            .map(str::trim)
            .filter(|directory| !directory.is_empty())
            .unwrap_or(".");
        let directory = shell_single_quote(working_directory);
        let directory_error = shell_single_quote(&format!(
            "TermiRust could not use remote working directory: {working_directory}"
        ));
        let version_error = shell_single_quote(&format!(
            "TermiRust found {executable}, but its version check failed. Update or repair the CLI before reconnecting."
        ));
        Ok(format!(
            "if ! command -v {executable} >/dev/null 2>&1; then printf '%s\\n' {guidance} >&2; exec \"${{SHELL:-/bin/sh}}\"; fi\nif ! {executable} {version_argument} >/dev/null 2>&1; then printf '%s\\n' {version_error} >&2; exec \"${{SHELL:-/bin/sh}}\"; fi\nif [ ! -d {directory} ] || [ ! -r {directory} ] || [ ! -x {directory} ]; then printf '%s\\n' {directory_error} >&2; exec \"${{SHELL:-/bin/sh}}\"; fi\nif ! cd -- {directory}; then printf '%s\\n' {directory_error} >&2; exec \"${{SHELL:-/bin/sh}}\"; fi\nexec {command}",
            executable = shell_single_quote(executable),
            version_argument = shell_single_quote(descriptor.version_argument),
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
                request.startup_directory = None;
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
        creation: AgentCreationState,
        mut definition: SavedAgentDefinition,
        initial_prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &definition.location {
            AgentLocation::Local => {
                let Some(working_directory) = definition
                    .working_directory
                    .as_deref()
                    .filter(|path| !path.trim().is_empty())
                    .map(std::path::PathBuf::from)
                else {
                    self.agent_creation = Some(creation);
                    self.error_message =
                        "Choose a Git repository or working directory.".to_string();
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
                    definition.working_directory = Some(managed.path.clone());
                    self.saved.register_managed_agent_worktree(managed.clone());
                    self.persist_runtime_state();
                    definition.managed_worktree = Some(managed);
                }
            }
            AgentLocation::SavedHost { .. }
                if definition.worktree == SavedWorktreePolicy::Isolated =>
            {
                self.agent_creation = Some(creation);
                self.error_message = "Automatic isolated worktrees are local-only. Choose Shared directory or Read only for the remote host.".to_string();
                cx.notify();
                return;
            }
            AgentLocation::SavedHost { .. } => {}
        }
        let initial_prompt = (!initial_prompt.trim().is_empty()).then_some(initial_prompt);
        let context_initial_prompt = initial_prompt.clone();
        let handle = match self.start_structured_agent_handle(&definition, initial_prompt) {
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
        let provider_label = definition.provider.label();
        let node_id = workspace
            .canvas
            .add_agent_node(None, definition, world_center);
        workspace.canvas.select_and_raise(&node_id);
        let transcript_focus = cx.focus_handle().tab_stop(true);
        self.structured_agents.insert(
            node_id,
            StructuredAgentRuntime::new(
                handle,
                context_initial_prompt.as_deref(),
                transcript_focus,
            ),
        );
        self.canvas_add_menu_open = false;
        self.status_message = format!("Starting structured {provider_label} session...");
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    fn start_structured_agent_handle(
        &mut self,
        definition: &SavedAgentDefinition,
        initial_prompt: Option<String>,
    ) -> anyhow::Result<StructuredAgentHandle> {
        match &definition.location {
            AgentLocation::Local => {
                let executable = match detect_agent_executable(definition) {
                    AgentExecutableStatus::Available { path, .. } => path,
                    AgentExecutableStatus::Missing {
                        requested,
                        guidance,
                    } => anyhow::bail!(
                        "{} executable '{}' is unavailable. {guidance}",
                        definition.provider.label(),
                        requested.to_string_lossy()
                    ),
                    AgentExecutableStatus::Unusable {
                        path,
                        error,
                        guidance,
                    } => anyhow::bail!(
                        "{} was found at {}, but its version check failed: {error}. {guidance}",
                        definition.provider.label(),
                        path.display()
                    ),
                };
                let working_directory = definition
                    .working_directory
                    .as_deref()
                    .filter(|path| !path.trim().is_empty())
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("Choose a working directory"))?;
                match definition.provider {
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
                }
            }
            AgentLocation::SavedHost { profile_id } => {
                if definition.worktree == SavedWorktreePolicy::Isolated {
                    anyhow::bail!(
                        "Automatic isolated worktrees are local-only. Choose Shared directory or Read only."
                    );
                }
                let profile = self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("The selected remote host no longer exists"))?;
                let mut request = self.connect_request_for_saved_canvas_host(&profile)?;
                request.session_id = self.next_session_id();
                request.title = format!(
                    "Structured {} on {}",
                    definition.provider.label(),
                    profile.display_name()
                );
                request.startup_directory = None;
                request.startup_command = None;
                request.persistent_session = false;
                request.persistent_session_name = None;
                request.persistent_session_detach_others = false;
                let known_hosts = self.known_hosts.clone();
                let keepalive_secs = self.saved.settings.ssh_keepalive_secs;
                match definition.provider {
                    AgentProvider::Codex => spawn_remote_codex_session(RemoteCodexSessionConfig {
                        definition: definition.clone(),
                        request,
                        known_hosts,
                        keepalive_secs,
                        initial_prompt,
                    })
                    .map(StructuredAgentHandle::Codex),
                    AgentProvider::ClaudeCode | AgentProvider::Gemini => {
                        spawn_remote_headless_session(RemoteHeadlessSessionConfig {
                            definition: definition.clone(),
                            request,
                            known_hosts,
                            keepalive_secs,
                            initial_prompt,
                        })
                        .map(StructuredAgentHandle::RemoteHeadless)
                    }
                    AgentProvider::CustomCli | AgentProvider::GroqApi => {
                        Err(anyhow::anyhow!("This provider has no structured adapter"))
                    }
                }
            }
        }
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
                AgentEvent::MessageDelta { role, text } => {
                    runtime.push_text(&text);
                    runtime.push_context_message(role, &text);
                }
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
        if changed && let Some(workspace_id) = self.orchestration_workspace_id {
            self.dispatch_ready_agent_tasks(workspace_id);
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
                    runtime.push_context_message(AgentRole::User, prompt.trim());
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
        if let Some(runtime) = self.structured_agents.get(&node_id)
            && let Err(error) = runtime.handle.cancel()
        {
            self.error_message = error.to_string();
        }
        cx.notify();
    }

    fn pause_structured_transcript_follow(
        &mut self,
        node_id: &CanvasNodeId,
        cx: &mut Context<Self>,
    ) {
        if let Some(runtime) = self.structured_agents.get_mut(node_id)
            && runtime.follow_transcript
        {
            runtime.follow_transcript = false;
            cx.notify();
        }
    }

    fn resume_structured_transcript_follow(
        &mut self,
        node_id: &CanvasNodeId,
        cx: &mut Context<Self>,
    ) {
        if let Some(runtime) = self.structured_agents.get_mut(node_id) {
            runtime.follow_transcript = true;
            runtime.selection = None;
            runtime.dragging_selection = false;
            runtime.transcript_scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn structured_transcript_cell_position(
        &self,
        node_id: &CanvasNodeId,
        position: Point<gpui::Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<TerminalCellPos> {
        let runtime = self.structured_agents.get(node_id)?;
        let lines = structured_transcript_lines(&runtime.transcript);
        let bounds = runtime.transcript_scroll.bounds();
        let offset = runtime.transcript_scroll.offset();
        let font_id = window
            .text_system()
            .resolve_font(&gpui::font(self.terminal_font_family(cx)));
        let char_width = window
            .text_system()
            .ch_advance(font_id, px(STRUCTURED_TRANSCRIPT_FONT_SIZE))
            .map(f32::from)
            .unwrap_or(7.2)
            .max(1.0);
        let position_x = f32::from(position.x);
        let position_y = f32::from(position.y);
        let content_x = position_x
            - f32::from(bounds.left())
            - STRUCTURED_TRANSCRIPT_PADDING
            - f32::from(offset.x);
        let content_y = position_y
            - f32::from(bounds.top())
            - STRUCTURED_TRANSCRIPT_PADDING
            - f32::from(offset.y);
        let row = (content_y / STRUCTURED_TRANSCRIPT_LINE_HEIGHT)
            .floor()
            .max(0.0) as usize;
        let row = row.min(lines.len().saturating_sub(1));
        let line_len = lines[row].chars().count();
        let col = (content_x / char_width).floor().max(0.0) as usize;
        let col = col.min(line_len.saturating_sub(1));

        Some(TerminalCellPos {
            row: row.min(usize::from(u16::MAX)) as u16,
            col: col.min(usize::from(u16::MAX)) as u16,
        })
    }

    fn start_structured_transcript_selection(
        &mut self,
        node_id: &CanvasNodeId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        let Some(position) =
            self.structured_transcript_cell_position(node_id, event.position, window, cx)
        else {
            return;
        };
        if let Some(runtime) = self.structured_agents.get_mut(node_id) {
            runtime.transcript_focus.focus(window);
            runtime.selection = Some(SelectionRange {
                anchor: position,
                head: position,
            });
            runtime.dragging_selection = true;
            runtime.follow_transcript = false;
        }
        cx.notify();
    }

    fn update_structured_transcript_selection(
        &mut self,
        node_id: &CanvasNodeId,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging()
            || !self
                .structured_agents
                .get(node_id)
                .is_some_and(|runtime| runtime.dragging_selection)
        {
            return;
        }
        let Some(position) =
            self.structured_transcript_cell_position(node_id, event.position, window, cx)
        else {
            return;
        };
        if let Some(runtime) = self.structured_agents.get_mut(node_id)
            && let Some(selection) = runtime.selection.as_mut()
        {
            selection.head = position;
            cx.notify();
        }
    }

    fn finish_structured_transcript_selection(
        &mut self,
        node_id: &CanvasNodeId,
        cx: &mut Context<Self>,
    ) {
        if let Some(runtime) = self.structured_agents.get_mut(node_id) {
            runtime.dragging_selection = false;
            if runtime
                .selection
                .is_some_and(|selection| selection.anchor == selection.head)
            {
                runtime.selection = None;
            }
        }
        cx.notify();
    }

    fn copy_structured_agent_transcript(
        &mut self,
        node_id: &CanvasNodeId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(runtime) = self.structured_agents.get(node_id) else {
            return false;
        };
        let (text, copied_selection) = if let Some(selection) = runtime.selection {
            let Some(text) = structured_transcript_selected_text(&runtime.transcript, selection)
            else {
                return false;
            };
            (text, true)
        } else {
            let transcript = runtime.transcript.trim();
            if transcript.is_empty() {
                return false;
            }
            (transcript.to_string(), false)
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status_message = if copied_selection {
            "Agent selection copied.".to_string()
        } else {
            "Agent output copied.".to_string()
        };
        self.error_message.clear();
        cx.notify();
        true
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
        let result = self.start_structured_agent_handle(&definition, None);
        match result {
            Ok(handle) => {
                let transcript_focus = cx.focus_handle().tab_stop(true);
                self.structured_agents.insert(
                    node_id,
                    StructuredAgentRuntime::new(handle, None, transcript_focus),
                );
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
        let orchestration_workspace_closed = self.orchestration_workspace_id.is_some_and(|id| {
            self.workspace(id)
                .is_some_and(|workspace| workspace.canvas.node(&node_id).is_some())
        });
        self.structured_agents.remove(&node_id);
        for workspace in &mut self.workspaces {
            workspace.canvas.remove_node(&node_id);
        }
        if orchestration_workspace_closed {
            self.orchestration_workspace_id = None;
        }
        self.persist_runtime_state();
        self.status_message = if orchestration_workspace_closed {
            "Structured agent closed and its dependency run stopped. Its worktree was kept."
                .to_string()
        } else {
            "Structured agent closed. Its worktree was kept.".to_string()
        };
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

    pub(super) fn ensure_same_execution_host(
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
                self.status_message = if self.stop_active_workspace_orchestration() {
                    "Dependency created. Dependency scheduling stopped; active agent turns continue."
                        .to_string()
                } else {
                    "Dependency created.".to_string()
                };
                self.error_message.clear();
                self.persist_runtime_state();
            }
            Err(error) => self.error_message = error.to_string(),
        }
        cx.notify();
    }

    fn cancel_canvas_dependency_link(&mut self, cx: &mut Context<Self>) {
        self.pending_dependency_source = None;
        self.status_message = "Dependency creation cancelled.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn stop_active_workspace_orchestration(&mut self) -> bool {
        let Some(workspace_id) = self.active_workspace_id else {
            return false;
        };
        if self.orchestration_workspace_id != Some(workspace_id) {
            return false;
        }
        self.orchestration_workspace_id = None;
        true
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
        let Some(next_state) = self
            .structured_agents
            .get(&node_id)
            .and_then(|runtime| agent_state_after_queue(runtime.state))
        else {
            self.error_message =
                "Wait for the active turn to finish or restart the disconnected agent before queuing a task."
                    .to_string();
            cx.notify();
            return;
        };
        if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
            runtime.queued_prompt = Some(prompt);
            runtime.state = next_state;
            runtime.diagnostic = None;
            runtime.approval = None;
            Self::set_input_value(&self.shell_inputs.structured_agent_prompt, "", window, cx);
            self.status_message = "Task queued for dependency orchestration.".to_string();
            self.error_message.clear();
        }
        cx.notify();
    }

    fn start_dependency_orchestration(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            self.error_message = "Open a Canvas workspace before running dependencies.".to_string();
            cx.notify();
            return;
        };
        if self
            .orchestration_workspace_id
            .is_some_and(|running_workspace_id| running_workspace_id != workspace_id)
        {
            self.error_message =
                "Another workspace already has an active dependency run.".to_string();
            cx.notify();
            return;
        }
        let dependency_endpoints: Vec<_> = self
            .workspace(workspace_id)
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
            self.orchestration_workspace_id = None;
            cx.notify();
            return;
        }
        self.orchestration_workspace_id = Some(workspace_id);
        if !self.dispatch_ready_agent_tasks(workspace_id)
            && self.orchestration_workspace_id == Some(workspace_id)
        {
            self.status_message =
                "No queued task is ready. Check dependency states and queued prompts.".to_string();
        }
        cx.notify();
    }

    fn dispatch_ready_agent_tasks(&mut self, workspace_id: u64) -> bool {
        let Some(workspace) = self.workspace(workspace_id) else {
            self.orchestration_workspace_id = None;
            return false;
        };
        let (node_ids, edges) = canvas_orchestration_scope(&workspace.canvas);
        let mut dispatched = false;
        loop {
            let agents: Vec<_> = self
                .structured_agents
                .iter()
                .filter(|(node_id, _)| node_ids.contains(*node_id))
                .map(|(node_id, runtime)| SchedulableAgent {
                    node_id: node_id.clone(),
                    state: runtime.state,
                    has_queued_task: runtime.queued_prompt.is_some(),
                })
                .collect();
            let schedule = schedule_dependency_dag(&agents, &edges, 2);
            if schedule.cycle_detected {
                self.error_message =
                    "Dependency graph contains a cycle and cannot run.".to_string();
                self.orchestration_workspace_id = None;
                return false;
            }
            for node_id in schedule.blocked {
                if let Some(runtime) = self.structured_agents.get_mut(&node_id) {
                    runtime.state = AgentRunState::Blocked;
                    runtime.diagnostic =
                        Some("A prerequisite failed or was cancelled.".to_string());
                }
            }
            let mut send_failed = false;
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
                            runtime.push_context_message(AgentRole::User, prompt.trim());
                            dispatched = true;
                        }
                        Err(error) => {
                            runtime.state = AgentRunState::Failed;
                            runtime.diagnostic = Some(error.to_string());
                            send_failed = true;
                        }
                    }
                }
            }
            if !send_failed {
                break;
            }
        }
        let pending = self.structured_agents.iter().any(|(node_id, runtime)| {
            node_ids.contains(node_id)
                && runtime.queued_prompt.is_some()
                && runtime.state != AgentRunState::Blocked
        });
        let blocked_count = self
            .structured_agents
            .iter()
            .filter(|(node_id, runtime)| {
                node_ids.contains(*node_id)
                    && runtime.queued_prompt.is_some()
                    && runtime.state == AgentRunState::Blocked
            })
            .count();
        let running = self.structured_agents.iter().any(|(node_id, runtime)| {
            node_ids.contains(node_id)
                && matches!(
                    runtime.state,
                    AgentRunState::Starting | AgentRunState::Running
                )
        });
        if !pending && !running {
            self.orchestration_workspace_id = None;
            self.status_message = if blocked_count == 0 {
                "Dependency run finished.".to_string()
            } else {
                format!("Dependency run finished with {blocked_count} blocked task(s).")
            };
        } else if dispatched {
            self.status_message = "Dependency run started ready tasks.".to_string();
        }
        dispatched
    }

    pub(super) fn open_context_review_for_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let policy = edge.context_policy.clone().unwrap_or_default();
        let preview = if let Some(runtime) = self.structured_agents.get(&source_node.id) {
            build_agent_context_handoff(
                &source_label,
                &runtime.context_messages,
                &policy,
                current_unix_millis(),
            )
        } else if let Some(pane) = source_node
            .kind
            .pane_id()
            .and_then(|pane_id| self.pane(pane_id))
        {
            build_context_handoff(
                &source_label,
                &pane.terminal.all_rows_text().join("\n"),
                &policy,
                current_unix_millis(),
            )
        } else {
            build_context_handoff(&source_label, "", &policy, current_unix_millis())
        };
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
        self.canvas_links_open = false;
        self.worktree_manager_open = false;
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn send_context_handoff(&mut self, cx: &mut Context<Self>) {
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
        if self.canvas_add_menu_open {
            self.canvas_links_open = false;
            self.canvas_node_menu_id = None;
            self.worktree_manager_open = false;
            self.context_handoff_review = None;
            self.pending_tmux_close = None;
            self.pending_canvas_pane_close = None;
            self.split_pane_chooser = None;
        }
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
            .max_w(relative(0.9))
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
                                .debug_selector(move || format!("canvas-add-agent-{index}"))
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
            AgentExecutableStatus::Unusable {
                path,
                error,
                guidance,
            } => format!(
                "Found {}, but its version check failed: {error}. {guidance}",
                path.display()
            ),
        };
        let status_text = if matches!(&location, AgentLocation::Local) {
            local_status
        } else {
            "The executable is checked on the remote host when the SSH session opens.".to_string()
        };
        let can_launch = agent_creation_can_launch(&location, &state.executable_status);
        let profiles = self.saved.profiles.clone();

        v_flex()
            .id("agent-creation-panel")
            .absolute()
            .top(px(10.0))
            .left(px(12.0))
            .w(px(520.0))
            .max_w(relative(0.9))
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
                                            .debug_selector(|| {
                                                "agent-backend-interactive".to_string()
                                            })
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
                                            .debug_selector(|| {
                                                "agent-backend-structured".to_string()
                                            })
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
                                .debug_selector(|| "agent-launch".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .icon(IconName::ArrowRight)
                                .label("Launch Agent")
                                .disabled(!can_launch)
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
            self.canvas_links_open = false;
            self.canvas_node_menu_id = None;
            self.agent_creation = None;
            self.context_handoff_review = None;
            self.pending_tmux_close = None;
            self.pending_canvas_pane_close = None;
            self.split_pane_chooser = None;
        }
        cx.notify();
    }

    fn toggle_canvas_links(&mut self, cx: &mut Context<Self>) {
        self.canvas_links_open = !self.canvas_links_open;
        if self.canvas_links_open {
            self.canvas_add_menu_open = false;
            self.canvas_node_menu_id = None;
            self.agent_creation = None;
            self.context_handoff_review = None;
            self.worktree_manager_open = false;
            self.pending_tmux_close = None;
            self.pending_canvas_pane_close = None;
            self.split_pane_chooser = None;
        }
        cx.notify();
    }

    pub(super) fn toggle_canvas_node_menu(
        &mut self,
        node_id: CanvasNodeId,
        cx: &mut Context<Self>,
    ) {
        if self.canvas_node_menu_id.as_ref() == Some(&node_id) {
            self.canvas_node_menu_id = None;
        } else {
            self.canvas_node_menu_id = Some(node_id);
            self.canvas_add_menu_open = false;
            self.canvas_links_open = false;
            self.worktree_manager_open = false;
            self.agent_creation = None;
            self.context_handoff_review = None;
            self.pending_tmux_close = None;
            self.pending_canvas_pane_close = None;
            self.split_pane_chooser = None;
        }
        cx.notify();
    }

    fn canvas_node_label(&self, node_id: &CanvasNodeId) -> String {
        self.active_workspace()
            .and_then(|workspace| workspace.canvas.node(node_id))
            .map(|node| {
                node.title
                    .clone()
                    .or_else(|| {
                        node.kind
                            .pane_id()
                            .and_then(|pane_id| self.pane(pane_id))
                            .map(|pane| pane.title.clone())
                    })
                    .unwrap_or_else(|| match &node.kind {
                        CanvasNodeKind::Terminal { .. } => "Terminal".to_string(),
                        CanvasNodeKind::Agent { definition, .. } => {
                            definition.provider.label().to_string()
                        }
                    })
            })
            .unwrap_or_else(|| "Unknown node".to_string())
    }

    fn set_canvas_edge_enabled(
        &mut self,
        edge_id: CanvasEdgeId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let dependency_changed = self.active_workspace().is_some_and(|workspace| {
            workspace
                .canvas
                .edges
                .iter()
                .any(|edge| edge.id == edge_id && edge.kind == CanvasEdgeKind::Dependency)
        });
        let changed = self
            .active_workspace_mut()
            .is_some_and(|workspace| workspace.canvas.set_edge_enabled(&edge_id, enabled));
        if !changed {
            self.error_message = "That canvas link no longer exists.".to_string();
            cx.notify();
            return;
        }
        if !enabled
            && self
                .context_handoff_review
                .as_ref()
                .is_some_and(|review| review.edge_id == edge_id)
        {
            self.context_handoff_review = None;
        }
        self.persist_runtime_state();
        let scheduling_stopped = dependency_changed && self.stop_active_workspace_orchestration();
        self.status_message = if scheduling_stopped {
            "Dependency changed. Dependency scheduling stopped; active agent turns continue."
        } else if enabled {
            "Canvas link enabled."
        } else {
            "Canvas link disabled."
        }
        .to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn remove_canvas_edge(&mut self, edge_id: CanvasEdgeId, cx: &mut Context<Self>) {
        let dependency_removed = self.active_workspace().is_some_and(|workspace| {
            workspace
                .canvas
                .edges
                .iter()
                .any(|edge| edge.id == edge_id && edge.kind == CanvasEdgeKind::Dependency)
        });
        let removed = self
            .active_workspace_mut()
            .is_some_and(|workspace| workspace.canvas.remove_edge(&edge_id));
        if !removed {
            self.error_message = "That canvas link no longer exists.".to_string();
            cx.notify();
            return;
        }
        if self
            .context_handoff_review
            .as_ref()
            .is_some_and(|review| review.edge_id == edge_id)
        {
            self.context_handoff_review = None;
        }
        self.persist_runtime_state();
        self.status_message = if dependency_removed && self.stop_active_workspace_orchestration() {
            "Dependency deleted and scheduling stopped; active agent turns continue.".to_string()
        } else {
            "Canvas link deleted; nodes and sessions were kept.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn render_canvas_node_menu(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(node_id) = self.canvas_node_menu_id.clone() else {
            return div().into_any_element();
        };
        let Some(node) = self.active_workspace().and_then(|workspace| {
            workspace
                .canvas
                .node(&node_id)
                .map(|node| (workspace, node))
        }) else {
            return div().into_any_element();
        };
        let screen = canvas_node_render_rect(node.0.canvas.transform, node.1);
        let viewport_width = f32::from(window.viewport_size().width);
        let menu_width = 300.0;
        let menu_gap = 8.0;
        let menu_x = if screen.x + screen.width + menu_gap + menu_width <= viewport_width - 12.0 {
            screen.x + screen.width + menu_gap
        } else {
            (screen.x - menu_width - menu_gap).max(12.0)
        };
        let menu_y = screen.y.max(12.0);
        let label = self.canvas_node_label(&node_id);
        let rename_id = node_id.clone();
        let dependency_id = node_id.clone();
        let review_id = node_id.clone();

        v_flex()
            .id("canvas-node-menu")
            .absolute()
            .top(px(menu_y))
            .left(px(menu_x))
            .w(px(300.0))
            .max_w(relative(0.9))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border_dark())
            .bg(theme::library_card())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
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
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.0))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(label),
                    )
                    .child(
                        Button::new("canvas-node-menu-close")
                            .debug_selector(|| "canvas-node-menu-close".to_string())
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close menu")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.canvas_node_menu_id = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .child(
                        Button::new("canvas-node-menu-rename")
                            .small()
                            .ghost()
                            .icon(IconName::ALargeSmall)
                            .label("Rename")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.canvas_node_menu_id = None;
                                this.start_canvas_node_rename(rename_id.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-node-menu-dependency")
                            .debug_selector(|| "canvas-node-menu-dependency".to_string())
                            .small()
                            .ghost()
                            .icon(IconName::Building2)
                            .label("Create dependency link")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.canvas_node_menu_id = None;
                                this.link_canvas_dependency(dependency_id.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-node-menu-review-context")
                            .small()
                            .ghost()
                            .icon(IconName::Eye)
                            .label("Review incoming context")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.canvas_node_menu_id = None;
                                if let Some(workspace) = this.active_workspace_mut() {
                                    workspace.canvas.select_and_raise(&review_id);
                                }
                                this.open_context_review_for_selected(window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-node-menu-view-links")
                            .small()
                            .ghost()
                            .icon(IconName::Inspector)
                            .label("View workspace links")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.canvas_node_menu_id = None;
                                this.toggle_canvas_links(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_canvas_links(&self, cx: &mut Context<Self>) -> AnyElement {
        let edges = self
            .active_workspace()
            .map(|workspace| workspace.canvas.edges.clone())
            .unwrap_or_default();
        v_flex()
            .id("canvas-links-panel")
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(500.0))
            .max_w(relative(0.9))
            .max_h(px(620.0))
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
                            .child(format!("Canvas Links ({})", edges.len())),
                    )
                    .child(
                        Button::new("canvas-links-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close links")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.canvas_links_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .when(edges.is_empty(), |list| {
                        list.child(
                            div()
                                .py_4()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child("No context or dependency links in this workspace."),
                        )
                    })
                    .children(edges.into_iter().map(|edge| {
                        let source = self.canvas_node_label(&edge.source);
                        let target = self.canvas_node_label(&edge.target);
                        let toggle_id = edge.id.clone();
                        let delete_id = edge.id.clone();
                        let kind_label = match edge.kind {
                            CanvasEdgeKind::Context => "Context",
                            CanvasEdgeKind::Dependency => "Dependency",
                        };
                        h_flex()
                            .id(SharedString::from(format!(
                                "canvas-link-row-{}",
                                edge.id.as_str()
                            )))
                            .min_h(px(46.0))
                            .px_2()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(theme::with_alpha(theme::border_dark(), 0.5))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child(format!("{source} -> {target}")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(if edge.enabled {
                                                theme::success()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(format!(
                                                "{kind_label} / {}",
                                                if edge.enabled { "Enabled" } else { "Disabled" }
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "canvas-link-toggle-{}",
                                    edge.id.as_str()
                                )))
                                .xsmall()
                                .ghost()
                                .label(if edge.enabled { "Disable" } else { "Enable" })
                                .tooltip(if edge.enabled {
                                    "Disable this link"
                                } else {
                                    "Enable this link"
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_canvas_edge_enabled(
                                            toggle_id.clone(),
                                            !edge.enabled,
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "canvas-link-delete-{}",
                                    edge.id.as_str()
                                )))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Delete)
                                .tooltip("Delete link; keep both nodes")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove_canvas_edge(delete_id.clone(), cx);
                                    },
                                )),
                            )
                    })),
            )
            .into_any_element()
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
                let state = match (status.dirty, status.has_commits_after_base) {
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
                let path_summary = format!(
                    "{} changed path{}",
                    status.changed_paths,
                    if status.changed_paths == 1 { "" } else { "s" }
                );
                self.status_message = if status.diff_summary.is_empty() {
                    format!("{state} {path_summary}.")
                } else {
                    format!("{state} {path_summary}; {}.", status.diff_summary)
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

    pub(super) fn set_managed_worktree_disposition(
        &mut self,
        path: &str,
        disposition: SavedManagedWorktreeDisposition,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .saved
            .managed_agent_worktrees
            .iter_mut()
            .find(|worktree| worktree.path == path)
        else {
            self.error_message = "That managed worktree is no longer registered.".to_string();
            cx.notify();
            return;
        };
        worktree.disposition = disposition;
        for workspace in &mut self.workspaces {
            for node in &mut workspace.canvas.nodes {
                let CanvasNodeKind::Agent { definition, .. } = &mut node.kind else {
                    continue;
                };
                if let Some(managed) = definition
                    .managed_worktree
                    .as_mut()
                    .filter(|managed| managed.path == path)
                {
                    managed.disposition = disposition;
                }
            }
        }
        self.persist_runtime_state();
        self.status_message = match disposition {
            SavedManagedWorktreeDisposition::Active => "Worktree marked active.".to_string(),
            SavedManagedWorktreeDisposition::Complete => {
                "Task marked complete. The worktree and branch were kept.".to_string()
            }
            SavedManagedWorktreeDisposition::Kept => {
                "Worktree and branch marked to keep. Nothing was deleted.".to_string()
            }
        };
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
            .max_w(relative(0.9))
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
                        let complete_path = worktree.path.clone();
                        let keep_path = worktree.path.clone();
                        let remove_path = worktree.path.clone();
                        let disposition = worktree.disposition;
                        let disposition_label = match disposition {
                            SavedManagedWorktreeDisposition::Active => "Active",
                            SavedManagedWorktreeDisposition::Complete => "Complete",
                            SavedManagedWorktreeDisposition::Kept => "Kept",
                        };
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
                                            .child(format!(
                                                "{} [{}]",
                                                worktree.branch, disposition_label
                                            )),
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
                                Button::new(("worktree-complete", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Check)
                                    .tooltip("Mark task complete and keep its worktree")
                                    .disabled(matches!(
                                        disposition,
                                        SavedManagedWorktreeDisposition::Complete
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_managed_worktree_disposition(
                                            &complete_path,
                                            SavedManagedWorktreeDisposition::Complete,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("worktree-keep", index))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::GitHub)
                                    .tooltip("Keep worktree and branch")
                                    .disabled(matches!(
                                        disposition,
                                        SavedManagedWorktreeDisposition::Kept
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_managed_worktree_disposition(
                                            &keep_path,
                                            SavedManagedWorktreeDisposition::Kept,
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
        let can_review_context = self.active_workspace().is_some_and(|workspace| {
            workspace
                .canvas
                .selected_node_id
                .as_ref()
                .is_some_and(|target| {
                    workspace.canvas.edges.iter().any(|edge| {
                        edge.enabled
                            && edge.kind == CanvasEdgeKind::Context
                            && &edge.target == target
                    })
                })
        });
        let can_run_workflow = self.active_workspace().is_some_and(|workspace| {
            workspace.canvas.nodes.iter().any(|node| {
                self.structured_agents
                    .get(&node.id)
                    .is_some_and(|runtime| runtime.queued_prompt.is_some())
            })
        });
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
                            .debug_selector(|| "canvas-add-terminal".to_string())
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
                            .debug_selector(|| "canvas-review-context".to_string())
                            .small()
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .label("Review context")
                            .tooltip("Review an incoming context link before sending")
                            .disabled(!can_review_context)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_context_review_for_selected(window, cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-run-workflow")
                            .debug_selector(|| "canvas-run-workflow".to_string())
                            .small()
                            .ghost()
                            .icon(IconName::Building2)
                            .label("Run workflow")
                            .tooltip("Run queued tasks when dependencies are satisfied")
                            .disabled(!can_run_workflow)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_dependency_orchestration(cx);
                            })),
                    )
                    .child(
                        Button::new("canvas-links")
                            .small()
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .label(format!(
                                "Links ({})",
                                self.active_workspace()
                                    .map(|workspace| workspace.canvas.edges.len())
                                    .unwrap_or_default()
                            ))
                            .tooltip("Inspect, enable, disable, or delete canvas links")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_canvas_links(cx);
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

    pub(super) fn canvas_node_location_label(&self, node: &CanvasNode) -> String {
        match &node.kind {
            CanvasNodeKind::Terminal { pane_id } => self
                .pane(*pane_id)
                .map(|pane| {
                    if pane.request.is_local_shell() {
                        "Local".to_string()
                    } else {
                        pane.request.host.clone()
                    }
                })
                .unwrap_or_else(|| "Unavailable".to_string()),
            CanvasNodeKind::Agent { definition, .. } => match &definition.location {
                AgentLocation::Local => "Local".to_string(),
                AgentLocation::SavedHost { profile_id } => self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .map(HostProfile::display_name)
                    .unwrap_or_else(|| "Saved host unavailable".to_string()),
            },
        }
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
        let screen = canvas_node_render_rect(workspace.canvas.transform, node);
        let selected = workspace.canvas.selected_node_id.as_ref() == Some(&node.id);
        let dependency_source = self.pending_dependency_source.as_ref() == Some(&node.id);
        let node_id = node.id.clone();
        let pane_id = node.kind.pane_id();
        let title = node
            .title
            .clone()
            .or_else(|| pane_id.and_then(|id| self.pane(id).map(|pane| pane.title.clone())))
            .unwrap_or_else(|| "Agent".to_string());
        let location = self.canvas_node_location_label(node);
        let (status, needs_attention) = self
            .structured_agents
            .get(&node.id)
            .map(|runtime| {
                (
                    runtime.state.label().to_string(),
                    agent_state_needs_attention(runtime.state),
                )
            })
            .or_else(|| {
                pane_id.and_then(|id| self.pane(id)).map(|pane| {
                    (
                        pane.status.clone(),
                        pane.status == "Error"
                            || (!pane.connected && pane.closed && !pane.user_closed),
                    )
                })
            })
            .unwrap_or_else(|| ("Idle".to_string(), false));
        let subtitle = format!("{location} / {status}");
        let header_node_id = node_id.clone();
        let link_node_id = node_id.clone();
        let more_node_id = node_id.clone();
        let more_selector = format!("canvas-node-more-{}", more_node_id.as_str());
        let collapse_node_id = node_id.clone();
        let resize_node_id = node_id.clone();
        let resize_selector = format!("canvas-node-resize-{}", resize_node_id.as_str());
        let rename_node_id = node_id.clone();
        let activate_node_id = node_id.clone();
        let renaming = self.canvas_node_rename_id.as_ref() == Some(&node_id);
        let close_pane_id = pane_id;
        let close_structured_node_id = (pane_id.is_none()).then_some(node_id.clone());
        let structured_output_state = self.structured_agents.get(&node_id).map(|runtime| {
            (
                !runtime.transcript.trim().is_empty(),
                runtime.follow_transcript,
                runtime.selection.is_some(),
            )
        });
        let header_latest_id = node_id.clone();
        let header_copy_id = node_id.clone();

        let mut body = v_flex()
            .id(SharedString::from(format!(
                "canvas-node-{}",
                node_id.as_str()
            )))
            .absolute()
            .left(px(screen.x))
            .top(px(screen.y))
            .w(px(screen.width))
            .h(px(screen.height))
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(if dependency_source {
                theme::warning()
            } else if selected {
                theme::focus_ring()
            } else {
                theme::border()
            })
            .bg(theme::terminal_panel())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.activate_canvas_node(workspace_id, activate_node_id.clone(), window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
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
                    .bg(theme::terminal_panel())
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .cursor(CursorStyle::OpenHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    if renaming {
                                        return;
                                    }
                                    if event.click_count >= 2 {
                                        this.start_canvas_node_rename(
                                            rename_node_id.clone(),
                                            window,
                                            cx,
                                        );
                                        return;
                                    }
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
                                Icon::new(match node.kind {
                                    CanvasNodeKind::Terminal { .. } => IconName::SquareTerminal,
                                    CanvasNodeKind::Agent { .. } => IconName::Bot,
                                })
                                .size(px(14.0))
                                .text_color(theme::accent()),
                            )
                            .when(renaming, |header| {
                                header.child(
                                    div()
                                        .w(px(180.0))
                                        .child(Input::new(&self.canvas_node_rename_input).small()),
                                )
                            })
                            .when(!renaming, |header| {
                                header.child(
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
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted_dark())
                                    .child(subtitle),
                            )
                            .when(needs_attention, |header| {
                                header.child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(12.0))
                                        .text_color(theme::warning()),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .when_some(
                                structured_output_state,
                                |actions, (has_output, follow_transcript, has_selection)| {
                                    actions
                                        .when(!follow_transcript, |actions| {
                                            actions.child(
                                                Button::new(SharedString::from(format!(
                                                    "structured-latest-{}",
                                                    header_latest_id.as_str()
                                                )))
                                                .debug_selector({
                                                    let node_id = header_latest_id.clone();
                                                    move || {
                                                        format!(
                                                            "structured-latest-{}",
                                                            node_id.as_str()
                                                        )
                                                    }
                                                })
                                                .xsmall()
                                                .ghost()
                                                .icon(IconName::ArrowDown)
                                                .tooltip("Jump to latest output")
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.resume_structured_transcript_follow(
                                                        &header_latest_id,
                                                        cx,
                                                    );
                                                })),
                                            )
                                        })
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "structured-copy-{}",
                                                header_copy_id.as_str()
                                            )))
                                            .debug_selector({
                                                let node_id = header_copy_id.clone();
                                                move || {
                                                    format!("structured-copy-{}", node_id.as_str())
                                                }
                                            })
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::Copy)
                                            .tooltip(if has_selection {
                                                "Copy selected agent output"
                                            } else {
                                                "Copy all agent output"
                                            })
                                            .disabled(!has_output)
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.copy_structured_agent_transcript(
                                                    &header_copy_id,
                                                    cx,
                                                );
                                            })),
                                        )
                                },
                            )
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
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_canvas_node_collapsed(
                                            collapse_node_id.clone(),
                                            cx,
                                        );
                                    },
                                )),
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
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.link_canvas_node(link_node_id.clone(), cx);
                                    },
                                )),
                            )
                            .child(
                                Button::new(SharedString::from(more_selector.clone()))
                                    .debug_selector(move || more_selector.clone())
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .tooltip("More node actions")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_canvas_node_menu(more_node_id.clone(), cx);
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
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_canvas_pane_close(pane_id, cx);
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
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
                    .debug_selector(move || resize_selector.clone())
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

    fn render_structured_transcript_group(
        &self,
        text: String,
        selected: bool,
        font_family: SharedString,
    ) -> AnyElement {
        div()
            .whitespace_nowrap()
            .font_family(font_family)
            .text_size(px(STRUCTURED_TRANSCRIPT_FONT_SIZE))
            .line_height(px(STRUCTURED_TRANSCRIPT_LINE_HEIGHT))
            .text_color(if selected {
                theme::terminal_selection_fg()
            } else {
                theme::text_on_dark()
            })
            .when(selected, |group| group.bg(theme::terminal_selection_bg()))
            .child(display_terminal_text(&text))
            .into_any_element()
    }

    fn render_structured_transcript_row(
        &self,
        row_index: usize,
        line: &str,
        selection: Option<SelectionRange>,
        font_family: SharedString,
    ) -> AnyElement {
        let mut groups = Vec::new();
        let mut pending_text = String::new();
        let mut pending_selected = None;
        for (column, character) in line.chars().enumerate() {
            let selected = selection_contains(selection, row_index, column);
            match pending_selected {
                Some(current) if current == selected => pending_text.push(character),
                Some(current) => {
                    groups.push(self.render_structured_transcript_group(
                        std::mem::take(&mut pending_text),
                        current,
                        font_family.clone(),
                    ));
                    pending_text.push(character);
                    pending_selected = Some(selected);
                }
                None => {
                    pending_text.push(character);
                    pending_selected = Some(selected);
                }
            }
        }
        if let Some(selected) = pending_selected {
            groups.push(self.render_structured_transcript_group(
                pending_text,
                selected,
                font_family,
            ));
        } else {
            groups.push(self.render_structured_transcript_group(
                " ".to_string(),
                false,
                font_family,
            ));
        }

        h_flex()
            .flex_none()
            .h(px(STRUCTURED_TRANSCRIPT_LINE_HEIGHT))
            .w_full()
            .whitespace_nowrap()
            .children(groups)
            .into_any_element()
    }

    fn render_structured_agent_body(
        &self,
        node_id: CanvasNodeId,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(runtime) = self.structured_agents.get(&node_id) else {
            let restart_id = node_id.clone();
            let restart_selector = format!("structured-restart-{}", node_id.as_str());
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
                    .debug_selector(move || restart_selector.clone())
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
        let transcript_scroll = runtime.transcript_scroll.clone();
        let transcript_focus = runtime.transcript_focus.clone();
        let follow_transcript = runtime.follow_transcript;
        let selection = runtime.selection;
        if follow_transcript {
            transcript_scroll.scroll_to_bottom();
        }
        let send_id = node_id.clone();
        let cancel_id = node_id.clone();
        let queue_id = node_id.clone();
        let scroll_id = node_id.clone();
        let selection_start_id = node_id.clone();
        let selection_move_id = node_id.clone();
        let selection_end_id = node_id.clone();
        let selection_out_id = node_id.clone();
        let selection_copy_id = node_id.clone();
        let transcript_selector_id = node_id.clone();
        let font_family = self.terminal_font_family(cx);
        let transcript_lines = structured_transcript_lines(&transcript);
        let last_line_index = transcript_lines.len().saturating_sub(1);
        let mut transcript_rows = Vec::with_capacity(transcript_lines.len());
        for (line_index, line) in transcript_lines.into_iter().enumerate() {
            let row_selector_id = node_id.clone();
            transcript_rows.push(
                div()
                    .id(SharedString::from(format!(
                        "structured-transcript-row-{}-{line_index}",
                        node_id.as_str()
                    )))
                    .when(line_index == last_line_index, |row| {
                        row.debug_selector(move || {
                            format!(
                                "structured-transcript-last-row-{}",
                                row_selector_id.as_str()
                            )
                        })
                    })
                    .flex_none()
                    .w_full()
                    .h(px(STRUCTURED_TRANSCRIPT_LINE_HEIGHT))
                    .child(self.render_structured_transcript_row(
                        line_index,
                        line,
                        selection,
                        font_family.clone(),
                    ))
                    .into_any_element(),
            );
        }
        let transcript_view = v_flex().w_full().children(transcript_rows);

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "structured-transcript-{}",
                        node_id.as_str()
                    )))
                    .debug_selector(move || {
                        format!("structured-transcript-{}", transcript_selector_id.as_str())
                    })
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .p_3()
                    .overflow_scroll()
                    .cursor(CursorStyle::IBeam)
                    .track_scroll(&transcript_scroll)
                    .track_focus(&transcript_focus)
                    .focusable()
                    .on_scroll_wheel(cx.listener(move |this, _: &ScrollWheelEvent, _, cx| {
                        this.pause_structured_transcript_follow(&scroll_id, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.start_structured_transcript_selection(
                                &selection_start_id,
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            this.update_structured_transcript_selection(
                                &selection_move_id,
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            this.finish_structured_transcript_selection(&selection_end_id, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            this.finish_structured_transcript_selection(&selection_out_id, cx);
                        }),
                    )
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.modifiers.secondary()
                            && event.keystroke.key.as_str() == "c"
                            && this.copy_structured_agent_transcript(&selection_copy_id, cx)
                        {
                            cx.stop_propagation();
                        }
                    }))
                    .child(transcript_view),
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
            .max_w(relative(0.9))
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

    fn render_tmux_close_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(pending) = self.pending_tmux_close.as_ref() else {
            return div().into_any_element();
        };
        let session_label = pending.session_name.as_deref().unwrap_or("unknown session");
        let can_kill = pending.session_name.is_some();
        v_flex()
            .id("canvas-tmux-close-dialog")
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(500.0))
            .max_w(relative(0.9))
            .rounded(px(7.0))
            .border_1()
            .border_color(if pending.confirm_kill {
                theme::danger()
            } else {
                theme::border_dark()
            })
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
                            .child(if pending.confirm_kill {
                                "Confirm tmux session deletion"
                            } else {
                                "Close persistent terminal"
                            }),
                    )
                    .child(
                        Button::new("canvas-tmux-close-cancel")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending_tmux_close = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(if pending.confirm_kill {
                                theme::danger()
                            } else {
                                theme::text_muted()
                            })
                            .child(if pending.confirm_kill {
                                format!(
                                    "This permanently ends tmux session {session_label} and every process running inside it."
                                )
                            } else {
                                format!(
                                    "Session {session_label} can keep running on the SSH host after TermiRust disconnects."
                                )
                            }),
                    )
                    .when(!pending.confirm_kill, |content| {
                        content
                            .child(
                                Button::new("canvas-tmux-detach-node")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Detach from Canvas")
                                    .tooltip("Close this node and leave tmux running")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.detach_tmux_node_from_canvas(cx);
                                    })),
                            )
                            .child(
                                Button::new("canvas-tmux-disconnect-client")
                                    .small()
                                    .ghost()
                                    .label("Disconnect Client")
                                    .tooltip("Keep the node so it can reconnect later")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.disconnect_tmux_client(cx);
                                    })),
                            )
                            .child(
                                Button::new("canvas-tmux-kill-request")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label("Kill tmux Session...")
                                    .tooltip("Permanently stop this tmux session")
                                    .disabled(!can_kill)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request_tmux_kill_confirmation(cx);
                                    })),
                            )
                    })
                    .when(pending.confirm_kill, |content| {
                        content.child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("canvas-tmux-kill-back")
                                        .small()
                                        .ghost()
                                        .label("Back")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(pending) = this.pending_tmux_close.as_mut() {
                                                pending.confirm_kill = false;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("canvas-tmux-kill-confirm")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Danger,
                                            cx,
                                        ))
                                        .label("Confirm Kill")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_tmux_session_kill(cx);
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_canvas_pane_close_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(pending) = self.pending_canvas_pane_close.as_ref() else {
            return div().into_any_element();
        };
        v_flex()
            .id("canvas-pane-close-dialog")
            .debug_selector(|| "canvas-pane-close-dialog".to_string())
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(460.0))
            .max_w(relative(0.9))
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::danger())
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
                            .child("Close active terminal?"),
                    )
                    .child(
                        Button::new("canvas-pane-close-cancel-icon")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending_canvas_pane_close = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "Closing {} ends its active local process or SSH connection.",
                                pending.title
                            )),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("canvas-pane-close-cancel")
                                    .debug_selector(|| "canvas-pane-close-cancel".to_string())
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_canvas_pane_close = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("canvas-pane-close-confirm")
                                    .debug_selector(|| "canvas-pane-close-confirm".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label("Close Terminal")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_canvas_pane_close(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_split_pane_chooser(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chooser) = self.split_pane_chooser.as_ref() else {
            return div().into_any_element();
        };
        let selected = chooser.selected_pane_ids.clone();
        let panes = self
            .workspace(chooser.workspace_id)
            .map(|workspace| workspace.pane_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pane_id| {
                self.pane(pane_id).map(|pane| {
                    (
                        pane_id,
                        pane.title.clone(),
                        if pane.request.is_local_shell() {
                            "Local".to_string()
                        } else {
                            pane.endpoint.clone()
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        v_flex()
            .id("canvas-split-pane-chooser")
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(540.0))
            .max_w(relative(0.9))
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
                        div()
                            .text_size(px(13.0))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Choose Sessions for Split"),
                    )
                    .child(
                        Button::new("canvas-split-pane-chooser-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Stay in Canvas")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.split_pane_chooser = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .min_h_0()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "Choose 1-{} sessions. Unselected sessions keep running and remain on the Canvas.",
                                super::MAX_SPLIT_PANES
                            )),
                    )
                    .child(
                        v_flex()
                            .max_h(px(500.0))
                            .overflow_y_scrollbar()
                            .children(panes.into_iter().map(|(pane_id, title, endpoint)| {
                                let is_selected = selected.contains(&pane_id);
                                h_flex()
                                    .id(("canvas-split-pane-choice", pane_id))
                                    .min_h(px(48.0))
                                    .px_2()
                                    .gap_2()
                                    .items_center()
                                    .cursor_pointer()
                                    .border_b_1()
                                    .border_color(theme::with_alpha(theme::border_dark(), 0.5))
                                    .bg(if is_selected {
                                        theme::with_alpha(theme::accent(), 0.12)
                                    } else {
                                        theme::library_card()
                                    })
                                    .child(
                                        Icon::new(if is_selected {
                                            IconName::Check
                                        } else {
                                            IconName::SquareTerminal
                                        })
                                        .size(px(14.0))
                                        .text_color(if is_selected {
                                            theme::accent()
                                        } else {
                                            theme::text_muted()
                                        }),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(theme::text_muted())
                                                    .child(endpoint),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_split_pane_choice(pane_id, cx);
                                    }))
                            })),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted())
                                    .child(format!(
                                        "{} of {} selected",
                                        selected.len(),
                                        super::MAX_SPLIT_PANES
                                    )),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("canvas-split-pane-chooser-cancel")
                                            .small()
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.split_pane_chooser = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("canvas-split-pane-chooser-confirm")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .icon(IconName::Check)
                                            .label("Open Split")
                                            .disabled(selected.is_empty())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.confirm_split_pane_choice(window, cx);
                                            })),
                                    ),
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
        let viewport = window.viewport_size();
        let viewport_width = f32::from(viewport.width);
        let viewport_height =
            (f32::from(viewport.height) - theme::CHROME_HEIGHT - CANVAS_TOOLBAR_HEIGHT).max(1.0);
        let mut node_indices: Vec<_> = (0..workspace.canvas.nodes.len()).collect();
        node_indices.sort_by_key(|index| workspace.canvas.nodes[*index].z_index);
        node_indices.retain(|index| {
            canvas_rect_is_visible(
                canvas_node_render_rect(
                    workspace.canvas.transform,
                    &workspace.canvas.nodes[*index],
                ),
                viewport_width,
                viewport_height,
                CANVAS_RENDER_OVERSCAN,
            )
        });

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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.canvas_add_menu_open = true;
                    this.canvas_links_open = false;
                    this.canvas_node_menu_id = None;
                    this.worktree_manager_open = false;
                    this.context_handoff_review = None;
                    this.pending_tmux_close = None;
                    this.pending_canvas_pane_close = None;
                    this.split_pane_chooser = None;
                    cx.notify();
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
        if self.canvas_node_menu_id.is_some() {
            body = body.child(self.render_canvas_node_menu(window, cx));
        }
        if let Some(source) = self.pending_dependency_source.as_ref() {
            let source_label = self.canvas_node_label(source);
            body = body.child(
                h_flex()
                    .id("canvas-dependency-target-prompt")
                    .absolute()
                    .top(px(12.0))
                    .left(px(12.0))
                    .max_w(relative(0.8))
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme::warning())
                    .bg(theme::library_card())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child("Choose the next agent"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted())
                                    .child(format!(
                                        "{source_label} will run first. Click the agent that should run after it."
                                    )),
                            ),
                    )
                    .child(
                        Button::new("canvas-dependency-target-cancel")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Cancel dependency creation")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.cancel_canvas_dependency_link(cx);
                            })),
                    ),
            );
        }
        if self.context_handoff_review.is_some() {
            body = body.child(self.render_context_handoff_review(cx));
        }
        if self.pending_tmux_close.is_some() {
            body = body.child(self.render_tmux_close_dialog(cx));
        }
        if self.pending_canvas_pane_close.is_some() {
            body = body.child(self.render_canvas_pane_close_dialog(cx));
        }
        if self.split_pane_chooser.is_some() {
            body = body.child(self.render_split_pane_chooser(cx));
        }
        if self.canvas_links_open {
            body = body.child(self.render_canvas_links(cx));
        }
        if self.worktree_manager_open {
            body = body.child(self.render_worktree_manager(cx));
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .bg(theme::terminal_bg())
            .child(self.render_canvas_toolbar(window, cx))
            .child(body)
    }
}

fn canvas_orchestration_scope(
    canvas: &CanvasWorkspaceState,
) -> (HashSet<CanvasNodeId>, Vec<SavedCanvasEdge>) {
    let node_ids = canvas
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let edges = canvas
        .edges
        .iter()
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
    (node_ids, edges)
}

fn agent_state_after_queue(state: AgentRunState) -> Option<AgentRunState> {
    match state {
        AgentRunState::Idle
        | AgentRunState::Succeeded
        | AgentRunState::Failed
        | AgentRunState::Cancelled
        | AgentRunState::Blocked => Some(AgentRunState::Idle),
        AgentRunState::Starting
        | AgentRunState::Running
        | AgentRunState::WaitingForApproval
        | AgentRunState::Disconnected => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentExecutableStatus, AgentRunState, CANVAS_DEFAULT_NODE_HEIGHT,
        CANVAS_DEFAULT_NODE_WIDTH, CanvasNode, CanvasNodeKind, CanvasPoint, CanvasRect,
        CanvasTransform, CanvasWorkspaceState, TermiRustApp, agent_creation_can_launch,
        agent_state_after_queue, agent_state_needs_attention, canvas_node_render_rect,
        canvas_orchestration_scope, canvas_rect_is_visible, canvas_reveal_delta,
        default_agent_backend, find_non_overlapping_position, fit_transform,
        structured_transcript_lines, structured_transcript_selected_text,
    };
    use crate::models::{
        AgentBackendKind, AgentLocation, AgentProvider, CanvasNodeId, SavedAgentDefinition,
        SavedCanvasState, SavedWorktreePolicy,
    };
    use crate::ui::keys::TerminalCellPos;
    use crate::ui::render_terminal::SelectionRange;

    #[test]
    fn supported_agents_default_to_workflow_mode() {
        for provider in [
            AgentProvider::Codex,
            AgentProvider::ClaudeCode,
            AgentProvider::Gemini,
        ] {
            assert_eq!(
                default_agent_backend(provider),
                AgentBackendKind::Structured
            );
        }
        assert_eq!(
            default_agent_backend(AgentProvider::CustomCli),
            AgentBackendKind::InteractivePty
        );
    }

    #[test]
    fn structured_transcript_preserves_lines_and_extracts_arbitrary_selection() {
        let transcript = "alpha beta\nsecond line\nthird\n";
        assert_eq!(
            structured_transcript_lines(transcript),
            vec!["alpha beta", "second line", "third"]
        );
        assert_eq!(
            structured_transcript_selected_text(
                transcript,
                SelectionRange {
                    anchor: TerminalCellPos { row: 0, col: 6 },
                    head: TerminalCellPos { row: 1, col: 5 },
                },
            )
            .as_deref(),
            Some("beta\nsecond")
        );
        assert_eq!(
            structured_transcript_selected_text(
                transcript,
                SelectionRange {
                    anchor: TerminalCellPos { row: 1, col: 5 },
                    head: TerminalCellPos { row: 0, col: 6 },
                },
            )
            .as_deref(),
            Some("beta\nsecond")
        );
    }

    #[test]
    fn local_agent_launch_requires_an_available_executable() {
        let available = AgentExecutableStatus::Available {
            path: "/tmp/codex".into(),
            version: None,
        };
        let missing = AgentExecutableStatus::Missing {
            requested: "codex".into(),
            guidance: "Install Codex.",
        };

        assert!(agent_creation_can_launch(&AgentLocation::Local, &available));
        assert!(!agent_creation_can_launch(&AgentLocation::Local, &missing));
        assert!(agent_creation_can_launch(
            &AgentLocation::SavedHost {
                profile_id: "remote".to_string(),
            },
            &missing,
        ));
    }

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
    fn render_visibility_culls_distant_nodes_with_conservative_overscan() {
        let transform = CanvasTransform::default();
        let visible = terminal_node("visible", 1, 950.0, 650.0);
        let overscan = terminal_node("overscan", 2, 1050.0, 750.0);
        let distant = terminal_node("distant", 3, 1400.0, 1100.0);

        assert!(canvas_rect_is_visible(
            canvas_node_render_rect(transform, &visible),
            1000.0,
            700.0,
            0.0,
        ));
        assert!(canvas_rect_is_visible(
            canvas_node_render_rect(transform, &overscan),
            1000.0,
            700.0,
            96.0,
        ));
        assert!(!canvas_rect_is_visible(
            canvas_node_render_rect(transform, &distant),
            1000.0,
            700.0,
            96.0,
        ));
    }

    #[test]
    fn low_zoom_visibility_uses_the_same_minimum_bounds_as_rendering() {
        let mut node = terminal_node("low-zoom", 1, -520.0, 0.0);
        node.rect.width = 300.0;
        let rendered = canvas_node_render_rect(
            CanvasTransform {
                zoom: 0.35,
                ..CanvasTransform::default()
            },
            &node,
        );

        assert_eq!(rendered.width, 180.0);
        assert!(canvas_rect_is_visible(rendered, 1000.0, 700.0, 8.0));
    }

    #[test]
    fn keyboard_reveal_moves_only_the_axes_outside_the_viewport() {
        assert_eq!(
            canvas_reveal_delta(
                CanvasRect {
                    x: 120.0,
                    y: 650.0,
                    width: 300.0,
                    height: 200.0,
                },
                1000.0,
                700.0,
                24.0,
            ),
            CanvasPoint::new(0.0, -174.0)
        );
        assert_eq!(
            canvas_reveal_delta(
                CanvasRect {
                    x: -100.0,
                    y: 50.0,
                    width: 1200.0,
                    height: 200.0,
                },
                1000.0,
                700.0,
                24.0,
            ),
            CanvasPoint::new(0.0, 0.0)
        );
    }

    #[test]
    fn only_actionable_agent_states_request_header_attention() {
        for state in [
            AgentRunState::WaitingForApproval,
            AgentRunState::Blocked,
            AgentRunState::Failed,
            AgentRunState::Disconnected,
        ] {
            assert!(agent_state_needs_attention(state));
        }
        for state in [
            AgentRunState::Idle,
            AgentRunState::Starting,
            AgentRunState::Running,
            AgentRunState::Succeeded,
            AgentRunState::Cancelled,
        ] {
            assert!(!agent_state_needs_attention(state));
        }
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
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![
                terminal_node("source", 1, 0.0, 0.0),
                terminal_node("target", 2, 800.0, 0.0),
            ],
            ..CanvasWorkspaceState::default()
        };
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
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![
                terminal_node("a", 1, 0.0, 0.0),
                terminal_node("b", 2, 800.0, 0.0),
                terminal_node("c", 3, 1600.0, 0.0),
            ],
            ..CanvasWorkspaceState::default()
        };
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
    fn orchestration_scope_contains_only_the_workspace_nodes_and_edges() {
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![
                terminal_node("workspace-a", 1, 0.0, 0.0),
                terminal_node("workspace-b", 2, 800.0, 0.0),
            ],
            ..CanvasWorkspaceState::default()
        };
        canvas
            .add_dependency_edge(
                CanvasNodeId::new("workspace-a"),
                CanvasNodeId::new("workspace-b"),
            )
            .unwrap();

        let (node_ids, edges) = canvas_orchestration_scope(&canvas);

        assert_eq!(node_ids.len(), 2);
        assert!(node_ids.contains(&CanvasNodeId::new("workspace-a")));
        assert!(node_ids.contains(&CanvasNodeId::new("workspace-b")));
        assert!(!node_ids.contains(&CanvasNodeId::new("other-workspace")));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, CanvasNodeId::new("workspace-a"));
        assert_eq!(edges[0].target, CanvasNodeId::new("workspace-b"));
    }

    #[test]
    fn queuing_a_completed_task_resets_it_without_reviving_disconnected_agents() {
        for state in [
            AgentRunState::Succeeded,
            AgentRunState::Failed,
            AgentRunState::Cancelled,
            AgentRunState::Blocked,
        ] {
            assert_eq!(agent_state_after_queue(state), Some(AgentRunState::Idle));
        }
        assert_eq!(agent_state_after_queue(AgentRunState::Disconnected), None);
        assert_eq!(agent_state_after_queue(AgentRunState::Running), None);
    }

    #[test]
    fn keyboard_selection_cycles_canvas_nodes_in_both_directions() {
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![
                terminal_node("a", 1, 0.0, 0.0),
                terminal_node("b", 2, 800.0, 0.0),
                terminal_node("c", 3, 1600.0, 0.0),
            ],
            ..CanvasWorkspaceState::default()
        };

        assert_eq!(canvas.select_adjacent_node(1), Some(CanvasNodeId::new("a")));
        assert_eq!(canvas.select_adjacent_node(1), Some(CanvasNodeId::new("b")));
        assert_eq!(
            canvas.select_adjacent_node(-1),
            Some(CanvasNodeId::new("a"))
        );
        assert_eq!(
            canvas.select_adjacent_node(-1),
            Some(CanvasNodeId::new("c"))
        );
    }

    #[test]
    fn node_titles_are_trimmed_and_can_return_to_the_default() {
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![terminal_node("a", 1, 0.0, 0.0)],
            ..CanvasWorkspaceState::default()
        };
        let node_id = CanvasNodeId::new("a");

        assert!(canvas.set_node_title(&node_id, "  API worker  "));
        assert_eq!(
            canvas.node(&node_id).and_then(|node| node.title.as_deref()),
            Some("API worker")
        );
        assert!(canvas.set_node_title(&node_id, "   "));
        assert_eq!(
            canvas.node(&node_id).and_then(|node| node.title.as_deref()),
            None
        );
        assert!(!canvas.set_node_title(&CanvasNodeId::new("missing"), "ignored"));
    }

    #[test]
    fn edge_controls_do_not_remove_connected_nodes() {
        let mut canvas = CanvasWorkspaceState {
            nodes: vec![
                terminal_node("a", 1, 0.0, 0.0),
                terminal_node("b", 2, 800.0, 0.0),
            ],
            ..CanvasWorkspaceState::default()
        };
        let edge_id = canvas
            .add_context_edge(CanvasNodeId::new("a"), CanvasNodeId::new("b"))
            .expect("edge should be created");

        assert!(canvas.set_edge_enabled(&edge_id, false));
        assert!(!canvas.edges[0].enabled);
        assert!(canvas.remove_edge(&edge_id));
        assert!(canvas.edges.is_empty());
        assert_eq!(canvas.nodes.len(), 2);
        assert!(!canvas.remove_edge(&edge_id));
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

    #[test]
    fn remote_agent_bootstrap_checks_version_and_working_directory_before_launch() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            working_directory: Some("/srv/project with space".to_string()),
            worktree: SavedWorktreePolicy::SharedDirectory,
            ..SavedAgentDefinition::default()
        };
        let script = TermiRustApp::remote_agent_startup_script(&definition, "review this")
            .expect("remote bootstrap should be generated");

        let executable_check = script.find("command -v 'codex'").unwrap();
        let version_check = script.find("'codex' '--version'").unwrap();
        let directory_check = script.find("[ ! -d '/srv/project with space' ]").unwrap();
        let directory_change = script.find("cd -- '/srv/project with space'").unwrap();
        let launch = script.rfind("exec 'codex'").unwrap();
        assert!(executable_check < version_check);
        assert!(version_check < directory_check);
        assert!(directory_check < directory_change);
        assert!(directory_change < launch);
        assert!(script.contains("'review this'"));
        assert!(script.contains("exec \"${SHELL:-/bin/sh}\""));
    }

    #[test]
    fn remote_agent_bootstrap_shell_quotes_untrusted_values() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::CustomCli,
            executable_override: Some("custom agent; touch /tmp/no".to_string()),
            working_directory: Some("/tmp/it's here; touch no".to_string()),
            arguments: vec!["argument; touch no".to_string()],
            worktree: SavedWorktreePolicy::SharedDirectory,
            ..SavedAgentDefinition::default()
        };
        let script = TermiRustApp::remote_agent_startup_script(&definition, "")
            .expect("custom remote bootstrap should be generated");

        assert!(script.contains("'custom agent; touch /tmp/no'"));
        assert!(script.contains("'/tmp/it'\"'\"'s here; touch no'"));
        assert!(script.contains("'argument; touch no'"));
        assert!(!script.lines().any(|line| line.starts_with("touch ")));
    }
}
