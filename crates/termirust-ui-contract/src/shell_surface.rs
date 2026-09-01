use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::agent_canvas_surface::{AgentCanvasAccessibilityCommand, AgentCanvasSemanticSnapshot};
use crate::host_connection_surface::{
    HostConnectionAccessibilityCommand, HostConnectionSemanticSnapshot,
};
use crate::preset_runtime_surface::{
    PresetRuntimeAccessibilityCommand, PresetRuntimeSemanticSnapshot,
};
use crate::product_session_surface::{
    ProductSessionAccessibilityCommand, ProductSessionSemanticSnapshot,
};
use crate::settings_surface::{SettingsAccessibilityCommand, SettingsSemanticSnapshot};
use crate::sftp_surface::{SftpAccessibilityCommand, SftpSemanticSnapshot};
use crate::terminal_surface::{TerminalAccessibilityCommand, TerminalSemanticSnapshot};
use crate::vault_key_snippet_surface::{
    VaultKeySnippetAccessibilityCommand, VaultKeySnippetSemanticSnapshot,
};
use crate::worktree_artifact_surface::{
    WorktreeArtifactAccessibilityCommand, WorktreeArtifactSemanticSnapshot,
};
use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticActionRouter, SemanticError,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticText, SemanticTree, SemanticValue,
};

pub const MAX_OVERLAY_DEPTH: usize = 16;
pub const MAX_PALETTE_RESULTS: usize = 1_000;
pub const PALETTE_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);
const SHELL_ROOT_NODE: u64 = 1;
const SHELL_SKIP_NODE: u64 = 2;
const SHELL_REGION_NODE_BASE: u64 = 10;
const SHELL_PALETTE_NODE: u64 = 20;
const SHELL_PALETTE_INPUT_NODE: u64 = 21;
const SHELL_PALETTE_RESULTS_NODE: u64 = 22;
const SHELL_PALETTE_RESULT_BASE: u64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellTextScale(u16);

impl ShellTextScale {
    pub const fn try_new(percent: u16) -> Option<Self> {
        if percent >= 100 && percent <= 400 {
            Some(Self(percent))
        } else {
            None
        }
    }

    pub const fn percent(self) -> u16 {
        self.0
    }

    pub const fn layout(self) -> ShellResponsiveLayout {
        match self.0 {
            100..=150 => ShellResponsiveLayout::Standard,
            151..=200 => ShellResponsiveLayout::Compact,
            _ => ShellResponsiveLayout::Stacked,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellResponsiveLayout {
    Standard,
    Compact,
    Stacked,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ShellRegionId {
    WindowChrome,
    PrimaryNavigation,
    Content,
    Inspector,
    Status,
}

impl ShellRegionId {
    pub const ORDER: [Self; 5] = [
        Self::WindowChrome,
        Self::PrimaryNavigation,
        Self::Content,
        Self::Inspector,
        Self::Status,
    ];

    pub fn next_available(self, reverse: bool, available: impl Fn(Self) -> bool) -> Option<Self> {
        let index = Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)?;
        (1..=Self::ORDER.len())
            .map(|offset| {
                if reverse {
                    (index + Self::ORDER.len() - offset) % Self::ORDER.len()
                } else {
                    (index + offset) % Self::ORDER.len()
                }
            })
            .map(|candidate| Self::ORDER[candidate])
            .find(|candidate| available(*candidate))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OverlayId(u64);

impl OverlayId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OverlayKind {
    Menu,
    Popover,
    Dialog,
    SecurityPrompt,
    Toast,
    Activity,
    GlobalPalette,
}

impl OverlayKind {
    pub const fn is_modal(self) -> bool {
        matches!(
            self,
            Self::Dialog | Self::SecurityPrompt | Self::GlobalPalette
        )
    }

    pub const fn priority(self) -> u8 {
        match self {
            Self::Toast => 0,
            Self::Activity => 1,
            Self::Menu | Self::Popover => 2,
            Self::GlobalPalette => 3,
            Self::Dialog => 4,
            Self::SecurityPrompt => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FocusReturn {
    Exact(u64),
    Region(ShellRegionId),
    FirstAvailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnnouncementPolicy {
    None,
    Polite,
    Immediate,
    Coalesced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayPhase {
    Opening,
    Open,
    Closing,
    Restored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayOwner {
    pub window_generation: u64,
    pub owner: FocusReturn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayFrame {
    pub id: OverlayId,
    pub kind: OverlayKind,
    pub owner: OverlayOwner,
    pub focus_scope: u64,
    pub safe_action: FocusReturn,
    pub announcements: AnnouncementPolicy,
    pub phase: OverlayPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayError {
    DuplicateId,
    MissingOverlay,
    ResourceLimit,
    StaleWindow,
    SecurityPromptObscured,
    InvalidTransition,
}

impl fmt::Display for OverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "overlay contract error: {self:?}")
    }
}

impl std::error::Error for OverlayError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayCloseResult {
    pub closed: OverlayId,
    pub focus: FocusReturn,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OverlayStack {
    frames: Vec<OverlayFrame>,
}

impl OverlayStack {
    pub fn frames(&self) -> &[OverlayFrame] {
        &self.frames
    }

    pub fn top(&self) -> Option<&OverlayFrame> {
        self.frames.last()
    }

    pub fn open(&mut self, frame: OverlayFrame) -> Result<(), OverlayError> {
        if self.frames.len() >= MAX_OVERLAY_DEPTH {
            return Err(OverlayError::ResourceLimit);
        }
        if self.frames.iter().any(|candidate| candidate.id == frame.id) {
            return Err(OverlayError::DuplicateId);
        }
        if frame.phase != OverlayPhase::Opening {
            return Err(OverlayError::InvalidTransition);
        }
        if let Some(security) = self
            .frames
            .iter()
            .rfind(|candidate| candidate.kind == OverlayKind::SecurityPrompt)
            && frame.kind.priority() < security.kind.priority()
        {
            return Err(OverlayError::SecurityPromptObscured);
        }
        self.frames.push(frame);
        Ok(())
    }

    pub fn mark_open(&mut self, id: OverlayId, window_generation: u64) -> Result<(), OverlayError> {
        let frame = self.frame_mut(id, window_generation)?;
        if frame.phase != OverlayPhase::Opening {
            return Err(OverlayError::InvalidTransition);
        }
        frame.phase = OverlayPhase::Open;
        Ok(())
    }

    pub fn begin_close(
        &mut self,
        id: OverlayId,
        window_generation: u64,
    ) -> Result<(), OverlayError> {
        let index = self.frame_index(id, window_generation)?;
        for frame in &mut self.frames[index..] {
            if !matches!(frame.phase, OverlayPhase::Opening | OverlayPhase::Open) {
                return Err(OverlayError::InvalidTransition);
            }
            frame.phase = OverlayPhase::Closing;
        }
        Ok(())
    }

    pub fn finish_close(
        &mut self,
        id: OverlayId,
        window_generation: u64,
        focus_exists: impl Fn(FocusReturn) -> bool,
    ) -> Result<Vec<OverlayCloseResult>, OverlayError> {
        let index = self.frame_index(id, window_generation)?;
        if self.frames[index..]
            .iter()
            .any(|frame| frame.phase != OverlayPhase::Closing)
        {
            return Err(OverlayError::InvalidTransition);
        }
        let removed = self.frames.split_off(index);
        let mut results = Vec::with_capacity(removed.len());
        for mut frame in removed.into_iter().rev() {
            frame.phase = OverlayPhase::Restored;
            let focus = if focus_exists(frame.owner.owner) {
                frame.owner.owner
            } else if focus_exists(frame.safe_action) {
                frame.safe_action
            } else {
                FocusReturn::FirstAvailable
            };
            results.push(OverlayCloseResult {
                closed: frame.id,
                focus,
            });
        }
        Ok(results)
    }

    pub fn escape_action(&self, window_generation: u64) -> Result<FocusReturn, OverlayError> {
        let frame = self.top().ok_or(OverlayError::MissingOverlay)?;
        if frame.owner.window_generation != window_generation {
            return Err(OverlayError::StaleWindow);
        }
        Ok(frame.safe_action)
    }

    fn frame_index(&self, id: OverlayId, window_generation: u64) -> Result<usize, OverlayError> {
        let index = self
            .frames
            .iter()
            .position(|candidate| candidate.id == id)
            .ok_or(OverlayError::MissingOverlay)?;
        if self.frames[index].owner.window_generation != window_generation {
            return Err(OverlayError::StaleWindow);
        }
        Ok(index)
    }

    fn frame_mut(
        &mut self,
        id: OverlayId,
        window_generation: u64,
    ) -> Result<&mut OverlayFrame, OverlayError> {
        let index = self.frame_index(id, window_generation)?;
        Ok(&mut self.frames[index])
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PaletteResultId(u64);

impl PaletteResultId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for PaletteResultId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PaletteResultId(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaletteError {
    DuplicateResult,
    ResourceLimit,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PalettePresentationState {
    selected: Option<PaletteResultId>,
    selected_index: usize,
    last_announcement: Option<Instant>,
    pending_announcement: bool,
}

impl PalettePresentationState {
    pub const fn selected_id(&self) -> Option<PaletteResultId> {
        self.selected
    }

    pub const fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn reset(&mut self) {
        self.selected = None;
        self.selected_index = 0;
        self.pending_announcement = false;
    }

    pub fn select(&mut self, ids: &[PaletteResultId], index: usize) -> bool {
        let Some(id) = ids.get(index).copied() else {
            return false;
        };
        self.selected = Some(id);
        self.selected_index = index;
        true
    }

    pub fn reconcile(&mut self, ids: &[PaletteResultId]) -> Result<usize, PaletteError> {
        if ids.len() > MAX_PALETTE_RESULTS {
            return Err(PaletteError::ResourceLimit);
        }
        let mut unique = HashSet::with_capacity(ids.len());
        if ids.iter().any(|id| !unique.insert(*id)) {
            return Err(PaletteError::DuplicateResult);
        }
        if ids.is_empty() {
            self.selected = None;
            self.selected_index = 0;
            return Ok(0);
        }
        let index = self
            .selected
            .and_then(|selected| ids.iter().position(|candidate| *candidate == selected))
            .unwrap_or_else(|| self.selected_index.min(ids.len() - 1));
        self.selected = Some(ids[index]);
        self.selected_index = index;
        Ok(index)
    }

    pub fn request_announcement(&mut self, now: Instant) -> bool {
        if self
            .last_announcement
            .is_none_or(|last| now.saturating_duration_since(last) >= PALETTE_ANNOUNCEMENT_INTERVAL)
        {
            self.last_announcement = Some(now);
            self.pending_announcement = false;
            return true;
        }
        self.pending_announcement = true;
        false
    }

    pub fn flush_announcement(&mut self, now: Instant) -> bool {
        if self.pending_announcement
            && self.last_announcement.is_none_or(|last| {
                now.saturating_duration_since(last) >= PALETTE_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last_announcement = Some(now);
            self.pending_announcement = false;
            return true;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellAccessibilityCommand {
    SkipToContent,
    FocusRegion(ShellRegionId),
    DismissPalette,
    FocusPaletteInput,
    SetPaletteQuery,
    FocusPaletteResult(usize),
    ActivatePaletteResult(usize),
    ProductSession(ProductSessionAccessibilityCommand),
    PresetRuntime(PresetRuntimeAccessibilityCommand),
    WorktreeArtifact(WorktreeArtifactAccessibilityCommand),
    HostConnection(HostConnectionAccessibilityCommand),
    Sftp(SftpAccessibilityCommand),
    VaultKeySnippet(VaultKeySnippetAccessibilityCommand),
    Settings(SettingsAccessibilityCommand),
    AgentCanvas(AgentCanvasAccessibilityCommand),
    Terminal(TerminalAccessibilityCommand),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellSemanticSnapshot {
    pub generation: u64,
    pub inspector_visible: bool,
    pub palette_open: bool,
    pub palette_result_count: usize,
    pub selected_palette_result: usize,
    pub status_urgency: AnnouncementPolicy,
    pub product_session: Option<ProductSessionSemanticSnapshot>,
    pub preset_runtime: Option<PresetRuntimeSemanticSnapshot>,
    pub worktree_artifact: Option<WorktreeArtifactSemanticSnapshot>,
    pub host_connection: Option<HostConnectionSemanticSnapshot>,
    pub sftp: Option<SftpSemanticSnapshot>,
    pub vault_key_snippet: Option<VaultKeySnippetSemanticSnapshot>,
    pub settings: Option<SettingsSemanticSnapshot>,
    pub agent_canvas: Option<AgentCanvasSemanticSnapshot>,
    pub terminal: Option<TerminalSemanticSnapshot>,
}

impl Default for ShellSemanticSnapshot {
    fn default() -> Self {
        Self {
            generation: 1,
            inspector_visible: false,
            palette_open: false,
            palette_result_count: 0,
            selected_palette_result: 0,
            status_urgency: AnnouncementPolicy::Polite,
            product_session: None,
            preset_runtime: None,
            worktree_artifact: None,
            host_connection: None,
            sftp: None,
            vault_key_snippet: None,
            settings: None,
            agent_canvas: None,
            terminal: None,
        }
    }
}

impl ShellSemanticSnapshot {
    pub fn try_tree(&self) -> Result<SemanticTree, SemanticError> {
        if self.palette_result_count > MAX_PALETTE_RESULTS {
            return Err(SemanticError::new(
                crate::SemanticErrorCode::ResourceLimit,
                None,
            ));
        }
        let root = semantic_id(SHELL_ROOT_NODE);
        let mut nodes = vec![SemanticNode::new(root, SemanticRole::Application)];

        let mut skip = named_node(
            SHELL_SKIP_NODE,
            SemanticRole::Button,
            MessageId::ShellSkipContent,
        );
        skip.parent = Some(root);
        skip.state.hidden = self.palette_open;
        skip.actions.insert(SemanticAction::Activate);
        nodes.push(skip);

        for region in ShellRegionId::ORDER {
            if region == ShellRegionId::Inspector && !self.inspector_visible {
                continue;
            }
            let mut node = named_node(
                shell_region_node(region).get(),
                if region == ShellRegionId::Status {
                    SemanticRole::Status
                } else {
                    SemanticRole::Landmark
                },
                shell_region_message(region),
            );
            node.parent = Some(root);
            node.state.hidden = self.palette_open;
            node.actions.insert(SemanticAction::Focus);
            if region == ShellRegionId::Status {
                node.state.live = match self.status_urgency {
                    AnnouncementPolicy::None => None,
                    AnnouncementPolicy::Immediate => Some(LiveRegionPoliteness::Immediate),
                    AnnouncementPolicy::Polite | AnnouncementPolicy::Coalesced => {
                        Some(LiveRegionPoliteness::Polite)
                    }
                };
            }
            nodes.push(node);
        }

        if !self.palette_open
            && let Some(product_session) = self.product_session.as_ref()
        {
            nodes.extend(product_session.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(preset_runtime) = self.preset_runtime.as_ref()
        {
            nodes.extend(preset_runtime.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(worktree_artifact) = self.worktree_artifact.as_ref()
        {
            nodes.extend(worktree_artifact.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(host_connection) = self.host_connection.as_ref()
        {
            nodes.extend(host_connection.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(sftp) = self.sftp.as_ref()
        {
            nodes.extend(sftp.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(vault_key_snippet) = self.vault_key_snippet.as_ref()
        {
            nodes.extend(vault_key_snippet.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(settings) = self.settings.as_ref()
        {
            nodes.extend(settings.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(agent_canvas) = self.agent_canvas.as_ref()
        {
            nodes.extend(agent_canvas.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }
        if !self.palette_open
            && let Some(terminal) = self.terminal.as_ref()
        {
            nodes.extend(terminal.try_nodes(shell_region_node(ShellRegionId::Content))?);
        }

        if self.palette_open {
            let palette = semantic_id(SHELL_PALETTE_NODE);
            let mut dialog = named_node(
                SHELL_PALETTE_NODE,
                SemanticRole::Dialog,
                MessageId::GlobalPaletteTitle,
            );
            dialog.parent = Some(root);
            dialog
                .actions
                .extend([SemanticAction::Dismiss, SemanticAction::Cancel]);
            nodes.push(dialog);

            let mut input = named_node(
                SHELL_PALETTE_INPUT_NODE,
                SemanticRole::TextField,
                MessageId::GlobalPalettePlaceholder,
            );
            input.parent = Some(palette);
            input
                .actions
                .extend([SemanticAction::Focus, SemanticAction::SetValue]);
            nodes.push(input);

            let mut results =
                SemanticNode::new(semantic_id(SHELL_PALETTE_RESULTS_NODE), SemanticRole::List);
            results.parent = Some(palette);
            nodes.push(results);

            for index in 0..self.palette_result_count {
                let mut result = named_node(
                    SHELL_PALETTE_RESULT_BASE + index as u64,
                    SemanticRole::ListItem,
                    MessageId::ShellPaletteResult,
                );
                result.parent = Some(semantic_id(SHELL_PALETTE_RESULTS_NODE));
                result.state.selected = index == self.selected_palette_result;
                result.value = Some(SemanticValue::Number {
                    current: index as i64 + 1,
                    minimum: 1,
                    maximum: self.palette_result_count as i64,
                });
                result
                    .actions
                    .extend([SemanticAction::Focus, SemanticAction::Activate]);
                nodes.push(result);
            }
        }

        SemanticTree::try_new(self.generation.max(1), root, nodes)
    }

    pub fn try_router(
        &self,
        tree: &SemanticTree,
    ) -> Result<SemanticActionRouter<ShellAccessibilityCommand>, SemanticError> {
        let mut routes = vec![(
            (semantic_id(SHELL_SKIP_NODE), SemanticAction::Activate),
            ShellAccessibilityCommand::SkipToContent,
        )];
        for region in ShellRegionId::ORDER {
            if region != ShellRegionId::Inspector || self.inspector_visible {
                routes.push((
                    (shell_region_node(region), SemanticAction::Focus),
                    ShellAccessibilityCommand::FocusRegion(region),
                ));
            }
        }
        if !self.palette_open
            && let Some(product_session) = self.product_session.as_ref()
        {
            routes.extend(
                product_session.routes().into_iter().map(|(key, command)| {
                    (key, ShellAccessibilityCommand::ProductSession(command))
                }),
            );
        }
        if !self.palette_open
            && let Some(preset_runtime) = self.preset_runtime.as_ref()
        {
            routes.extend(
                preset_runtime
                    .routes()
                    .into_iter()
                    .map(|(key, command)| (key, ShellAccessibilityCommand::PresetRuntime(command))),
            );
        }
        if !self.palette_open
            && let Some(worktree_artifact) = self.worktree_artifact.as_ref()
        {
            routes.extend(
                worktree_artifact
                    .routes()
                    .into_iter()
                    .map(|(key, command)| {
                        (key, ShellAccessibilityCommand::WorktreeArtifact(command))
                    }),
            );
        }
        if !self.palette_open
            && let Some(host_connection) = self.host_connection.as_ref()
        {
            routes.extend(
                host_connection.routes().into_iter().map(|(key, command)| {
                    (key, ShellAccessibilityCommand::HostConnection(command))
                }),
            );
        }
        if !self.palette_open
            && let Some(sftp) = self.sftp.as_ref()
        {
            routes.extend(
                sftp.routes()
                    .into_iter()
                    .map(|(key, command)| (key, ShellAccessibilityCommand::Sftp(command))),
            );
        }
        if !self.palette_open
            && let Some(vault_key_snippet) = self.vault_key_snippet.as_ref()
        {
            routes.extend(
                vault_key_snippet
                    .routes()
                    .into_iter()
                    .map(|(key, command)| {
                        (key, ShellAccessibilityCommand::VaultKeySnippet(command))
                    }),
            );
        }
        if !self.palette_open
            && let Some(settings) = self.settings.as_ref()
        {
            routes.extend(
                settings
                    .routes()
                    .into_iter()
                    .map(|(key, command)| (key, ShellAccessibilityCommand::Settings(command))),
            );
        }
        if !self.palette_open
            && let Some(agent_canvas) = self.agent_canvas.as_ref()
        {
            routes.extend(
                agent_canvas
                    .routes()?
                    .into_iter()
                    .map(|(key, command)| (key, ShellAccessibilityCommand::AgentCanvas(command))),
            );
        }
        if !self.palette_open
            && let Some(terminal) = self.terminal.as_ref()
        {
            routes.extend(
                terminal
                    .routes()
                    .into_iter()
                    .map(|(key, command)| (key, ShellAccessibilityCommand::Terminal(command))),
            );
        }
        if self.palette_open {
            routes.extend([
                (
                    (semantic_id(SHELL_PALETTE_NODE), SemanticAction::Dismiss),
                    ShellAccessibilityCommand::DismissPalette,
                ),
                (
                    (semantic_id(SHELL_PALETTE_NODE), SemanticAction::Cancel),
                    ShellAccessibilityCommand::DismissPalette,
                ),
                (
                    (semantic_id(SHELL_PALETTE_INPUT_NODE), SemanticAction::Focus),
                    ShellAccessibilityCommand::FocusPaletteInput,
                ),
                (
                    (
                        semantic_id(SHELL_PALETTE_INPUT_NODE),
                        SemanticAction::SetValue,
                    ),
                    ShellAccessibilityCommand::SetPaletteQuery,
                ),
            ]);
            for index in 0..self.palette_result_count {
                let id = semantic_id(SHELL_PALETTE_RESULT_BASE + index as u64);
                routes.push((
                    (id, SemanticAction::Focus),
                    ShellAccessibilityCommand::FocusPaletteResult(index),
                ));
                routes.push((
                    (id, SemanticAction::Activate),
                    ShellAccessibilityCommand::ActivatePaletteResult(index),
                ));
            }
        }
        SemanticActionRouter::try_new(tree, routes)
    }
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("semantic node IDs are non-zero"))
}

fn shell_region_node(region: ShellRegionId) -> SemanticNodeId {
    semantic_id(
        SHELL_REGION_NODE_BASE
            + ShellRegionId::ORDER
                .iter()
                .position(|candidate| *candidate == region)
                .expect("known shell region") as u64,
    )
}

pub fn shell_region_semantic_node(region: ShellRegionId) -> SemanticNodeId {
    shell_region_node(region)
}

pub fn shell_palette_input_semantic_node() -> SemanticNodeId {
    semantic_id(SHELL_PALETTE_INPUT_NODE)
}

fn shell_region_message(region: ShellRegionId) -> MessageId {
    match region {
        ShellRegionId::WindowChrome => MessageId::ShellRegionWindowChrome,
        ShellRegionId::PrimaryNavigation => MessageId::ShellRegionPrimaryNavigation,
        ShellRegionId::Content => MessageId::ShellRegionContent,
        ShellRegionId::Inspector => MessageId::ShellRegionInspector,
        ShellRegionId::Status => MessageId::ShellRegionStatus,
    }
}

fn named_node(value: u64, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(semantic_id(value), role);
    node.name = Some(SemanticText::Message(name));
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessibleCollectionRow, AccessibleRowId, HierarchyLevel, ProductSessionScreen,
        ProductSessionSurfaceState,
    };

    fn id(value: u64) -> OverlayId {
        OverlayId::new(value).unwrap()
    }

    fn frame(value: u64, kind: OverlayKind, owner: FocusReturn) -> OverlayFrame {
        OverlayFrame {
            id: id(value),
            kind,
            owner: OverlayOwner {
                window_generation: 7,
                owner,
            },
            focus_scope: value + 100,
            safe_action: FocusReturn::Region(ShellRegionId::Content),
            announcements: AnnouncementPolicy::Polite,
            phase: OverlayPhase::Opening,
        }
    }

    #[test]
    fn shell_focus_order_skips_unavailable_regions_in_both_directions() {
        assert_eq!(
            ShellRegionId::WindowChrome.next_available(false, |region| {
                matches!(region, ShellRegionId::Content | ShellRegionId::Status)
            }),
            Some(ShellRegionId::Content)
        );
        assert_eq!(
            ShellRegionId::Content.next_available(true, |region| {
                matches!(region, ShellRegionId::WindowChrome | ShellRegionId::Status)
            }),
            Some(ShellRegionId::WindowChrome)
        );
    }

    #[test]
    fn nested_owner_close_unwinds_children_and_restores_exact_focus() {
        let mut stack = OverlayStack::default();
        stack
            .open(frame(1, OverlayKind::Dialog, FocusReturn::Exact(41)))
            .unwrap();
        stack.mark_open(id(1), 7).unwrap();
        stack
            .open(frame(2, OverlayKind::Popover, FocusReturn::Exact(42)))
            .unwrap();
        stack.mark_open(id(2), 7).unwrap();
        stack.begin_close(id(1), 7).unwrap();
        let closed = stack
            .finish_close(id(1), 7, |target| target == FocusReturn::Exact(41))
            .unwrap();
        assert_eq!(closed.len(), 2);
        assert_eq!(closed[0].closed, id(2));
        assert_eq!(closed[0].focus, FocusReturn::FirstAvailable);
        assert_eq!(closed[1].focus, FocusReturn::Exact(41));
        assert!(stack.frames().is_empty());
    }

    #[test]
    fn stale_focus_and_window_use_deterministic_recovery() {
        let mut stack = OverlayStack::default();
        stack
            .open(frame(1, OverlayKind::GlobalPalette, FocusReturn::Exact(99)))
            .unwrap();
        stack.mark_open(id(1), 7).unwrap();
        assert_eq!(stack.escape_action(8), Err(OverlayError::StaleWindow));
        stack.begin_close(id(1), 7).unwrap();
        let result = stack.finish_close(id(1), 7, |_| false).unwrap();
        assert_eq!(result[0].focus, FocusReturn::FirstAvailable);
    }

    #[test]
    fn ordinary_overlay_cannot_obscure_security_prompt() {
        let mut stack = OverlayStack::default();
        stack
            .open(frame(
                1,
                OverlayKind::SecurityPrompt,
                FocusReturn::FirstAvailable,
            ))
            .unwrap();
        assert_eq!(
            stack.open(frame(2, OverlayKind::Toast, FocusReturn::FirstAvailable)),
            Err(OverlayError::SecurityPromptObscured)
        );
    }

    #[test]
    fn overlay_depth_is_bounded() {
        let mut stack = OverlayStack::default();
        for value in 1..=MAX_OVERLAY_DEPTH as u64 {
            stack
                .open(frame(
                    value,
                    OverlayKind::Dialog,
                    FocusReturn::FirstAvailable,
                ))
                .unwrap();
        }
        assert_eq!(
            stack.open(frame(99, OverlayKind::Dialog, FocusReturn::FirstAvailable)),
            Err(OverlayError::ResourceLimit)
        );
    }

    fn palette_id(value: u64) -> PaletteResultId {
        PaletteResultId::new(value).unwrap()
    }

    #[test]
    fn palette_replacement_preserves_selection_by_result_id() {
        let mut state = PalettePresentationState::default();
        let original = [palette_id(1), palette_id(2), palette_id(3)];
        state.reconcile(&original).unwrap();
        assert!(state.select(&original, 1));
        let replacement = [palette_id(8), palette_id(2), palette_id(9)];
        assert_eq!(state.reconcile(&replacement).unwrap(), 1);
        assert_eq!(state.selected_id(), Some(palette_id(2)));
    }

    #[test]
    fn palette_reconciliation_handles_one_thousand_results_and_rejects_more() {
        let ids = (1..=MAX_PALETTE_RESULTS as u64)
            .map(palette_id)
            .collect::<Vec<_>>();
        let mut state = PalettePresentationState::default();
        assert_eq!(state.reconcile(&ids), Ok(0));
        let mut too_many = ids;
        too_many.push(palette_id(1_001));
        assert_eq!(state.reconcile(&too_many), Err(PaletteError::ResourceLimit));
    }

    #[test]
    fn palette_announcements_are_coalesced() {
        let start = Instant::now();
        let mut state = PalettePresentationState::default();
        assert!(state.request_announcement(start));
        assert!(!state.request_announcement(start + Duration::from_millis(10)));
        assert!(!state.flush_announcement(start + Duration::from_millis(249)));
        assert!(state.flush_announcement(start + Duration::from_millis(250)));
    }

    #[test]
    fn shell_semantics_expose_landmarks_and_privacy_safe_palette_results() {
        let snapshot = ShellSemanticSnapshot {
            inspector_visible: true,
            palette_open: true,
            palette_result_count: 1_000,
            selected_palette_result: 999,
            ..ShellSemanticSnapshot::default()
        };
        let tree = snapshot.try_tree().unwrap();
        assert_eq!(tree.nodes().len(), 1_010);
        assert!(tree.nodes().values().all(|node| {
            !matches!(node.name, Some(SemanticText::UserText(_)))
                && !matches!(node.description, Some(SemanticText::UserText(_)))
        }));
        assert!(
            tree.nodes()
                .values()
                .any(|node| node.role == SemanticRole::Dialog)
        );
        let selected = tree
            .nodes()
            .values()
            .filter(|node| node.state.selected)
            .count();
        assert_eq!(selected, 1);
        assert!(tree.nodes().values().any(|node| {
            node.state.selected
                && node.value
                    == Some(SemanticValue::Number {
                        current: 1_000,
                        minimum: 1,
                        maximum: 1_000,
                    })
        }));
        snapshot.try_router(&tree).unwrap();
    }

    #[test]
    fn shell_semantics_reject_more_than_the_palette_resource_limit() {
        let snapshot = ShellSemanticSnapshot {
            palette_open: true,
            palette_result_count: MAX_PALETTE_RESULTS + 1,
            ..ShellSemanticSnapshot::default()
        };
        assert_eq!(
            snapshot.try_tree().unwrap_err().code,
            crate::SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn shell_scale_contract_reflows_at_two_and_four_hundred_percent() {
        assert_eq!(
            ShellTextScale::try_new(200).unwrap().layout(),
            ShellResponsiveLayout::Compact
        );
        assert_eq!(
            ShellTextScale::try_new(400).unwrap().layout(),
            ShellResponsiveLayout::Stacked
        );
        assert!(ShellTextScale::try_new(99).is_none());
        assert!(ShellTextScale::try_new(401).is_none());
    }

    #[test]
    fn shell_messages_and_tokens_resolve_for_every_required_locale_and_theme() {
        for locale in crate::Locale::ALL {
            let localizer = crate::Localizer::try_new(locale.tag()).unwrap();
            let label = localizer
                .format_static(MessageId::ShellRegionPrimaryNavigation)
                .unwrap();
            assert!(!label.trim().is_empty());
        }
        for theme in crate::ThemeKind::ALL {
            let tokens = crate::DesignTokens::new(theme);
            assert!(tokens.layout_palette_width().0 > 0.0);
            assert!(tokens.color_focus().alpha > 0);
            assert!(tokens.radius_dialog().0 >= tokens.radius_control().0);
        }
    }

    #[test]
    fn shell_routes_product_collection_rows_by_typed_identity() {
        let project_id = AccessibleRowId::project(0x1111);
        let row_id = AccessibleRowId::session(0x1234);
        let product = ProductSessionSemanticSnapshot {
            screen: ProductSessionScreen::Sessions,
            state: ProductSessionSurfaceState::Ready,
            rows: vec![
                AccessibleCollectionRow {
                    id: project_id,
                    parent: None,
                    level: HierarchyLevel::Project,
                    name: "Synthetic project".to_string(),
                    status: MessageId::ProductSurfaceStateReady,
                    selected: false,
                    expanded: Some(true),
                    unread: false,
                    disabled: false,
                    position: 1,
                    set_size: 1,
                },
                AccessibleCollectionRow {
                    id: row_id,
                    parent: Some(project_id),
                    level: HierarchyLevel::Session,
                    name: "Synthetic session".to_string(),
                    status: MessageId::ProductSurfaceStateReady,
                    selected: true,
                    expanded: None,
                    unread: true,
                    disabled: false,
                    position: 1,
                    set_size: 1,
                },
            ],
            controls: Vec::new(),
            dialog: None,
            recording_friendly: false,
        };
        let product_route = product
            .routes()
            .into_iter()
            .find(|(_, command)| {
                *command == ProductSessionAccessibilityCommand::ActivateRow(row_id)
            })
            .unwrap();
        let snapshot = ShellSemanticSnapshot {
            product_session: Some(product),
            ..ShellSemanticSnapshot::default()
        };
        let tree = snapshot.try_tree().unwrap();
        let router = snapshot.try_router(&tree).unwrap();
        let command = router
            .resolve(crate::SemanticActionRequest {
                generation: tree.generation(),
                node: product_route.0.0,
                action: product_route.0.1,
                value: None,
            })
            .unwrap();
        assert_eq!(
            *command,
            ShellAccessibilityCommand::ProductSession(
                ProductSessionAccessibilityCommand::ActivateRow(row_id)
            )
        );
    }

    #[test]
    fn shell_routes_worktree_artifact_controls_without_exposing_preview_bytes() {
        let row = crate::WorktreeArtifactRowId::artifact(7, 11);
        let action = crate::WorktreeArtifactAction::PreviewArtifact(row);
        let surface = crate::WorktreeArtifactSemanticSnapshot {
            screen: crate::WorktreeArtifactScreen::ArtifactGallery,
            state: crate::WorktreeArtifactSurfaceState::Ready,
            rows: vec![crate::WorktreeArtifactRow {
                id: row,
                parent: None,
                name: "evidence.html".to_string(),
                status: MessageId::ArtifactStateReady,
                detail: Some("metadata only, explicit import".to_string()),
                selected: true,
                disabled: false,
                expanded: Some(false),
                invalid: false,
                stale: false,
                position: 1,
                set_size: 1,
            }],
            controls: vec![crate::WorktreeArtifactControl {
                action,
                parent: Some(row),
                role: crate::WorktreeArtifactControlRole::Button,
                name: MessageId::ArtifactPreviewAction,
                value: None,
                selected: false,
                disabled: false,
                invalid: false,
            }],
            progress: None,
            recording_friendly: false,
        };
        let route = surface
            .routes()
            .into_iter()
            .find(|(_, command)| {
                *command == crate::WorktreeArtifactAccessibilityCommand::ActivateControl(action)
            })
            .unwrap();
        let snapshot = ShellSemanticSnapshot {
            worktree_artifact: Some(surface),
            ..ShellSemanticSnapshot::default()
        };
        let tree = snapshot.try_tree().unwrap();
        assert!(!format!("{tree:?}").contains("preview-byte-canary"));
        let router = snapshot.try_router(&tree).unwrap();
        assert_eq!(
            *router
                .resolve(crate::SemanticActionRequest {
                    generation: tree.generation(),
                    node: route.0.0,
                    action: route.0.1,
                    value: None,
                })
                .unwrap(),
            ShellAccessibilityCommand::WorktreeArtifact(
                crate::WorktreeArtifactAccessibilityCommand::ActivateControl(action)
            )
        );
    }

    #[test]
    fn shell_routes_host_controls_without_exposing_password_values() {
        let row = crate::HostConnectionRowId::host(crate::stable_host_row_value("host-1"));
        let action = crate::HostConnectionAction::ConnectHost(row);
        let surface = crate::HostConnectionSemanticSnapshot {
            screen: crate::HostConnectionScreen::HostEditor,
            state: crate::HostConnectionSurfaceState::Editing,
            rows: vec![crate::HostConnectionRow {
                id: row,
                parent: None,
                name: "Production".to_string(),
                status: MessageId::HostAuthPassword,
                detail: None,
                selected: true,
                disabled: false,
                checked: None,
                invalid: false,
                stale: false,
                position: 1,
                set_size: 1,
            }],
            controls: vec![
                crate::HostConnectionControl {
                    action: crate::HostConnectionAction::SetHostPassword,
                    parent: None,
                    role: crate::HostConnectionControlRole::PasswordField,
                    name: MessageId::HostPasswordField,
                    value: Some("password-canary".to_string()),
                    selected: false,
                    disabled: false,
                    invalid: false,
                },
                crate::HostConnectionControl {
                    action,
                    parent: Some(row),
                    role: crate::HostConnectionControlRole::Button,
                    name: MessageId::CommonConnect,
                    value: None,
                    selected: false,
                    disabled: false,
                    invalid: false,
                },
            ],
            recording_friendly: false,
        };
        let route = surface
            .routes()
            .into_iter()
            .find(|(_, command)| {
                *command == crate::HostConnectionAccessibilityCommand::ActivateControl(action)
            })
            .unwrap();
        let snapshot = ShellSemanticSnapshot {
            host_connection: Some(surface),
            inspector_visible: true,
            ..ShellSemanticSnapshot::default()
        };
        let tree = snapshot.try_tree().unwrap();
        assert!(!format!("{tree:?}").contains("password-canary"));
        let router = snapshot.try_router(&tree).unwrap();
        assert_eq!(
            *router
                .resolve(crate::SemanticActionRequest {
                    generation: tree.generation(),
                    node: route.0.0,
                    action: route.0.1,
                    value: None,
                })
                .unwrap(),
            ShellAccessibilityCommand::HostConnection(
                crate::HostConnectionAccessibilityCommand::ActivateControl(action)
            )
        );
    }

    #[test]
    fn shell_routes_exact_sftp_transfer_action_and_masks_paths() {
        let row = crate::SftpRowId::transfer(17, 41);
        let action = crate::SftpAction::CancelTransfer {
            workspace_id: 17,
            operation_id: 41,
        };
        let surface = crate::SftpSemanticSnapshot {
            screen: crate::SftpScreen::Workspace,
            state: crate::SftpSurfaceState::Transferring,
            rows: vec![crate::SftpRow {
                id: row,
                name: "/Users/private/upload.txt".to_string(),
                detail: Some("/remote/private/upload.txt".to_string()),
                status: MessageId::SftpStateTransferring,
                selected: true,
                disabled: false,
                activatable: false,
                stale: false,
                position: 1,
                set_size: 1,
            }],
            controls: vec![crate::SftpControl {
                action,
                parent: Some(row),
                role: crate::SftpControlRole::Button,
                name: MessageId::CommonCancel,
                value: None,
                selected: false,
                disabled: false,
                invalid: false,
            }],
            recording_friendly: true,
        };
        let route = surface
            .routes()
            .into_iter()
            .find(|(_, command)| {
                *command == crate::SftpAccessibilityCommand::ActivateControl(action)
            })
            .unwrap();
        let snapshot = ShellSemanticSnapshot {
            sftp: Some(surface),
            ..ShellSemanticSnapshot::default()
        };
        let tree = snapshot.try_tree().unwrap();
        let debug_tree = format!("{tree:?}");
        assert!(!debug_tree.contains("/Users/private"));
        assert!(!debug_tree.contains("/remote/private"));
        let router = snapshot.try_router(&tree).unwrap();
        assert_eq!(
            *router
                .resolve(crate::SemanticActionRequest {
                    generation: tree.generation(),
                    node: route.0.0,
                    action: route.0.1,
                    value: None,
                })
                .unwrap(),
            ShellAccessibilityCommand::Sftp(crate::SftpAccessibilityCommand::ActivateControl(
                action
            ))
        );
    }
}
