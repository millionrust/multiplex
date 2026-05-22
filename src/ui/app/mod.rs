mod chrome;
mod connect;
mod editor;
mod hosts;
mod library;
mod overlay;
mod palette;
mod sftp;
mod types;
mod workspace;

pub(crate) use types::{
    ConnectDialogMode, ConnectProtocol, DropZone, EditorMenu, HostsSort, HostsViewMode,
    ToolbarMenu, WorkspaceRuntimeTone, WorkspaceViewMode,
};

use palette::{
    CommandPaletteCandidate, OutputSuggestionContext, PathSuggestionContext,
    collect_autocomplete_candidates, collect_command_palette_candidates, pane_recent_output_lines,
};

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    ClipboardItem, InteractiveElement as _, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, StatefulInteractiveElement as _, font, *,
};
use gpui_component::IconName;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, Icon, Sizable, StyledExt as _, h_flex, v_flex};
use rfd::{AsyncFileDialog, FileDialog};
use vt100::MouseProtocolMode;

use crate::credentials;
use crate::local::spawn_local_session;
use crate::models::{
    AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DEFAULT_VAULT_ID, DraftProfile,
    HostColorTag, HostProfile, JumpHostConnection, PortForwardKind, PortForwardRule, ProfileSource,
    QuickConnect, SavedHostGroup, SavedIdentity, SavedSnippet, SavedSplitNode, SavedState,
    SavedVault, SavedVaultMember, SavedWindowBounds, SavedWorkspace, SessionLogEntry, SplitAxis,
    ThemePreset, VaultKind, VaultMemberRole,
};
use crate::sftp::{
    RemoteFileEntry, SftpEvent, spawn_delete_path, spawn_download_file, spawn_list_directory,
    spawn_upload_file,
};
use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent, spawn_session};
use crate::storage::{
    KnownHostStore, export_encrypted_portable_data_bundle, export_portable_data_bundle,
    import_encrypted_portable_data_bundle, import_portable_data_bundle, inspect_identity_file,
    load_local_ssh_identities, save_saved_state,
};
use crate::terminal::{TerminalSize, TerminalState};
use crate::ui::autocomplete::{AutocompleteCandidate, AutocompleteSource};
use crate::ui::keys::{MouseEventKind, encode_mouse_report, encode_terminal_input};
use crate::ui::path::remote_parent_path;
use crate::ui::render_terminal::{SelectionRange, normalized_selection};
use crate::ui::shell::{shell_command_requires_continuation, startup_bytes_for_request};
use crate::ui::snippet::{
    extract_snippet_prompt_names, substitute_snippet_placeholders, substitute_snippet_prompts,
};
use crate::ui::theme;
use crate::ui::util::{
    current_unix_millis, format_count_label, format_relative_time, format_tag_values, is_word_char,
    merge_tag_values, non_empty_string, parse_tag_values,
};

const TERMINAL_LINE_HEIGHT: f32 = 1.3;
const WORKSPACE_SEARCH_ROW_HEIGHT: f32 = 52.0;
const WORKSPACE_PADDING: f32 = 18.0;
const PANE_GAP: f32 = 12.0;
const TERMINAL_INNER_PADDING_X: f32 = 20.0;
const TERMINAL_INNER_PADDING_Y: f32 = 14.0;
const MAX_SPLIT_PANES: usize = 4;
const HOST_CARD_WIDTH: f32 = 300.0;
const ICON_KEY: &str = "icons/key.svg";
const ICON_SHIELD_CHECK: &str = "icons/shield-check.svg";
const ICON_VAULT: &str = "icons/vault.svg";
const ICON_X: &str = "icons/x.svg";
const ICON_PENCIL: &str = "icons/pencil.svg";
const ICON_GRID: &str = "icons/grid.svg";
const ICON_TAG: &str = "icons/tag.svg";
const ICON_CALENDAR: &str = "icons/calendar.svg";
const ICON_PANEL_COLLAPSE_RIGHT: &str = "icons/panel-collapse-right.svg";
const ICON_PALETTE: &str = "icons/palette.svg";

fn app_icon(path: &'static str) -> Icon {
    Icon::new(Icon::empty().path(path))
}

fn primary_shortcut_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Cmd"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl"
    }
}

fn ssh_directory_label() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "%USERPROFILE%\\.ssh"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "~/.ssh"
    }
}

fn ssh_config_path_label() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "%USERPROFILE%\\.ssh\\config"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "~/.ssh/config"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavSection {
    Hosts,
    Sftp,
    Vaults,
    Keychain,
    Snippets,
    Settings,
    KnownHosts,
    Logs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KeychainTab {
    #[default]
    Keys,
    Identities,
}

impl NavSection {
    fn label(self) -> &'static str {
        match self {
            Self::Hosts => "Hosts",
            Self::Sftp => "SFTP",
            Self::Vaults => "Vaults",
            Self::Keychain => "Keys",
            Self::Snippets => "Snippets",
            Self::Settings => "Settings",
            Self::KnownHosts => "Known Hosts",
            Self::Logs => "Logs",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Hosts => IconName::SquareTerminal.into(),
            Self::Sftp => IconName::Folder.into(),
            Self::Vaults => app_icon(ICON_VAULT),
            Self::Keychain => app_icon(ICON_KEY),
            Self::Snippets => IconName::BookOpen.into(),
            Self::Settings => IconName::Settings.into(),
            Self::KnownHosts => app_icon(ICON_SHIELD_CHECK),
            Self::Logs => IconName::BookOpen.into(),
        }
    }
}

use crate::ui::keys::TerminalCellPos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchMatch {
    full_row: usize,
    start_col: usize,
    end_col: usize,
}

#[derive(Clone, Copy, Debug)]
struct PaneLayout {
    pane_id: u64,
    cell_x: f32,
    cell_y: f32,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
    rows: u16,
    char_width: f32,
    line_height: f32,
}

struct DraftInputs {
    label: Entity<InputState>,
    group: Entity<InputState>,
    tags: Entity<InputState>,
    jump_host: Entity<InputState>,
    startup_directory: Entity<InputState>,
    startup_command: Entity<InputState>,
    terminal_scrollback_rows: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    key_path: Entity<InputState>,
    forward_local_port: Entity<InputState>,
    forward_remote_host: Entity<InputState>,
    forward_remote_port: Entity<InputState>,
    key_passphrase: Entity<InputState>,
    description: Entity<InputState>,
    environment: Entity<InputState>,
}

impl DraftInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            label: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Display name, e.g. Local Mac Test")
            }),
            group: cx.new(|cx| InputState::new(window, cx).placeholder("Folder/group, e.g. Local")),
            tags: cx.new(|cx| InputState::new(window, cx).placeholder("prod, blue, kubernetes")),
            jump_host: cx.new(|cx| InputState::new(window, cx).placeholder("Optional saved host")),
            startup_directory: cx.new(|cx| InputState::new(window, cx).placeholder("/var/www/app")),
            startup_command: cx
                .new(|cx| InputState::new(window, cx).placeholder("docker compose logs -f")),
            terminal_scrollback_rows: cx.new(|cx| InputState::new(window, cx).placeholder("10000")),
            host: cx.new(|cx| {
                InputState::new(window, cx).placeholder("localhost, IP address, or domain")
            }),
            port: cx.new(|cx| InputState::new(window, cx).default_value("22")),
            username: cx
                .new(|cx| InputState::new(window, cx).placeholder("SSH username, e.g. jacob")),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Password, only if using password auth")
            }),
            key_path: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Private key path, e.g. ~/.ssh/id_ed25519")
            }),
            forward_local_port: cx.new(|cx| InputState::new(window, cx).placeholder("15432")),
            forward_remote_host: cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1")),
            forward_remote_port: cx.new(|cx| InputState::new(window, cx).placeholder("5432")),
            key_passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Optional key passphrase")
            }),
            description: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Optional notes about this host")
            }),
            environment: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("AWS_PROFILE=prod\\nLOG_LEVEL=info (one KEY=value per line)")
            }),
        }
    }
}

struct ShellInputs {
    host_search: Entity<InputState>,
    quick_connect_password: Entity<InputState>,
    create_host_address: Entity<InputState>,
    connect_username: Entity<InputState>,
    protocol_ssh_port: Entity<InputState>,
    protocol_mosh_port: Entity<InputState>,
    protocol_mosh_command: Entity<InputState>,
    protocol_telnet_port: Entity<InputState>,
    sftp_local_filter: Entity<InputState>,
    bulk_group: Entity<InputState>,
    terminal_search: Entity<InputState>,
    command_palette: Entity<InputState>,
}

struct SnippetInputs {
    label: Entity<InputState>,
    group: Entity<InputState>,
    command: Entity<InputState>,
}

struct SettingsInputs {
    local_shell_program: Entity<InputState>,
    local_shell_cwd: Entity<InputState>,
    export_backup_passphrase: Entity<InputState>,
    export_backup_confirm: Entity<InputState>,
    import_backup_passphrase: Entity<InputState>,
    default_ssh_startup_directory: Entity<InputState>,
    terminal_font_family: Entity<InputState>,
    sync_folder_input: Entity<InputState>,
}

struct VaultInputs {
    label: Entity<InputState>,
    description: Entity<InputState>,
}

struct VaultMemberInputs {
    name: Entity<InputState>,
    email: Entity<InputState>,
}

impl SnippetInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            label: cx.new(|cx| InputState::new(window, cx).placeholder("Restart service")),
            group: cx.new(|cx| InputState::new(window, cx).placeholder("Ops / Deploy")),
            command: cx
                .new(|cx| InputState::new(window, cx).placeholder("sudo systemctl restart app")),
        }
    }
}

impl SettingsInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            local_shell_program: cx
                .new(|cx| InputState::new(window, cx).placeholder("Shell executable")),
            local_shell_cwd: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional working directory")),
            export_backup_passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Backup passphrase")
            }),
            export_backup_confirm: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Confirm passphrase")
            }),
            import_backup_passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Backup passphrase")
            }),
            default_ssh_startup_directory: cx
                .new(|cx| InputState::new(window, cx).placeholder("e.g. /home/user/projects")),
            terminal_font_family: cx.new(|cx| {
                InputState::new(window, cx).placeholder("e.g. JetBrains Mono, Fira Code")
            }),
            sync_folder_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("e.g. ~/Dropbox/TermiRust or any cloud-synced folder")
            }),
        }
    }
}

impl VaultInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            label: cx.new(|cx| InputState::new(window, cx).placeholder("Ops Team")),
            description: cx
                .new(|cx| InputState::new(window, cx).placeholder("Shared infrastructure access")),
        }
    }
}

impl VaultMemberInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Alex Rivera")),
            email: cx.new(|cx| InputState::new(window, cx).placeholder("alex@company.com")),
        }
    }
}

impl ShellInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            host_search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Find a host, group, tag, or ssh user@hostname...")
            }),
            quick_connect_password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Password")
            }),
            create_host_address: cx
                .new(|cx| InputState::new(window, cx).placeholder("Type IP or Hostname")),
            connect_username: cx.new(|cx| InputState::new(window, cx).placeholder("Username")),
            protocol_ssh_port: cx.new(|cx| InputState::new(window, cx).default_value("22")),
            protocol_mosh_port: cx.new(|cx| InputState::new(window, cx).default_value("22")),
            protocol_mosh_command: cx.new(|cx| {
                InputState::new(window, cx).default_value("mosh --server=/path/server host")
            }),
            protocol_telnet_port: cx.new(|cx| InputState::new(window, cx).default_value("23")),
            sftp_local_filter: cx.new(|cx| InputState::new(window, cx).placeholder("Filter files")),
            bulk_group: cx.new(|cx| InputState::new(window, cx).placeholder("Bulk group name")),
            terminal_search: cx
                .new(|cx| InputState::new(window, cx).placeholder("Search terminal output")),
            command_palette: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Run command, snippet, or recent task")
            }),
        }
    }
}

struct SessionPane {
    id: u64,
    request: ConnectRequest,
    title: String,
    endpoint: String,
    terminal: TerminalState,
    terminal_focus: FocusHandle,
    last_size: Option<TerminalSize>,
    runtime: SessionRuntimeHandle,
    connected: bool,
    closed: bool,
    status: String,
    selection: Option<SelectionRange>,
    dragging_selection: bool,
    log_id: String,
    current_input: String,
    selected_autocomplete_index: Option<usize>,
    auto_reconnect_attempts: u8,
    auto_reconnect_at: Option<u64>,
    user_closed: bool,
}

#[derive(Clone)]
struct PendingPaste {
    pane_id: u64,
    text: String,
}

struct SnippetPromptField {
    name: String,
    input: Entity<InputState>,
}

struct PendingSnippetPrompts {
    command: String,
    fields: Vec<SnippetPromptField>,
}

struct WorkspaceTab {
    id: u64,
    title: String,
    pane_ids: Vec<u64>,
    active_pane_id: u64,
    unread_events: u32,
    /// Recursive split layout. `None` while the workspace has no panes yet.
    layout: Option<SplitNode>,
    view_mode: WorkspaceViewMode,
    sftp: Option<WorkspaceSftpState>,
    search_visible: bool,
    search_query: String,
    search_results: Vec<SearchMatch>,
    active_search_index: Option<usize>,
    broadcast_input: bool,
    pending_connect: Option<HostProfile>,
    pending_connect_mode: ConnectDialogMode,
    pending_connect_protocol: ConnectProtocol,
    connect_failure: Option<ConnectFailure>,
}

impl WorkspaceTab {
    /// Rebuild the flat `pane_ids` cache from the `layout` tree.
    fn sync_pane_ids(&mut self) {
        self.pane_ids = self
            .layout
            .as_ref()
            .map(|layout| layout.leaf_ids())
            .unwrap_or_default();
        if !self.pane_ids.contains(&self.active_pane_id) {
            if let Some(first) = self.pane_ids.first().copied() {
                self.active_pane_id = first;
            }
        }
    }
}

/// Recursive split layout for a workspace — a binary tree of panes.
#[derive(Clone, Debug)]
enum SplitNode {
    /// A single terminal pane (by id).
    Leaf(u64),
    /// Two children laid out along `axis`; `ratio` is the fraction given to `a`.
    Split {
        axis: SplitAxis,
        ratio: f32,
        a: Box<SplitNode>,
        b: Box<SplitNode>,
    },
}

impl SplitNode {
    fn collect_leaves(&self, out: &mut Vec<u64>) {
        match self {
            SplitNode::Leaf(id) => out.push(*id),
            SplitNode::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    fn leaf_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn first_leaf(&self) -> u64 {
        match self {
            SplitNode::Leaf(id) => *id,
            SplitNode::Split { a, .. } => a.first_leaf(),
        }
    }

    /// Replace the leaf for `target` with a split of `target` and `new_node`.
    fn split_leaf(
        &mut self,
        target: u64,
        new_node: &SplitNode,
        axis: SplitAxis,
        new_first: bool,
    ) -> bool {
        match self {
            SplitNode::Leaf(id) if *id == target => {
                let existing = SplitNode::Leaf(*id);
                let (a, b) = if new_first {
                    (new_node.clone(), existing)
                } else {
                    (existing, new_node.clone())
                };
                *self = SplitNode::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(a),
                    b: Box::new(b),
                };
                true
            }
            SplitNode::Leaf(_) => false,
            SplitNode::Split { a, b, .. } => {
                a.split_leaf(target, new_node, axis, new_first)
                    || b.split_leaf(target, new_node, axis, new_first)
            }
        }
    }

    /// Remove `pane`'s leaf, collapsing the parent split into its sibling.
    fn without_pane(self, pane: u64) -> Option<SplitNode> {
        match self {
            SplitNode::Leaf(id) => (id != pane).then_some(SplitNode::Leaf(id)),
            SplitNode::Split { axis, ratio, a, b } => {
                match (a.without_pane(pane), b.without_pane(pane)) {
                    (None, None) => None,
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (Some(a), Some(b)) => Some(SplitNode::Split {
                        axis,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                }
            }
        }
    }

    /// Set the ratio of the split whose `b` subtree starts at `divider_id`.
    fn set_ratio(&mut self, divider_id: u64, ratio: f32) -> bool {
        if let SplitNode::Split { ratio: r, a, b, .. } = self {
            if b.first_leaf() == divider_id {
                *r = ratio.clamp(0.08, 0.92);
                return true;
            }
            return a.set_ratio(divider_id, ratio) || b.set_ratio(divider_id, ratio);
        }
        false
    }
}

/// Build a right-leaning flat split tree along one axis (used to reconstruct a
/// layout from saved state that predates nested splits).
fn flat_split(pane_ids: &[u64], axis: SplitAxis) -> Option<SplitNode> {
    let (first, rest) = pane_ids.split_first()?;
    let mut node = SplitNode::Leaf(*first);
    for id in rest {
        node = SplitNode::Split {
            axis,
            ratio: 0.5,
            a: Box::new(node),
            b: Box::new(SplitNode::Leaf(*id)),
        };
    }
    Some(node)
}

/// Convert a runtime `SplitNode` (pane ids) to its persistable form, mapping
/// each pane id to its index. Returns `None` if any pane id is missing.
fn split_node_to_saved(
    node: &SplitNode,
    indices: &std::collections::HashMap<u64, usize>,
) -> Option<SavedSplitNode> {
    match node {
        SplitNode::Leaf(id) => indices.get(id).copied().map(SavedSplitNode::Leaf),
        SplitNode::Split { axis, ratio, a, b } => Some(SavedSplitNode::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(split_node_to_saved(a, indices)?),
            b: Box::new(split_node_to_saved(b, indices)?),
        }),
    }
}

/// Rebuild a runtime `SplitNode` from its persisted form, mapping pane indices
/// back to live pane ids.
fn saved_to_split_node(node: &SavedSplitNode, pane_ids: &[u64]) -> Option<SplitNode> {
    match node {
        SavedSplitNode::Leaf(index) => pane_ids.get(*index).copied().map(SplitNode::Leaf),
        SavedSplitNode::Split { axis, ratio, a, b } => Some(SplitNode::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(saved_to_split_node(a, pane_ids)?),
            b: Box::new(saved_to_split_node(b, pane_ids)?),
        }),
    }
}

#[derive(Clone, Copy)]
struct PaneRect {
    pane_id: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy)]
struct DividerRect {
    divider_id: u64,
    axis: SplitAxis,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    span: f32,
    ratio: f32,
}

/// Walk a `SplitNode` tree, emitting a flat pixel rect for every pane leaf and
/// every divider, within the box `(x, y, width, height)`.
fn compute_split_layout(
    node: &SplitNode,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    panes: &mut Vec<PaneRect>,
    dividers: &mut Vec<DividerRect>,
) {
    match node {
        SplitNode::Leaf(id) => panes.push(PaneRect {
            pane_id: *id,
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        }),
        SplitNode::Split { axis, ratio, a, b } => {
            let ratio = ratio.clamp(0.08, 0.92);
            match axis {
                SplitAxis::Horizontal => {
                    let span = (width - PANE_GAP).max(2.0);
                    let aw = (span * ratio).max(1.0);
                    let bw = (span - aw).max(1.0);
                    compute_split_layout(a, x, y, aw, height, panes, dividers);
                    dividers.push(DividerRect {
                        divider_id: b.first_leaf(),
                        axis: *axis,
                        x: x + aw,
                        y,
                        width: PANE_GAP,
                        height,
                        span,
                        ratio,
                    });
                    compute_split_layout(b, x + aw + PANE_GAP, y, bw, height, panes, dividers);
                }
                SplitAxis::Vertical => {
                    let span = (height - PANE_GAP).max(2.0);
                    let ah = (span * ratio).max(1.0);
                    let bh = (span - ah).max(1.0);
                    compute_split_layout(a, x, y, width, ah, panes, dividers);
                    dividers.push(DividerRect {
                        divider_id: b.first_leaf(),
                        axis: *axis,
                        x,
                        y: y + ah,
                        width,
                        height: PANE_GAP,
                        span,
                        ratio,
                    });
                    compute_split_layout(b, x, y + ah + PANE_GAP, width, bh, panes, dividers);
                }
            }
        }
    }
}

#[derive(Clone)]
struct ConnectFailure {
    profile: HostProfile,
    protocol: ConnectProtocol,
    port: u16,
    log: Vec<String>,
}

#[derive(Clone)]
struct WorkspaceTabDrag {
    workspace_id: u64,
    title: String,
}

struct WorkspaceTabDragPreview {
    title: String,
}

/// In-progress drag of a split divider handle.
#[derive(Clone, Copy)]
struct DividerDrag {
    workspace_id: u64,
    /// Identifies the split: the first leaf pane id of the split's `b` subtree.
    divider_id: u64,
    axis: SplitAxis,
    /// Mouse position along the split axis when the drag started.
    origin: f32,
    start_ratio: f32,
    /// Pixels spanned by the split's two children (excludes the gap).
    span: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct WorkspaceIndicators {
    live_panes: usize,
    connecting_panes: usize,
    error_panes: usize,
    closed_panes: usize,
    split_count: usize,
    unread_events: u32,
}

#[derive(Clone, Debug)]
struct WorkspaceSftpState {
    pane_id: u64,
    request: ConnectRequest,
    current_path: String,
    entries: Vec<RemoteFileEntry>,
    selected_path: Option<String>,
    loading: bool,
    pending_operation_id: Option<u64>,
}

impl WorkspaceSftpState {
    fn new(pane_id: u64, request: ConnectRequest, current_path: String) -> Self {
        Self {
            pane_id,
            request,
            current_path,
            entries: Vec::new(),
            selected_path: None,
            loading: true,
            pending_operation_id: None,
        }
    }
}

impl Render for WorkspaceTabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("workspace-tab-drag-preview")
            .gap(px(7.))
            .items_center()
            .pl(px(12.))
            .pr(px(14.))
            .py(px(7.))
            .rounded(px(10.))
            .bg(theme::with_alpha(theme::chrome_bg(), 0.92))
            .border_1()
            .border_color(theme::with_alpha(theme::accent(), 0.4))
            .shadow_lg()
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(14.))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_semibold()
                    .text_color(theme::text_on_dark())
                    .child(self.title.clone()),
            )
    }
}

pub struct TermiRustApp {
    saved: SavedState,
    inputs: DraftInputs,
    shell_inputs: ShellInputs,
    snippet_inputs: SnippetInputs,
    settings_inputs: SettingsInputs,
    vault_inputs: VaultInputs,
    vault_member_inputs: VaultMemberInputs,
    draft_auth_mode: AuthMode,
    nav_section: NavSection,
    show_editor_panel: bool,
    event_tx: Sender<SshEvent>,
    event_rx: Receiver<SshEvent>,
    sftp_event_tx: Sender<SftpEvent>,
    sftp_event_rx: Receiver<SftpEvent>,
    panes: Vec<SessionPane>,
    workspaces: Vec<WorkspaceTab>,
    active_workspace_id: Option<u64>,
    selected_profile_id: Option<String>,
    selected_host_ids: HashSet<String>,
    selected_snippet_id: Option<String>,
    selected_vault_id: Option<String>,
    selected_vault_member_id: Option<String>,
    next_session_id: u64,
    next_sftp_operation_id: u64,
    next_workspace_id: u64,
    status_message: String,
    error_message: String,
    draft_identity_id: Option<String>,
    draft_vault_id: Option<String>,
    draft_profile_favorite: bool,
    draft_start_in_files: bool,
    draft_color_tag: Option<HostColorTag>,
    draft_port_forward_rules: Vec<PortForwardRule>,
    draft_port_forward_kind: PortForwardKind,
    snippet_vault_id: Option<String>,
    snippet_pinned: bool,
    draft_vault_member_role: VaultMemberRole,
    known_hosts: Arc<KnownHostStore>,
    keychain_tab: KeychainTab,
    show_command_palette: bool,
    show_new_host_menu: bool,
    hosts_view_mode: HostsViewMode,
    hosts_sort: HostsSort,
    open_toolbar_menu: Option<ToolbarMenu>,
    hosts_tag_filter: Option<String>,
    editor_advanced_expanded: bool,
    editor_telnet_added: bool,
    open_editor_menu: Option<EditorMenu>,
    sftp_local_path: std::path::PathBuf,
    sftp_show_host_picker: bool,
    sftp_local_filter_visible: bool,
    split_drop_target: Option<(u64, DropZone)>,
    divider_drag: Option<DividerDrag>,
    selected_command_palette_index: usize,
    tab_rename_workspace_id: Option<u64>,
    open_workspace_tab_menu: Option<u64>,
    /// Right-click context menu on a terminal pane: (pane id, click position).
    pane_context_menu: Option<(u64, Point<Pixels>)>,
    tab_rename_input: Entity<InputState>,
    pane_rename_id: Option<u64>,
    pane_rename_input: Entity<InputState>,
    pending_paste: Option<PendingPaste>,
    pending_snippet_prompts: Option<PendingSnippetPrompts>,
    sync_pull_force: bool,
    sync_pull_pending_warning: bool,
    settings_scroll: ScrollHandle,
    host_editor_scroll: ScrollHandle,
    hosts_list_scroll: ScrollHandle,
    tab_strip_scroll: ScrollHandle,
    tab_strip_scrolled_to: Option<u64>,
    launched_at: Instant,
    _window_bounds_subscription: Option<Subscription>,
    _window_bounds_save_task: Option<Task<()>>,
}

impl TermiRustApp {
    pub fn new(mut saved: SavedState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs = DraftInputs::new(window, cx);
        let shell_inputs = ShellInputs::new(window, cx);
        let snippet_inputs = SnippetInputs::new(window, cx);
        let settings_inputs = SettingsInputs::new(window, cx);
        let vault_inputs = VaultInputs::new(window, cx);
        let vault_member_inputs = VaultMemberInputs::new(window, cx);
        let (event_tx, event_rx) = mpsc::channel();
        let (sftp_event_tx, sftp_event_rx) = mpsc::channel();
        let known_hosts =
            Arc::new(KnownHostStore::load().expect("unable to initialize known host storage"));
        saved.merge_imported_identities(load_local_ssh_identities().unwrap_or_default());
        saved.ensure_vaults();
        theme::set_theme_preset(saved.settings.theme_preset);

        let draft_auth_mode = saved
            .selected_profile_id
            .as_ref()
            .and_then(|profile_id| {
                saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .map(|profile| profile.auth_mode)
            })
            .unwrap_or(AuthMode::Password);

        let imported_host_count = saved
            .profiles
            .iter()
            .filter(|profile| profile.source == ProfileSource::SshConfig)
            .count();
        let initial_status = if saved.identities.is_empty() && imported_host_count == 0 {
            "Choose a host or create a new entry.".to_string()
        } else {
            format!(
                "Imported {} hosts and {} identities from {}. Choose a host or create a new entry.",
                imported_host_count,
                saved.identities.len(),
                ssh_directory_label()
            )
        };

        let mut app = Self {
            selected_profile_id: saved.selected_profile_id.clone(),
            saved,
            inputs,
            shell_inputs,
            snippet_inputs,
            settings_inputs,
            vault_inputs,
            vault_member_inputs,
            draft_auth_mode,
            nav_section: NavSection::Hosts,
            show_editor_panel: false,
            event_tx,
            event_rx,
            sftp_event_tx,
            sftp_event_rx,
            panes: Vec::new(),
            workspaces: Vec::new(),
            active_workspace_id: None,
            selected_snippet_id: None,
            selected_host_ids: HashSet::new(),
            selected_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            selected_vault_member_id: None,
            next_session_id: 1,
            next_sftp_operation_id: 1,
            next_workspace_id: 1,
            status_message: initial_status,
            error_message: String::new(),
            draft_identity_id: None,
            draft_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            draft_profile_favorite: false,
            draft_start_in_files: false,
            draft_color_tag: None,
            draft_port_forward_rules: Vec::new(),
            draft_port_forward_kind: PortForwardKind::Local,
            snippet_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            snippet_pinned: false,
            draft_vault_member_role: VaultMemberRole::Editor,
            known_hosts,
            keychain_tab: KeychainTab::Keys,
            show_command_palette: false,
            show_new_host_menu: false,
            hosts_view_mode: HostsViewMode::Grid,
            hosts_sort: HostsSort::NewestFirst,
            open_toolbar_menu: None,
            hosts_tag_filter: None,
            editor_advanced_expanded: false,
            editor_telnet_added: false,
            open_editor_menu: None,
            sftp_local_path: std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/")),
            sftp_show_host_picker: false,
            sftp_local_filter_visible: false,
            split_drop_target: None,
            divider_drag: None,
            selected_command_palette_index: 0,
            tab_rename_workspace_id: None,
            open_workspace_tab_menu: None,
            pane_context_menu: None,
            tab_rename_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            pane_rename_id: None,
            pane_rename_input: cx.new(|cx| InputState::new(window, cx).placeholder("Pane name")),
            pending_paste: None,
            pending_snippet_prompts: None,
            sync_pull_force: false,
            sync_pull_pending_warning: false,
            settings_scroll: ScrollHandle::new(),
            host_editor_scroll: ScrollHandle::new(),
            hosts_list_scroll: ScrollHandle::new(),
            tab_strip_scroll: ScrollHandle::new(),
            tab_strip_scrolled_to: None,
            launched_at: Instant::now(),
            _window_bounds_subscription: None,
            _window_bounds_save_task: None,
        };

        app.load_settings_inputs(window, cx);

        if app.saved.settings.restore_workspaces_on_launch {
            app.restore_saved_workspaces(window, cx);
        }

        app.show_editor_panel = false;
        app.selected_profile_id = None;
        app.selected_host_ids.clear();

        let window_bounds_subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_terminal_layout(window, cx);
            this.persist_window_bounds(window, cx);
        });
        app._window_bounds_subscription = Some(window_bounds_subscription);

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;

                if cx
                    .update(|window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.process_events(cx);
                            app.process_pending_auto_reconnects(window, cx);
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // One-shot capture once the window has settled, so its frame and
        // display id are recorded even if the user never moves the window.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.persist_window_bounds(window, cx);
                });
            });
        })
        .detach();

        app
    }

    fn terminal_font_size(&self) -> f32 {
        self.saved.settings.terminal_font_size as f32
    }

    fn load_settings_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(
            &self.settings_inputs.local_shell_program,
            self.saved.settings.default_local_shell.program.clone(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.settings_inputs.local_shell_cwd,
            self.saved
                .settings
                .default_local_shell
                .cwd
                .clone()
                .unwrap_or_default(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.settings_inputs.default_ssh_startup_directory,
            self.saved
                .settings
                .default_ssh_startup_directory
                .clone()
                .unwrap_or_default(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.settings_inputs.terminal_font_family,
            self.saved
                .settings
                .terminal_font_family
                .clone()
                .unwrap_or_default(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.settings_inputs.sync_folder_input,
            self.saved
                .settings
                .sync_folder_path
                .clone()
                .unwrap_or_default(),
            window,
            cx,
        );
        self.clear_backup_inputs(window, cx);
    }

    fn current_profile_draft_raw(&self, cx: &App) -> anyhow::Result<DraftProfile> {
        let key_path = self.inputs.key_path.read(cx).value().to_string();
        let identity_id = self
            .draft_identity_id
            .clone()
            .filter(|identity_id| {
                self.identity_by_id(identity_id)
                    .is_some_and(|identity| identity.key_path == key_path)
            })
            .or_else(|| {
                self.identity_for_key_path(&key_path)
                    .map(|identity| identity.id.clone())
            });
        let jump_host_value = self.inputs.jump_host.read(cx).value().trim().to_string();
        let jump_host_id = self.resolve_jump_host_reference(&jump_host_value)?;

        let draft = DraftProfile {
            label: self.inputs.label.read(cx).value().to_string(),
            vault_id: self.draft_vault_id.clone(),
            favorite: self.draft_profile_favorite,
            group: self.inputs.group.read(cx).value().to_string(),
            tags: self.inputs.tags.read(cx).value().to_string(),
            host: self.inputs.host.read(cx).value().to_string(),
            port: self.inputs.port.read(cx).value().to_string(),
            username: self.inputs.username.read(cx).value().to_string(),
            password: self.inputs.password.read(cx).value().to_string(),
            key_path,
            identity_id,
            jump_host_id,
            startup_directory: self.inputs.startup_directory.read(cx).value().to_string(),
            startup_command: self.inputs.startup_command.read(cx).value().to_string(),
            start_in_files: self.draft_start_in_files,
            terminal_scrollback_rows: self
                .inputs
                .terminal_scrollback_rows
                .read(cx)
                .value()
                .to_string(),
            saved_port_forward_rules: self.draft_port_forward_rules.clone(),
            forward_kind: self.draft_port_forward_kind,
            forward_local_port: self.inputs.forward_local_port.read(cx).value().to_string(),
            forward_remote_host: self.inputs.forward_remote_host.read(cx).value().to_string(),
            forward_remote_port: self.inputs.forward_remote_port.read(cx).value().to_string(),
            key_passphrase: self.inputs.key_passphrase.read(cx).value().to_string(),
            password_credential_id: self.selected_profile_id.as_ref().and_then(|profile_id| {
                self.saved
                    .profiles
                    .iter()
                    .find(|item| &item.id == profile_id)
                    .and_then(|profile| profile.password_credential_id.clone())
            }),
            auth_mode: self.draft_auth_mode,
            description: self.inputs.description.read(cx).value().to_string(),
            color_tag: self.draft_color_tag,
            environment: self.inputs.environment.read(cx).value().to_string(),
        };

        Ok(draft)
    }

    fn current_profile_draft(&self, cx: &App) -> anyhow::Result<DraftProfile> {
        let draft = self.current_profile_draft_raw(cx)?;

        Ok(apply_group_defaults_to_draft(
            draft,
            self.host_group_by_label(&self.inputs.group.read(cx).value()),
            &self.saved.identities,
        ))
    }

    fn set_auth_mode(&mut self, auth_mode: AuthMode, cx: &mut Context<Self>) {
        self.draft_auth_mode = auth_mode;
        self.status_message = format!("Using {} authentication.", auth_mode.label());
        self.error_message.clear();
        cx.notify();
    }

    fn set_draft_connect_view(&mut self, start_in_files: bool, cx: &mut Context<Self>) {
        self.draft_start_in_files = start_in_files;
        self.status_message = if start_in_files {
            "This host will open in the remote Files view after connect.".to_string()
        } else {
            "This host will open in the terminal view after connect.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn toggle_draft_profile_favorite(&mut self, favorite: bool, cx: &mut Context<Self>) {
        self.draft_profile_favorite = favorite;
        self.status_message = if favorite {
            "This host will be starred in the library.".to_string()
        } else {
            "This host will appear in the regular library groups.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn save_group_defaults_from_draft(&mut self, cx: &mut Context<Self>) {
        let Ok(draft) = self.current_profile_draft_raw(cx) else {
            self.error_message = "Group defaults require a valid draft context.".to_string();
            cx.notify();
            return;
        };
        let label = draft.group.trim().to_string();
        if label.is_empty() {
            self.error_message = "Enter a group name first.".to_string();
            cx.notify();
            return;
        }
        let port_forward_rules = match draft.parse_port_forward_rules() {
            Ok(rules) => rules,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };

        self.saved.upsert_host_group(SavedHostGroup {
            label: label.clone(),
            vault_id: self.draft_vault_id.clone(),
            username: non_empty_string(&draft.username),
            tags: parse_tag_values(&draft.tags),
            identity_id: draft.identity_id,
            jump_host_id: draft.jump_host_id,
            startup_directory: non_empty_string(&draft.startup_directory),
            startup_command: non_empty_string(&draft.startup_command),
            port_forward_rules,
        });
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.status_message = format!("Saved defaults for group '{}'.", label);
        self.error_message.clear();
        cx.notify();
    }

    fn apply_group_defaults_to_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let label = self.inputs.group.read(cx).value().trim().to_string();
        let Some(group) = self.host_group_by_label(&label).cloned() else {
            self.error_message = "No saved defaults exist for this group yet.".to_string();
            cx.notify();
            return;
        };

        if let Some(identity_id) = group.identity_id.as_deref() {
            if let Some(identity) = self.identity_by_id(identity_id).cloned() {
                self.use_identity(&identity, window, cx);
            }
        }
        if let Some(username) = group.username.clone() {
            Self::set_input_value(&self.inputs.username, username, window, cx);
        }
        if let Some(jump_host_id) = group.jump_host_id.as_deref() {
            let jump_host = self
                .jump_host_display_name(jump_host_id)
                .unwrap_or_else(|| jump_host_id.to_string());
            Self::set_input_value(&self.inputs.jump_host, jump_host, window, cx);
        }
        if !group.tags.is_empty() {
            let merged_tags = merge_tag_values(
                &parse_tag_values(&self.inputs.tags.read(cx).value()),
                &group.tags,
            );
            Self::set_input_value(
                &self.inputs.tags,
                format_tag_values(&merged_tags),
                window,
                cx,
            );
        }
        if let Some(startup_directory) = group.startup_directory.clone() {
            Self::set_input_value(
                &self.inputs.startup_directory,
                startup_directory,
                window,
                cx,
            );
        }
        if let Some(startup_command) = group.startup_command.clone() {
            Self::set_input_value(&self.inputs.startup_command, startup_command, window, cx);
        }
        if let Some(vault_id) = group.vault_id.as_deref() {
            self.draft_vault_id = Some(vault_id.to_string());
        }
        if !group.port_forward_rules.is_empty() {
            self.draft_port_forward_rules =
                merge_port_forward_rules(&self.draft_port_forward_rules, &group.port_forward_rules);
        }

        self.status_message = format!("Loaded defaults for group '{}'.", group.display_name());
        self.error_message.clear();
        cx.notify();
    }

    fn remove_group_defaults(&mut self, cx: &mut Context<Self>) {
        let label = self.inputs.group.read(cx).value().trim().to_string();
        if label.is_empty() {
            self.error_message = "Enter a group name first.".to_string();
            cx.notify();
            return;
        }
        if !self.saved.remove_host_group(&label) {
            self.error_message = format!("No saved defaults exist for group '{}'.", label);
            cx.notify();
            return;
        }
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.status_message = format!("Removed defaults for group '{}'.", label);
        self.error_message.clear();
        cx.notify();
    }

    fn set_input_value(
        input: &Entity<InputState>,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        input.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
    }

    fn clear_backup_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(
            &self.settings_inputs.export_backup_passphrase,
            "",
            window,
            cx,
        );
        Self::set_input_value(&self.settings_inputs.export_backup_confirm, "", window, cx);
        Self::set_input_value(
            &self.settings_inputs.import_backup_passphrase,
            "",
            window,
            cx,
        );
    }

    fn preferred_identity(&self) -> Option<&SavedIdentity> {
        self.saved.identities.first()
    }

    fn host_group_by_label(&self, label: &str) -> Option<&SavedHostGroup> {
        let label = label.trim();
        if label.is_empty() {
            return None;
        }

        self.saved
            .host_groups
            .iter()
            .find(|group| group.label.eq_ignore_ascii_case(label))
    }

    fn group_host_counts(&self, group_name: &str, cx: &App) -> (usize, usize) {
        let visible = self
            .filtered_profiles(cx)
            .into_iter()
            .filter(|profile| Self::profile_group_name(profile) == group_name)
            .count();
        let total = self
            .saved
            .profiles
            .iter()
            .filter(|profile| Self::profile_group_name(profile) == group_name)
            .count();
        (visible, total)
    }

    fn filtered_profile_ids_for_group(&self, group_name: &str, cx: &App) -> Vec<String> {
        self.filtered_profiles(cx)
            .into_iter()
            .filter(|profile| Self::profile_group_name(profile) == group_name)
            .map(|profile| profile.id)
            .collect()
    }

    fn select_filtered_group_hosts(&mut self, group_name: &str, cx: &mut Context<Self>) {
        let ids = self.filtered_profile_ids_for_group(group_name, cx);
        if ids.is_empty() {
            self.error_message = format!("No visible hosts are in '{}'.", group_name);
            cx.notify();
            return;
        }
        self.selected_host_ids = ids.into_iter().collect();
        self.status_message = format!(
            "Selected {} host(s) from '{}'.",
            self.selected_host_ids.len(),
            group_name
        );
        self.error_message.clear();
        cx.notify();
    }

    fn prepare_bulk_group_assignment(
        &mut self,
        group_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_bulk_group_input(group_name.to_string(), window, cx);
        self.status_message = format!(
            "Bulk group target set to '{}'. Select hosts and apply when ready.",
            group_name
        );
        self.error_message.clear();
        cx.notify();
    }

    fn vault_by_id(&self, vault_id: &str) -> Option<&SavedVault> {
        self.saved.vaults.iter().find(|vault| vault.id == vault_id)
    }

    fn default_vault(&self) -> Option<&SavedVault> {
        self.vault_by_id(DEFAULT_VAULT_ID)
            .or_else(|| self.saved.vaults.first())
    }

    fn effective_vault_id(&self, vault_id: Option<&str>) -> String {
        vault_id
            .and_then(|id| self.vault_by_id(id))
            .map(|vault| vault.id.clone())
            .or_else(|| self.default_vault().map(|vault| vault.id.clone()))
            .unwrap_or_else(|| DEFAULT_VAULT_ID.to_string())
    }

    fn effective_vault_name(&self, vault_id: Option<&str>) -> String {
        self.vault_by_id(&self.effective_vault_id(vault_id))
            .map(SavedVault::display_name)
            .unwrap_or_else(|| "Personal".to_string())
    }

    fn vault_item_counts(&self, vault_id: &str) -> (usize, usize, usize) {
        let hosts = self
            .saved
            .profiles
            .iter()
            .filter(|profile| profile.effective_vault_id() == vault_id)
            .count();
        let identities = self
            .saved
            .identities
            .iter()
            .filter(|identity| identity.effective_vault_id() == vault_id)
            .count();
        let snippets = self
            .saved
            .snippets
            .iter()
            .filter(|snippet| snippet.effective_vault_id() == vault_id)
            .count();
        (hosts, identities, snippets)
    }

    fn current_key_path(&self, cx: &App) -> String {
        self.inputs.key_path.read(cx).value().trim().to_string()
    }

    fn identity_by_id(&self, identity_id: &str) -> Option<&SavedIdentity> {
        self.saved
            .identities
            .iter()
            .find(|identity| identity.id == identity_id)
    }

    fn identity_for_key_path(&self, key_path: &str) -> Option<&SavedIdentity> {
        self.saved
            .identities
            .iter()
            .find(|identity| identity.key_path == key_path)
    }

    fn resolve_jump_host_reference(&self, value: &str) -> anyhow::Result<Option<String>> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }

        let normalized = value.to_ascii_lowercase();
        let matched = self.saved.profiles.iter().find(|profile| {
            profile.id.eq_ignore_ascii_case(value)
                || profile.label.eq_ignore_ascii_case(value)
                || profile.display_name().to_ascii_lowercase() == normalized
                || profile.host.eq_ignore_ascii_case(value)
        });

        let Some(profile) = matched else {
            anyhow::bail!("Jump host '{value}' does not match a saved host");
        };

        if self.selected_profile_id.as_deref() == Some(profile.id.as_str()) {
            anyhow::bail!("A host cannot use itself as its jump host");
        }

        Ok(Some(profile.id.clone()))
    }

    fn jump_host_display_name(&self, jump_host_id: &str) -> Option<String> {
        self.saved
            .profiles
            .iter()
            .find(|profile| profile.id == jump_host_id)
            .map(HostProfile::display_name)
    }

    fn last_connected_at(&self, profile: &HostProfile) -> Option<u64> {
        self.saved
            .session_logs
            .iter()
            .filter(|log| {
                log.host == profile.host
                    && log.port == profile.port
                    && log.username == profile.username
                    && log.started_at > 0
            })
            .map(|log| log.started_at)
            .max()
    }

    fn resolve_jump_host_connection(
        &self,
        jump_host_id: &str,
    ) -> anyhow::Result<JumpHostConnection> {
        let mut visited = HashSet::new();
        if let Some(profile_id) = self.selected_profile_id.as_ref() {
            visited.insert(profile_id.clone());
        }
        self.resolve_jump_host_connection_recursive(jump_host_id, &mut visited)
    }

    fn resolve_jump_host_connection_recursive(
        &self,
        jump_host_id: &str,
        visited: &mut HashSet<String>,
    ) -> anyhow::Result<JumpHostConnection> {
        if !visited.insert(jump_host_id.to_string()) {
            anyhow::bail!("Jump host chain contains a cycle");
        }

        let profile = self
            .saved
            .profiles
            .iter()
            .find(|profile| profile.id == jump_host_id)
            .ok_or_else(|| anyhow::anyhow!("Jump host is no longer available"))?;

        let auth = match profile.auth_mode {
            AuthMode::Password => {
                let Some(credential_id) = profile.password_credential_id.clone() else {
                    anyhow::bail!(
                        "Jump host '{}' needs a saved password in the system credential store",
                        profile.display_name()
                    );
                };
                AuthConfig::PasswordRef { credential_id }
            }
            AuthMode::PrivateKey => AuthConfig::PrivateKey {
                key_path: profile.key_path.clone(),
                passphrase: None,
            },
        };

        let nested_jump_host = profile
            .jump_host_id
            .as_deref()
            .map(|nested_id| self.resolve_jump_host_connection_recursive(nested_id, visited))
            .transpose()?
            .map(Box::new);

        Ok(JumpHostConnection {
            title: profile.display_name(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth,
            jump_host: nested_jump_host,
        })
    }

    fn current_quick_connect_password(&self, cx: &App) -> String {
        self.shell_inputs
            .quick_connect_password
            .read(cx)
            .value()
            .to_string()
    }

    fn current_bulk_group_input(&self, cx: &App) -> String {
        self.shell_inputs.bulk_group.read(cx).value().to_string()
    }

    fn set_quick_connect_password_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.quick_connect_password, value, window, cx);
    }

    fn set_bulk_group_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.bulk_group, value, window, cx);
    }

    fn selected_profile_mut(&mut self) -> Option<&mut HostProfile> {
        let selected_profile_id = self.selected_profile_id.as_ref()?;
        self.saved
            .profiles
            .iter_mut()
            .find(|item| &item.id == selected_profile_id)
    }

    fn persist_password_to_keychain(
        &mut self,
        credential_id: &str,
        password: &str,
    ) -> anyhow::Result<()> {
        credentials::store_password(credential_id, password)?;
        Ok(())
    }

    fn draft_password_credential_id(&self, draft: &DraftProfile) -> anyhow::Result<String> {
        if let Some(profile_id) = self.selected_profile_id.as_ref() {
            return Ok(credentials::profile_password_credential_id(profile_id));
        }

        let preview = draft.to_profile("preview".to_string())?;
        Ok(credentials::connection_password_credential_id(
            &preview.username,
            &preview.host,
            preview.port,
        ))
    }

    fn ensure_default_identity_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.draft_auth_mode != AuthMode::PrivateKey {
            return false;
        }

        if !self.current_key_path(cx).is_empty() {
            return false;
        }

        let Some(identity) = self.preferred_identity().cloned() else {
            return false;
        };

        self.draft_identity_id = Some(identity.id.clone());
        Self::set_input_value(&self.inputs.key_path, identity.key_path.clone(), window, cx);
        self.status_message = format!("Using identity '{}'.", identity.label);
        self.error_message.clear();
        true
    }

    fn use_identity(
        &mut self,
        identity: &SavedIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nav_section = NavSection::Hosts;
        self.show_editor_panel = true;
        self.draft_auth_mode = AuthMode::PrivateKey;
        self.draft_identity_id = Some(identity.id.clone());
        Self::set_input_value(&self.inputs.key_path, identity.key_path.clone(), window, cx);
        self.status_message = format!("Identity '{}' selected.", identity.label);
        self.error_message.clear();
        cx.notify();
    }

    fn load_profile_into_inputs(
        &mut self,
        profile_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self
            .saved
            .profiles
            .iter()
            .find(|item| item.id == profile_id)
        else {
            return;
        };

        let draft = DraftProfile::from_profile(profile);
        Self::set_input_value(&self.inputs.label, draft.label, window, cx);
        Self::set_input_value(&self.inputs.group, draft.group, window, cx);
        Self::set_input_value(&self.inputs.tags, draft.tags, window, cx);
        Self::set_input_value(
            &self.inputs.jump_host,
            draft
                .jump_host_id
                .as_deref()
                .and_then(|jump_host_id| self.jump_host_display_name(jump_host_id))
                .unwrap_or_default(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.inputs.startup_directory,
            draft.startup_directory,
            window,
            cx,
        );
        Self::set_input_value(
            &self.inputs.startup_command,
            draft.startup_command,
            window,
            cx,
        );
        Self::set_input_value(
            &self.inputs.terminal_scrollback_rows,
            draft.terminal_scrollback_rows,
            window,
            cx,
        );
        Self::set_input_value(&self.inputs.host, draft.host, window, cx);
        Self::set_input_value(&self.inputs.port, draft.port, window, cx);
        Self::set_input_value(&self.inputs.username, draft.username, window, cx);
        Self::set_input_value(&self.inputs.password, "", window, cx);
        Self::set_input_value(&self.inputs.key_path, draft.key_path, window, cx);
        Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
        Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
        Self::set_input_value(&self.inputs.description, draft.description, window, cx);
        Self::set_input_value(&self.inputs.environment, draft.environment, window, cx);
        self.draft_color_tag = draft.color_tag;
        self.draft_vault_id = Some(self.effective_vault_id(draft.vault_id.as_deref()));
        self.draft_profile_favorite = draft.favorite;
        self.draft_start_in_files = draft.start_in_files;
        self.draft_port_forward_rules = draft.saved_port_forward_rules;
        self.draft_port_forward_kind = PortForwardKind::Local;
        self.draft_identity_id = draft.identity_id.or_else(|| {
            self.identity_for_key_path(profile.key_path.as_str())
                .map(|identity| identity.id.clone())
        });

        self.selected_profile_id = Some(profile.id.clone());
        self.saved.selected_profile_id = Some(profile.id.clone());
        self.draft_auth_mode = profile.auth_mode;
        self.show_editor_panel = true;
        self.nav_section = NavSection::Hosts;
        self.status_message = if profile.auth_mode == AuthMode::Password
            && profile.password_credential_id.is_some()
        {
            format!(
                "Loaded host '{}'. Password is available from the system credential store.",
                profile.display_name()
            )
        } else {
            format!("Loaded host '{}'.", profile.display_name())
        };
        self.error_message.clear();
        cx.notify();
    }

    fn clear_profile_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.inputs.label, "", window, cx);
        Self::set_input_value(&self.inputs.group, "", window, cx);
        Self::set_input_value(&self.inputs.tags, "", window, cx);
        Self::set_input_value(&self.inputs.jump_host, "", window, cx);
        Self::set_input_value(&self.inputs.startup_directory, "", window, cx);
        Self::set_input_value(&self.inputs.startup_command, "", window, cx);
        Self::set_input_value(&self.inputs.terminal_scrollback_rows, "10000", window, cx);
        Self::set_input_value(&self.inputs.host, "", window, cx);
        Self::set_input_value(&self.inputs.port, "22", window, cx);
        Self::set_input_value(&self.inputs.username, "", window, cx);
        Self::set_input_value(&self.inputs.password, "", window, cx);
        Self::set_input_value(&self.inputs.key_path, "", window, cx);
        Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
        Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
        Self::set_input_value(&self.inputs.description, "", window, cx);
        Self::set_input_value(&self.inputs.environment, "", window, cx);
        self.draft_color_tag = None;
        self.draft_vault_id = Some(self.effective_vault_id(self.selected_vault_id.as_deref()));
        self.draft_profile_favorite = false;
        self.draft_start_in_files = false;
        self.draft_port_forward_rules.clear();
        self.draft_port_forward_kind = PortForwardKind::Local;
        let first_identity = self.saved.identities.first().cloned();
        if let Some(identity) = &first_identity {
            self.draft_identity_id = Some(identity.id.clone());
            Self::set_input_value(&self.inputs.key_path, identity.key_path.clone(), window, cx);
            self.draft_auth_mode = AuthMode::PrivateKey;
        } else {
            self.draft_identity_id = None;
            self.draft_auth_mode = AuthMode::Password;
        }
        self.selected_profile_id = None;
        self.saved.selected_profile_id = None;
        self.show_editor_panel = true;
        self.status_message = "Draft cleared. Define a host to save or connect.".into();
        self.error_message.clear();
        cx.notify();
    }

    fn set_draft_port_forward_kind(&mut self, kind: PortForwardKind, cx: &mut Context<Self>) {
        self.draft_port_forward_kind = kind;
        self.error_message.clear();
        self.status_message = format!("Forward rule type set to {}.", kind.label());
        cx.notify();
    }

    fn add_draft_port_forward_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = match self.current_profile_draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };

        match draft.parse_pending_port_forward_rule() {
            Ok(Some(rule)) => {
                if self
                    .draft_port_forward_rules
                    .iter()
                    .any(|existing| existing == &rule)
                {
                    self.error_message =
                        format!("Forward rule '{}' already exists.", rule.display_name());
                    cx.notify();
                    return;
                }

                let label = rule.display_name();
                self.draft_port_forward_rules.push(rule);
                Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
                Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
                Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
                self.status_message = format!("Added forward rule {label}.");
                self.error_message.clear();
                cx.notify();
            }
            Ok(None) => {
                self.error_message =
                    "Enter local port, remote host, and remote port to add a rule.".to_string();
                cx.notify();
            }
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
            }
        }
    }

    fn remove_draft_port_forward_rule(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.draft_port_forward_rules.len() {
            return;
        }

        let removed = self.draft_port_forward_rules.remove(index);
        self.status_message = format!("Removed forward rule {}.", removed.display_name());
        self.error_message.clear();
        if self.draft_port_forward_rules.is_empty() {
            Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
            Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
            Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
        }
        cx.notify();
    }

    fn current_snippet_draft(&self, cx: &App) -> SavedSnippet {
        SavedSnippet {
            id: self
                .selected_snippet_id
                .clone()
                .unwrap_or_else(SavedSnippet::snippet_id),
            label: self.snippet_inputs.label.read(cx).value().to_string(),
            vault_id: Some(self.effective_vault_id(self.snippet_vault_id.as_deref())),
            group: self.snippet_inputs.group.read(cx).value().to_string(),
            pinned: self.snippet_pinned,
            command: self.snippet_inputs.command.read(cx).value().to_string(),
        }
    }

    fn load_snippet_into_inputs(
        &mut self,
        snippet_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snippet) = self
            .saved
            .snippets
            .iter()
            .find(|item| item.id == snippet_id)
        else {
            return;
        };

        Self::set_input_value(
            &self.snippet_inputs.label,
            snippet.label.clone(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.snippet_inputs.group,
            snippet.group.clone(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.snippet_inputs.command,
            snippet.command.clone(),
            window,
            cx,
        );
        self.snippet_vault_id = Some(self.effective_vault_id(snippet.vault_id.as_deref()));
        self.snippet_pinned = snippet.pinned;
        self.selected_snippet_id = Some(snippet.id.clone());
        self.nav_section = NavSection::Snippets;
        self.status_message = format!("Loaded snippet '{}'.", snippet.display_name());
        self.error_message.clear();
        cx.notify();
    }

    fn clear_snippet_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.snippet_inputs.label, "", window, cx);
        Self::set_input_value(&self.snippet_inputs.group, "", window, cx);
        Self::set_input_value(&self.snippet_inputs.command, "", window, cx);
        self.snippet_vault_id = Some(self.effective_vault_id(self.selected_vault_id.as_deref()));
        self.snippet_pinned = false;
        self.selected_snippet_id = None;
        self.nav_section = NavSection::Snippets;
        self.status_message = "Snippet draft cleared.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn toggle_snippet_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.snippet_pinned = pinned;
        self.status_message = if pinned {
            "Snippet will appear in pinned command quick actions.".to_string()
        } else {
            "Snippet removed from pinned command quick actions.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn set_saved_snippet_pinned(
        &mut self,
        snippet_id: &str,
        pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snippet) = self
            .saved
            .snippets
            .iter_mut()
            .find(|item| item.id == snippet_id)
        else {
            return;
        };

        snippet.pinned = pinned;
        self.saved.snippets.sort_by(|left, right| {
            right.pinned.cmp(&left.pinned).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }
        if self.selected_snippet_id.as_deref() == Some(snippet_id) {
            self.snippet_pinned = pinned;
            self.load_snippet_into_inputs(snippet_id, window, cx);
            return;
        }
        self.status_message = if pinned {
            "Snippet pinned to workspace quick actions.".to_string()
        } else {
            "Snippet removed from workspace quick actions.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn save_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = self.current_snippet_draft(cx);
        if snippet.command.trim().is_empty() {
            self.error_message = "Snippet command is required.".to_string();
            cx.notify();
            return;
        }

        self.saved.upsert_snippet(snippet.clone());
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.selected_snippet_id = Some(snippet.id.clone());
        if snippet.label.trim().is_empty() {
            Self::set_input_value(
                &self.snippet_inputs.label,
                snippet.display_name(),
                window,
                cx,
            );
        }
        self.status_message = format!("Saved snippet '{}'.", snippet.display_name());
        self.error_message.clear();
        cx.notify();
    }

    fn remove_selected_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snippet_id) = self.selected_snippet_id.clone() else {
            return;
        };

        self.saved.remove_snippet(&snippet_id);
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.clear_snippet_form(window, cx);
        self.status_message = "Snippet removed.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn current_vault_draft(&self, _cx: &App) -> SavedVault {
        let selected_id = self
            .selected_vault_id
            .clone()
            .filter(|vault_id| vault_id != DEFAULT_VAULT_ID);
        let current_kind = selected_id
            .as_deref()
            .and_then(|vault_id| self.vault_by_id(vault_id))
            .map(|vault| vault.kind)
            .unwrap_or(VaultKind::Shared);

        SavedVault {
            id: selected_id.unwrap_or_else(SavedVault::vault_id),
            label: self.vault_inputs.label.read(_cx).value().to_string(),
            description: self.vault_inputs.description.read(_cx).value().to_string(),
            kind: current_kind,
            members: self
                .selected_vault_id
                .as_deref()
                .and_then(|vault_id| self.vault_by_id(vault_id))
                .map(|vault| vault.members.clone())
                .unwrap_or_default(),
        }
    }

    fn current_vault_member_draft(&self, cx: &App) -> Option<SavedVaultMember> {
        let name = self
            .vault_member_inputs
            .name
            .read(cx)
            .value()
            .trim()
            .to_string();
        let email = self
            .vault_member_inputs
            .email
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty() && email.is_empty() {
            return None;
        }

        Some(SavedVaultMember {
            id: self
                .selected_vault_member_id
                .clone()
                .unwrap_or_else(SavedVaultMember::member_id),
            name,
            email,
            role: self.draft_vault_member_role,
        })
    }

    fn load_vault_into_inputs(
        &mut self,
        vault_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(vault) = self.saved.vaults.iter().find(|item| item.id == vault_id) else {
            return;
        };

        Self::set_input_value(&self.vault_inputs.label, vault.label.clone(), window, cx);
        Self::set_input_value(
            &self.vault_inputs.description,
            vault.description.clone(),
            window,
            cx,
        );
        self.selected_vault_id = Some(vault.id.clone());
        self.selected_vault_member_id = None;
        self.draft_vault_id = Some(vault.id.clone());
        self.snippet_vault_id = Some(vault.id.clone());
        Self::set_input_value(&self.vault_member_inputs.name, "", window, cx);
        Self::set_input_value(&self.vault_member_inputs.email, "", window, cx);
        self.draft_vault_member_role = VaultMemberRole::Editor;
        self.nav_section = NavSection::Vaults;
        self.status_message = format!("Loaded vault '{}'.", vault.display_name());
        self.error_message.clear();
        cx.notify();
    }

    fn clear_vault_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.vault_inputs.label, "", window, cx);
        Self::set_input_value(&self.vault_inputs.description, "", window, cx);
        Self::set_input_value(&self.vault_member_inputs.name, "", window, cx);
        Self::set_input_value(&self.vault_member_inputs.email, "", window, cx);
        self.selected_vault_id = Some(DEFAULT_VAULT_ID.to_string());
        self.selected_vault_member_id = None;
        self.draft_vault_id = Some(DEFAULT_VAULT_ID.to_string());
        self.snippet_vault_id = Some(DEFAULT_VAULT_ID.to_string());
        self.draft_vault_member_role = VaultMemberRole::Editor;
        self.nav_section = NavSection::Vaults;
        self.status_message = "Vault draft cleared.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn save_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut vault = self.current_vault_draft(cx);
        if vault.label.trim().is_empty() {
            self.error_message = "Vault name is required.".to_string();
            cx.notify();
            return;
        }
        if self.saved.vaults.iter().any(|existing| {
            existing.id != vault.id && existing.label.eq_ignore_ascii_case(&vault.label)
        }) {
            self.error_message = format!("Vault '{}' already exists.", vault.label.trim());
            cx.notify();
            return;
        }
        vault.label = vault.label.trim().to_string();
        vault.description = vault.description.trim().to_string();
        self.saved.upsert_vault(vault.clone());
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.selected_vault_id = Some(vault.id.clone());
        self.draft_vault_id = Some(vault.id.clone());
        self.snippet_vault_id = Some(vault.id.clone());
        self.status_message = format!("Saved vault '{}'.", vault.display_name());
        self.error_message.clear();
        self.nav_section = NavSection::Vaults;
        if vault.kind == VaultKind::Personal {
            Self::set_input_value(&self.vault_inputs.label, vault.label, window, cx);
        }
        cx.notify();
    }

    fn remove_selected_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vault_id) = self.selected_vault_id.clone() else {
            return;
        };
        if vault_id == DEFAULT_VAULT_ID {
            self.error_message = "The personal vault cannot be deleted.".to_string();
            cx.notify();
            return;
        }
        if !self.saved.remove_vault(&vault_id) {
            return;
        }
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.clear_vault_form(window, cx);
        self.status_message = "Vault removed. Its items were moved to Personal.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn load_vault_member_into_inputs(
        &mut self,
        member_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(vault_id) = self.selected_vault_id.clone() else {
            return;
        };
        let Some(member) = self
            .saved
            .vaults
            .iter()
            .find(|vault| vault.id == vault_id)
            .and_then(|vault| vault.members.iter().find(|member| member.id == member_id))
            .cloned()
        else {
            return;
        };

        Self::set_input_value(&self.vault_member_inputs.name, member.name, window, cx);
        Self::set_input_value(&self.vault_member_inputs.email, member.email, window, cx);
        self.selected_vault_member_id = Some(member.id);
        self.draft_vault_member_role = member.role;
        self.status_message = "Loaded vault member.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn clear_vault_member_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.vault_member_inputs.name, "", window, cx);
        Self::set_input_value(&self.vault_member_inputs.email, "", window, cx);
        self.selected_vault_member_id = None;
        self.draft_vault_member_role = VaultMemberRole::Editor;
        self.error_message.clear();
        cx.notify();
    }

    fn save_vault_member(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vault_id) = self.selected_vault_id.clone() else {
            return;
        };
        let Some(member) = self.current_vault_member_draft(cx) else {
            self.error_message = "Member name or email is required.".to_string();
            cx.notify();
            return;
        };
        if member.email.trim().is_empty() {
            self.error_message = "Member email is required.".to_string();
            cx.notify();
            return;
        }

        let Some(vault) = self
            .saved
            .vaults
            .iter_mut()
            .find(|vault| vault.id == vault_id)
        else {
            return;
        };
        if vault.is_personal() {
            self.error_message = "The personal vault does not support shared members.".to_string();
            cx.notify();
            return;
        }

        let member_name = member.display_name();
        vault.upsert_member(member);
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.clear_vault_member_form(window, cx);
        self.status_message = format!("Saved vault member '{}'.", member_name);
        self.error_message.clear();
        cx.notify();
    }

    fn remove_vault_member(
        &mut self,
        member_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(vault_id) = self.selected_vault_id.clone() else {
            return;
        };
        let Some(vault) = self
            .saved
            .vaults
            .iter_mut()
            .find(|vault| vault.id == vault_id)
        else {
            return;
        };
        if vault.is_personal() {
            self.error_message = "The personal vault member cannot be removed.".to_string();
            cx.notify();
            return;
        }
        if !vault.remove_member(member_id) {
            return;
        }
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.clear_vault_member_form(window, cx);
        self.status_message = "Vault member removed.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn run_command_in_active_pane(
        &mut self,
        command: &str,
        success_message: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            self.error_message = "Open a terminal session to run a command.".to_string();
            cx.notify();
            return false;
        };

        let mut bytes = command.as_bytes().to_vec();
        if !command.ends_with('\n') {
            bytes.push(b'\n');
        }

        let broadcasting = self
            .workspace_id_for_pane(pane_id)
            .and_then(|workspace_id| self.workspace(workspace_id))
            .is_some_and(|workspace| workspace.broadcast_input && workspace.pane_ids.len() > 1);

        if self.send_input_bytes_broadcast(pane_id, bytes, cx) {
            self.status_message = if broadcasting {
                format!(
                    "{} (broadcast across panes)",
                    success_message.trim_end_matches('.')
                )
            } else {
                success_message.to_string()
            };
            self.error_message.clear();
            cx.notify();
            return true;
        }
        false
    }

    fn run_snippet_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let prompts = extract_snippet_prompt_names(command);
        if !prompts.is_empty() {
            if self.active_pane().is_none() {
                self.error_message =
                    "Open a terminal session before running a snippet with prompts.".to_string();
                cx.notify();
                return;
            }
            let fields: Vec<SnippetPromptField> = prompts
                .iter()
                .map(|name| {
                    let placeholder = format!("Value for {name}");
                    SnippetPromptField {
                        name: name.clone(),
                        input: cx
                            .new(|cx| InputState::new(window, cx).placeholder(placeholder.clone())),
                    }
                })
                .collect();
            self.pending_snippet_prompts = Some(PendingSnippetPrompts {
                command: command.to_string(),
                fields,
            });
            self.status_message = format!(
                "Snippet needs {} input(s). Fill the panel and Run.",
                prompts.len()
            );
            self.error_message.clear();
            if let Some(prompts) = self.pending_snippet_prompts.as_ref() {
                if let Some(first) = prompts.fields.first() {
                    first.input.read(cx).focus_handle(cx).focus(window);
                }
            }
            cx.notify();
            return;
        }
        self.run_resolved_snippet(command.to_string(), cx);
    }

    fn run_resolved_snippet(&mut self, command: String, cx: &mut Context<Self>) {
        let resolved = self
            .active_pane()
            .map(|pane| substitute_snippet_placeholders(&command, &pane.request))
            .unwrap_or_else(|| command.clone());
        let _ =
            self.run_command_in_active_pane(&resolved, "Snippet sent to the active session.", cx);
    }

    fn confirm_snippet_prompts(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_snippet_prompts.take() else {
            return;
        };
        let values: Vec<(String, String)> = pending
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.input.read(cx).value().to_string()))
            .collect();
        let resolved_prompts = substitute_snippet_prompts(&pending.command, &values);
        self.run_resolved_snippet(resolved_prompts, cx);
    }

    fn cancel_snippet_prompts(&mut self, cx: &mut Context<Self>) {
        if self.pending_snippet_prompts.take().is_some() {
            self.status_message = "Snippet prompts cancelled.".to_string();
            self.error_message.clear();
            cx.notify();
        }
    }

    fn send_startup_actions(&mut self, session_id: u64) -> bool {
        let Some((command_tx, startup_bytes)) = self.pane(session_id).and_then(|pane| {
            startup_bytes_for_request(
                &pane.request,
                self.saved.settings.default_ssh_startup_directory.as_deref(),
            )
            .map(|bytes| (pane.runtime.command_tx.clone(), bytes))
        }) else {
            return false;
        };

        if command_tx
            .send(SessionCommand::Input(startup_bytes))
            .is_ok()
        {
            if let Some(pane) = self.pane_mut(session_id) {
                pane.current_input.clear();
                pane.selected_autocomplete_index = None;
            }
            return true;
        }

        false
    }

    fn toggle_host_batch_selection(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        if !self.selected_host_ids.insert(profile_id.to_string()) {
            self.selected_host_ids.remove(profile_id);
        }
        self.status_message = if self.selected_host_ids.is_empty() {
            "Cleared host batch selection.".to_string()
        } else {
            format!(
                "Selected {} host(s) for batch actions.",
                self.selected_host_ids.len()
            )
        };
        self.error_message.clear();
        cx.notify();
    }

    fn clear_host_batch_selection(&mut self, cx: &mut Context<Self>) {
        self.selected_host_ids.clear();
        self.status_message = "Cleared host batch selection.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn select_all_filtered_hosts(&mut self, cx: &mut Context<Self>) {
        let ids = self.filtered_profile_ids(cx);
        if ids.is_empty() {
            self.error_message = "No hosts match the current filter.".to_string();
            cx.notify();
            return;
        }
        self.selected_host_ids = ids.into_iter().collect();
        self.status_message = format!(
            "Selected {} host(s) from the current filter.",
            self.selected_host_ids.len()
        );
        self.error_message.clear();
        cx.notify();
    }

    fn set_profile_favorite(
        &mut self,
        profile_id: &str,
        favorite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile_label = {
            let Some(profile) = self
                .saved
                .profiles
                .iter_mut()
                .find(|profile| profile.id == profile_id)
            else {
                return;
            };

            profile.favorite = favorite;
            if profile.source == ProfileSource::SshConfig {
                profile.source = ProfileSource::User;
            }
            profile.display_name()
        };
        self.saved.profiles.sort_by(|left, right| {
            right.favorite.cmp(&left.favorite).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });

        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        if self.selected_profile_id.as_deref() == Some(profile_id) {
            self.load_profile_into_inputs(profile_id, window, cx);
            self.status_message = if favorite {
                "Host starred.".to_string()
            } else {
                "Host removed from favorites.".to_string()
            };
            self.error_message.clear();
            cx.notify();
            return;
        }

        self.status_message = if favorite {
            format!("Starred '{}'.", profile_label)
        } else {
            format!("Removed '{}' from starred hosts.", profile_label)
        };
        self.error_message.clear();
        cx.notify();
    }

    fn bulk_set_selected_hosts_favorite(
        &mut self,
        favorite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_host_ids.is_empty() {
            self.error_message = "Select at least one host first.".to_string();
            cx.notify();
            return;
        }

        let selected = self.selected_host_ids.clone();
        let mut updated = 0usize;
        let mut promoted = 0usize;
        for profile in &mut self.saved.profiles {
            if selected.contains(&profile.id) {
                profile.favorite = favorite;
                if profile.source == ProfileSource::SshConfig {
                    profile.source = ProfileSource::User;
                    promoted += 1;
                }
                updated += 1;
            }
        }
        self.saved.profiles.sort_by(|left, right| {
            right.favorite.cmp(&left.favorite).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });

        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        if self
            .selected_profile_id
            .as_ref()
            .is_some_and(|profile_id| selected.contains(profile_id))
        {
            if let Some(profile_id) = self.selected_profile_id.clone() {
                self.load_profile_into_inputs(&profile_id, window, cx);
            }
        }

        self.status_message = if promoted > 0 {
            format!(
                "{} host(s) updated. {} imported host(s) were saved as local copies.",
                updated, promoted
            )
        } else if favorite {
            format!("Starred {} selected host(s).", updated)
        } else {
            format!("Removed {} selected host(s) from favorites.", updated)
        };
        self.error_message.clear();
        cx.notify();
    }

    fn bulk_assign_selected_hosts_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_host_ids.is_empty() {
            self.error_message = "Select at least one host first.".to_string();
            cx.notify();
            return;
        }

        let group = self.current_bulk_group_input(cx).trim().to_string();
        let selected = self.selected_host_ids.clone();
        let mut updated = 0usize;
        let mut promoted = 0usize;
        for profile in &mut self.saved.profiles {
            if selected.contains(&profile.id) {
                profile.group = group.clone();
                if profile.source == ProfileSource::SshConfig {
                    profile.source = ProfileSource::User;
                    promoted += 1;
                }
                updated += 1;
            }
        }

        self.saved.profiles.sort_by(|left, right| {
            right.favorite.cmp(&left.favorite).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });

        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        if self
            .selected_profile_id
            .as_ref()
            .is_some_and(|profile_id| selected.contains(profile_id))
        {
            if let Some(profile_id) = self.selected_profile_id.clone() {
                self.load_profile_into_inputs(&profile_id, window, cx);
            }
        }

        self.status_message = if promoted > 0 {
            format!(
                "{} host(s) regrouped. {} imported host(s) were saved as local copies.",
                updated, promoted
            )
        } else if group.is_empty() {
            format!("Cleared the group for {} selected host(s).", updated)
        } else {
            format!("Assigned '{}' to {} selected host(s).", group, updated)
        };
        self.error_message.clear();
        self.set_bulk_group_input("", window, cx);
    }

    fn pinned_snippet_quick_actions(&self) -> Vec<SavedSnippet> {
        self.saved
            .snippets
            .iter()
            .filter(|snippet| snippet.pinned)
            .take(6)
            .cloned()
            .collect()
    }

    fn pick_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status_message = "Choose a private key file.".to_string();
        self.error_message.clear();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = AsyncFileDialog::new()
                .set_title("Choose private key file")
                .pick_file()
                .await
                .map(|file| file.path().to_path_buf())
            else {
                return;
            };

            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.import_key_file(path, window, cx);
                });
            });
        })
        .detach();
    }

    fn import_key_file(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match inspect_identity_file(&path) {
            Ok(Some(imported)) => {
                let mut identity = imported.into_saved();
                identity.vault_id =
                    Some(self.effective_vault_id(self.selected_vault_id.as_deref()));
                let label = identity.label.clone();
                self.saved.upsert_identity(identity.clone());
                if let Err(error) = save_saved_state(&self.saved) {
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
                self.use_identity(&identity, window, cx);
                self.status_message = format!("Identity '{}' added.", label);
                self.error_message.clear();
                cx.notify();
            }
            Ok(None) => {
                self.error_message =
                    "That file does not look like a supported private key.".to_string();
                cx.notify();
            }
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
            }
        }
    }

    fn save_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.ensure_default_identity_selected(window, cx);
        let mut draft = match self.current_profile_draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        let profile_source = self
            .selected_profile_id
            .as_ref()
            .and_then(|profile_id| {
                self.saved
                    .profiles
                    .iter()
                    .find(|item| &item.id == profile_id)
                    .map(|profile| profile.source)
            })
            .unwrap_or(ProfileSource::User);
        let profile_id = self
            .selected_profile_id
            .clone()
            .unwrap_or_else(DraftProfile::profile_id);
        let existing_password_credential_id =
            self.selected_profile_id.as_ref().and_then(|profile_id| {
                self.saved
                    .profiles
                    .iter()
                    .find(|item| &item.id == profile_id)
                    .and_then(|profile| profile.password_credential_id.clone())
            });

        if draft.auth_mode == AuthMode::Password {
            let password = draft.password.trim().to_string();
            if !password.is_empty() {
                let credential_id = credentials::profile_password_credential_id(&profile_id);
                if let Err(error) = self.persist_password_to_keychain(&credential_id, &password) {
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
                draft.password_credential_id = Some(credential_id);
            } else {
                draft.password_credential_id = existing_password_credential_id.clone();
            }
        } else {
            if let Some(credential_id) = existing_password_credential_id.as_deref() {
                if let Err(error) = credentials::delete_password(credential_id) {
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }
            }
            draft.password_credential_id = None;
        }

        match draft.to_profile(profile_id.clone()) {
            Ok(mut profile) => {
                profile.source = ProfileSource::User;
                self.saved.upsert_profile(profile.clone());
                if let Err(error) = save_saved_state(&self.saved) {
                    self.error_message = error.to_string();
                    cx.notify();
                    return;
                }

                self.mark_onboarding_complete();
                self.selected_profile_id = Some(profile_id);
                self.saved.selected_profile_id = self.selected_profile_id.clone();
                Self::set_input_value(&self.inputs.password, "", window, cx);
                Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
                self.show_editor_panel = true;
                self.status_message = if profile.auth_mode == AuthMode::Password
                    && profile.password_credential_id.is_some()
                {
                    if profile_source == ProfileSource::SshConfig {
                        format!(
                            "Saved local copy of imported host '{}'. Password stored in the system credential store.",
                            profile.display_name()
                        )
                    } else {
                        format!(
                            "Saved '{}'. Password stored in the system credential store.",
                            profile.display_name()
                        )
                    }
                } else if profile_source == ProfileSource::SshConfig {
                    format!(
                        "Saved local copy of imported host '{}'.",
                        profile.display_name()
                    )
                } else {
                    format!("Saved '{}'.", profile.display_name())
                };
                self.error_message.clear();
                cx.notify();
            }
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
            }
        }
    }

    fn remove_selected_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile_id) = self.selected_profile_id.clone() else {
            return;
        };
        self.selected_host_ids.remove(&profile_id);
        let credential_id = self
            .saved
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .and_then(|profile| profile.password_credential_id.clone());
        if self
            .saved
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .is_some_and(|profile| profile.source == ProfileSource::SshConfig)
        {
            self.error_message.clear();
            self.status_message = format!(
                "Imported SSH config hosts are read from {}. Edit the config or save a local copy instead.",
                ssh_config_path_label()
            );
            cx.notify();
            return;
        }

        if let Some(credential_id) = credential_id.as_deref() {
            if let Err(error) = credentials::delete_password(credential_id) {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        }

        self.saved.remove_profile(&profile_id);
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
            cx.notify();
            return;
        }

        self.selected_profile_id = None;
        self.show_editor_panel = false;
        self.clear_profile_form(window, cx);
        self.status_message = "Saved host removed.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn host_search_query(&self, cx: &App) -> String {
        self.shell_inputs
            .host_search
            .read(cx)
            .value()
            .trim()
            .to_string()
    }

    fn filtered_profiles(&self, cx: &App) -> Vec<HostProfile> {
        let query = self.host_search_query(cx).to_ascii_lowercase();
        let mut profiles = self.saved.profiles.clone();
        profiles.sort_by(|left, right| {
            right.favorite.cmp(&left.favorite).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });

        if let Some(tag) = &self.hosts_tag_filter {
            profiles.retain(|p| p.tags.iter().any(|t| t == tag));
        }

        if query.is_empty() {
            return profiles;
        }

        profiles
            .into_iter()
            .filter(|profile| {
                let vault_label = self.effective_vault_name(profile.vault_id.as_deref());
                let jump_host_label = profile
                    .jump_host_id
                    .as_deref()
                    .and_then(|jump_host_id| self.jump_host_display_name(jump_host_id))
                    .unwrap_or_default();
                let haystacks = [
                    profile.display_name(),
                    if profile.favorite {
                        "starred favorite".to_string()
                    } else {
                        String::new()
                    },
                    profile.group.clone(),
                    profile.tags.join(" "),
                    profile.host.clone(),
                    profile.username.clone(),
                    profile.endpoint(),
                    profile.startup_directory.clone().unwrap_or_default(),
                    profile.startup_command.clone().unwrap_or_default(),
                    profile
                        .effective_port_forward_rules()
                        .iter()
                        .map(PortForwardRule::display_name)
                        .collect::<Vec<_>>()
                        .join(" "),
                    vault_label,
                    jump_host_label,
                    profile.description.clone(),
                ];
                haystacks
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&query))
            })
            .collect()
    }

    fn filtered_profile_ids(&self, cx: &App) -> Vec<String> {
        self.filtered_profiles(cx)
            .into_iter()
            .map(|profile| profile.id)
            .collect()
    }

    fn profile_group_name(profile: &HostProfile) -> String {
        if profile.favorite {
            "Favorites".to_string()
        } else if !profile.group.trim().is_empty() {
            let group = profile.group.trim();
            group.to_string()
        } else if profile.source == ProfileSource::SshConfig {
            "Imported".to_string()
        } else {
            "Ungrouped".to_string()
        }
    }

    fn group_sort_key(group: &str) -> (u8, String) {
        match group {
            "Favorites" => (0, group.to_ascii_lowercase()),
            "Imported" => (2, group.to_ascii_lowercase()),
            "Ungrouped" => (3, group.to_ascii_lowercase()),
            _ => (1, group.to_ascii_lowercase()),
        }
    }

    fn grouped_profiles(&self, cx: &App) -> Vec<(String, Vec<HostProfile>)> {
        let mut groups: Vec<(String, Vec<HostProfile>)> = Vec::new();

        for profile in self.filtered_profiles(cx) {
            let group_name = Self::profile_group_name(&profile);
            if let Some((_, items)) = groups.iter_mut().find(|(name, _)| *name == group_name) {
                items.push(profile);
            } else {
                groups.push((group_name, vec![profile]));
            }
        }

        groups.sort_by(|(left, _), (right, _)| {
            Self::group_sort_key(left).cmp(&Self::group_sort_key(right))
        });
        let order = self.hosts_sort;
        for (_, profiles) in groups.iter_mut() {
            profiles.sort_by(|a, b| match order {
                HostsSort::AZ => a
                    .display_name()
                    .to_ascii_lowercase()
                    .cmp(&b.display_name().to_ascii_lowercase()),
                HostsSort::ZA => b
                    .display_name()
                    .to_ascii_lowercase()
                    .cmp(&a.display_name().to_ascii_lowercase()),
                HostsSort::NewestFirst => self
                    .last_connected_at(b)
                    .unwrap_or(0)
                    .cmp(&self.last_connected_at(a).unwrap_or(0)),
                HostsSort::OldestFirst => self
                    .last_connected_at(a)
                    .unwrap_or(u64::MAX)
                    .cmp(&self.last_connected_at(b).unwrap_or(u64::MAX)),
            });
        }
        groups
    }

    fn submit_create_host_from_empty_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = self
            .shell_inputs
            .create_host_address
            .read(cx)
            .value()
            .trim()
            .to_string();
        if value.is_empty() {
            return false;
        }
        // Open the editor with the typed value pre-filled in label + host,
        // and seed username from the OS so save_profile validation passes.
        self.open_editor_for_new_host_with_address(value, window, cx);
        let current_user = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("USERNAME").ok())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "user".to_string());
        if self.inputs.username.read(cx).value().trim().is_empty() {
            Self::set_input_value(&self.inputs.username, current_user, window, cx);
        }
        self.save_profile(window, cx);
        true
    }

    fn toggle_new_host_menu(&mut self, cx: &mut Context<Self>) {
        self.show_new_host_menu = !self.show_new_host_menu;
        cx.notify();
    }

    fn focus_host_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_inputs
            .host_search
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn focus_terminal_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_inputs
            .terminal_search
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn focus_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_inputs
            .command_palette
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn set_terminal_search_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.terminal_search, value, window, cx);
    }

    fn set_command_palette_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.command_palette, value, window, cx);
    }

    fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_command_palette = false;
        self.selected_command_palette_index = 0;
        self.set_command_palette_input("", window, cx);
        if let Some(pane) = self.active_pane() {
            pane.terminal_focus.focus(window);
        }
        cx.notify();
    }

    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        if workspace.view_mode != WorkspaceViewMode::Terminal {
            self.error_message = "Switch back to Terminal to run commands.".to_string();
            cx.notify();
            return;
        }
        let Some(current_input) = self.active_pane().map(|pane| pane.current_input.clone()) else {
            self.error_message = "Open a terminal session to run commands.".to_string();
            cx.notify();
            return;
        };

        if self.show_command_palette {
            self.close_command_palette(window, cx);
            return;
        }

        self.show_command_palette = true;
        self.selected_command_palette_index = 0;
        self.set_command_palette_input(current_input.trim().to_string(), window, cx);
        self.focus_command_palette(window, cx);
        self.status_message = "Command palette ready.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn activate_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_workspace_id = None;
        self.nav_section = NavSection::Hosts;
        self.set_terminal_search_input("", window, cx);
        self.set_command_palette_input("", window, cx);
        self.show_command_palette = false;
        self.selected_command_palette_index = 0;
        self.status_message = "Host library ready.".to_string();
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    fn activate_library_section(
        &mut self,
        section: NavSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_workspace_id = None;
        if self.nav_section != section {
            self.show_editor_panel = false;
        }
        self.nav_section = section;
        self.set_command_palette_input("", window, cx);
        self.show_command_palette = false;
        self.selected_command_palette_index = 0;
        self.error_message.clear();
        self.status_message = format!("{} ready.", section.label());
        self.persist_runtime_state();
        cx.notify();
    }

    fn save_settings(&mut self) {
        self.saved.ensure_settings();
        let _ = save_saved_state(&self.saved);
    }

    fn imported_host_count(&self) -> usize {
        self.saved
            .profiles
            .iter()
            .filter(|profile| profile.source == ProfileSource::SshConfig)
            .count()
    }

    fn user_host_count(&self) -> usize {
        self.saved
            .profiles
            .iter()
            .filter(|profile| profile.source == ProfileSource::User)
            .count()
    }

    fn should_show_onboarding(&self) -> bool {
        self.active_workspace_id.is_none()
            && self.nav_section == NavSection::Hosts
            && !self.saved.settings.onboarding_dismissed
    }

    fn mark_onboarding_complete(&mut self) {
        if !self.saved.settings.onboarding_dismissed {
            self.saved.settings.onboarding_dismissed = true;
            self.save_settings();
        }
    }

    fn dismiss_onboarding(&mut self, cx: &mut Context<Self>) {
        self.mark_onboarding_complete();
        self.status_message = "Welcome panel dismissed.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn update_theme_preset(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.saved.settings.theme_preset = preset;
        theme::set_theme_preset(preset);
        self.save_settings();
        self.status_message = format!("Theme set to {}.", preset.label());
        self.error_message.clear();
        cx.notify();
    }

    fn update_restore_workspaces_on_launch(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.saved.settings.restore_workspaces_on_launch = enabled;
        self.save_settings();
        self.status_message = if enabled {
            "Saved workspaces will reopen on launch.".to_string()
        } else {
            "Launch now opens directly into the library.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn update_session_log_limit(&mut self, limit: u16, cx: &mut Context<Self>) {
        self.saved.settings.session_log_limit = limit;
        self.saved.ensure_settings();
        self.save_settings();
        self.status_message = format!("Session history retention set to {limit} entries.");
        self.error_message.clear();
        cx.notify();
    }

    fn update_ssh_keepalive_secs(&mut self, secs: u16, cx: &mut Context<Self>) {
        self.saved.settings.ssh_keepalive_secs = secs;
        self.saved.ensure_settings();
        self.save_settings();
        self.status_message = if secs == 0 {
            "SSH keep-alive disabled.".to_string()
        } else {
            format!("SSH keep-alive set to {secs}s.")
        };
        self.error_message.clear();
        cx.notify();
    }

    fn update_auto_reconnect_attempts(&mut self, attempts: u8, cx: &mut Context<Self>) {
        self.saved.settings.auto_reconnect_attempts = attempts;
        self.saved.ensure_settings();
        self.save_settings();
        self.status_message = if attempts == 0 {
            "Auto-reconnect disabled.".to_string()
        } else {
            format!("Auto-reconnect set to {attempts} attempts.")
        };
        self.error_message.clear();
        cx.notify();
    }

    fn update_auto_reconnect_delay(&mut self, delay_secs: u8, cx: &mut Context<Self>) {
        self.saved.settings.auto_reconnect_delay_secs = delay_secs;
        self.saved.ensure_settings();
        self.save_settings();
        self.status_message = format!("Auto-reconnect delay set to {delay_secs}s.");
        self.error_message.clear();
        cx.notify();
    }

    fn update_confirm_multiline_paste(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.saved.settings.confirm_multiline_paste = enabled;
        self.save_settings();
        self.status_message = if enabled {
            "Multi-line clipboards now require confirmation before pasting.".to_string()
        } else {
            "Multi-line paste confirmation disabled.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn update_copy_on_select(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.saved.settings.copy_on_select = enabled;
        self.save_settings();
        self.status_message = if enabled {
            "Selecting text now copies it to the clipboard automatically.".to_string()
        } else {
            "Auto-copy on selection disabled.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn reset_onboarding_panel(&mut self, cx: &mut Context<Self>) {
        self.saved.settings.onboarding_dismissed = false;
        self.save_settings();
        self.status_message = "Welcome panel reset. Open Hosts to see it again.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn update_terminal_font_size(
        &mut self,
        font_size: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.saved.settings.terminal_font_size = font_size;
        self.save_settings();
        self.sync_terminal_layout(window, cx);
        self.status_message = format!("Terminal font size set to {} px.", font_size);
        self.error_message.clear();
        cx.notify();
    }

    fn save_local_shell_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let program = self
            .settings_inputs
            .local_shell_program
            .read(cx)
            .value()
            .trim()
            .to_string();
        if program.is_empty() {
            self.error_message = "Local shell program cannot be empty.".to_string();
            cx.notify();
            return;
        }

        let cwd = self
            .settings_inputs
            .local_shell_cwd
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.saved.settings.default_local_shell.program = program.clone();
        self.saved.settings.default_local_shell.cwd = (!cwd.is_empty()).then_some(cwd.clone());
        self.save_settings();
        self.load_settings_inputs(window, cx);
        self.status_message = format!("Default local shell set to {}.", program);
        self.error_message.clear();
        cx.notify();
    }

    fn save_terminal_font_family(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let family = self
            .settings_inputs
            .terminal_font_family
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.saved.settings.terminal_font_family = (!family.is_empty()).then(|| family.clone());
        self.save_settings();
        self.sync_terminal_layout(window, cx);
        self.status_message = if family.is_empty() {
            "Terminal font family reset to the app default.".to_string()
        } else {
            format!("Terminal font family set to {family}.")
        };
        self.error_message.clear();
        cx.notify();
    }

    fn clear_terminal_font_family(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saved.settings.terminal_font_family = None;
        self.save_settings();
        Self::set_input_value(
            &self.settings_inputs.terminal_font_family,
            String::new(),
            window,
            cx,
        );
        self.sync_terminal_layout(window, cx);
        self.status_message = "Terminal font family reset to the app default.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn save_default_ssh_startup_directory(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .settings_inputs
            .default_ssh_startup_directory
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.saved.settings.default_ssh_startup_directory =
            (!dir.is_empty()).then_some(dir.clone());
        self.save_settings();
        self.status_message = if let Some(ref d) = self.saved.settings.default_ssh_startup_directory
        {
            format!("Default SSH startup directory set to {}.", d)
        } else {
            "Default SSH startup directory cleared.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn clear_default_ssh_startup_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saved.settings.default_ssh_startup_directory = None;
        self.save_settings();
        Self::set_input_value(
            &self.settings_inputs.default_ssh_startup_directory,
            String::new(),
            window,
            cx,
        );
        self.status_message = "Default SSH startup directory cleared.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn export_portable_data(&mut self, cx: &mut Context<Self>) {
        let Some(path) = FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name("termirust-export.json")
            .save_file()
        else {
            return;
        };

        match export_portable_data_bundle(&path, &self.saved, &self.known_hosts) {
            Ok(report) => {
                self.status_message = format!(
                    "Exported {} hosts, {} identities, {} snippets, {} vaults, and {} known hosts.",
                    report.profiles,
                    report.identities,
                    report.snippets,
                    report.vaults,
                    report.known_hosts
                );
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = format!("Failed to export data bundle: {error:#}");
            }
        }
        cx.notify();
    }

    fn pick_sync_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = FileDialog::new().pick_folder() else {
            return;
        };
        let path_str = path.display().to_string();
        self.saved.settings.sync_folder_path = Some(path_str.clone());
        self.save_settings();
        Self::set_input_value(
            &self.settings_inputs.sync_folder_input,
            path_str.clone(),
            window,
            cx,
        );
        self.status_message = format!("Sync folder set to {path_str}.");
        self.error_message.clear();
        cx.notify();
    }

    fn save_sync_folder_input(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let entered = self
            .settings_inputs
            .sync_folder_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.saved.settings.sync_folder_path = (!entered.is_empty()).then(|| entered.clone());
        self.save_settings();
        self.status_message = if entered.is_empty() {
            "Sync folder cleared.".to_string()
        } else {
            format!("Sync folder set to {entered}.")
        };
        self.error_message.clear();
        cx.notify();
    }

    fn sync_bundle_path(&self) -> Option<std::path::PathBuf> {
        self.saved
            .settings
            .sync_folder_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(|p| std::path::Path::new(p).join("termirust-vault.encrypted.json"))
    }

    fn push_to_sync_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.sync_bundle_path() else {
            self.error_message =
                "Pick a sync folder before pushing the encrypted bundle.".to_string();
            cx.notify();
            return;
        };
        let passphrase = self
            .settings_inputs
            .export_backup_passphrase
            .read(cx)
            .value()
            .to_string();
        let confirm = self
            .settings_inputs
            .export_backup_confirm
            .read(cx)
            .value()
            .to_string();
        if passphrase.trim().is_empty() {
            self.error_message =
                "Set a backup passphrase in the Encrypted Backup card before pushing.".to_string();
            cx.notify();
            return;
        }
        if passphrase != confirm {
            self.error_message = "Backup passphrase confirmation does not match.".to_string();
            cx.notify();
            return;
        }
        if let Some(parent) = target.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.error_message = format!("Unable to create sync folder: {error}");
                cx.notify();
                return;
            }
        }
        match export_encrypted_portable_data_bundle(
            &target,
            &self.saved,
            &self.known_hosts,
            &passphrase,
        ) {
            Ok(report) => {
                self.clear_backup_inputs(window, cx);
                self.saved.settings.sync_last_pushed_at = Some(current_unix_millis());
                self.save_settings();
                self.status_message = format!(
                    "Synced to {}: {} hosts, {} identities, {} snippets.",
                    target.display(),
                    report.profiles,
                    report.identities,
                    report.snippets
                );
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = format!("Failed to push to sync folder: {error:#}");
            }
        }
        cx.notify();
    }

    fn sync_bundle_modified_at(&self) -> Option<u64> {
        let target = self.sync_bundle_path()?;
        let metadata = std::fs::metadata(&target).ok()?;
        let modified = metadata.modified().ok()?;
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64)
    }

    fn pull_from_sync_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.sync_bundle_path() else {
            self.error_message =
                "Pick a sync folder before pulling the encrypted bundle.".to_string();
            cx.notify();
            return;
        };
        if !target.exists() {
            self.error_message = format!(
                "No bundle to pull at {} - push from another machine first.",
                target.display()
            );
            cx.notify();
            return;
        }
        let passphrase = self
            .settings_inputs
            .import_backup_passphrase
            .read(cx)
            .value()
            .to_string();
        if passphrase.trim().is_empty() {
            self.error_message =
                "Enter the backup passphrase in the Encrypted Backup card before pulling."
                    .to_string();
            cx.notify();
            return;
        }
        if !self.sync_pull_force {
            if let (Some(remote_at), Some(pushed_at)) = (
                self.sync_bundle_modified_at(),
                self.saved.settings.sync_last_pushed_at,
            ) {
                if pushed_at > remote_at + 5_000 {
                    self.sync_pull_pending_warning = true;
                    self.error_message = format!(
                        "Conflict: this machine's last push ({}) is newer than the bundle in the sync folder ({}). Confirm to overwrite local state.",
                        format_relative_time(pushed_at),
                        format_relative_time(remote_at),
                    );
                    cx.notify();
                    return;
                }
            }
        }
        self.sync_pull_force = false;
        self.sync_pull_pending_warning = false;
        match import_encrypted_portable_data_bundle(
            &target,
            &mut self.saved,
            &self.known_hosts,
            &passphrase,
        ) {
            Ok(report) => {
                let _ = save_saved_state(&self.saved);
                self.load_settings_inputs(window, cx);
                theme::set_theme_preset(self.saved.settings.theme_preset);
                self.saved.settings.sync_last_pulled_at = Some(current_unix_millis());
                self.save_settings();
                self.status_message = format!(
                    "Pulled from {}: merged {} hosts, {} identities, {} snippets.",
                    target.display(),
                    report.profiles,
                    report.identities,
                    report.snippets
                );
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = format!("Failed to pull from sync folder: {error:#}");
            }
        }
        cx.notify();
    }

    fn force_pull_from_sync_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_pull_force = true;
        self.pull_from_sync_folder(window, cx);
    }

    fn export_encrypted_portable_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let passphrase = self
            .settings_inputs
            .export_backup_passphrase
            .read(cx)
            .value()
            .to_string();
        let confirm = self
            .settings_inputs
            .export_backup_confirm
            .read(cx)
            .value()
            .to_string();

        if passphrase.trim().is_empty() {
            self.error_message = "Backup passphrase cannot be empty.".to_string();
            cx.notify();
            return;
        }
        if passphrase != confirm {
            self.error_message = "Backup passphrase confirmation does not match.".to_string();
            cx.notify();
            return;
        }

        let Some(path) = FileDialog::new()
            .add_filter("Encrypted Backup", &["json"])
            .set_file_name("termirust-backup.encrypted.json")
            .save_file()
        else {
            return;
        };

        match export_encrypted_portable_data_bundle(
            &path,
            &self.saved,
            &self.known_hosts,
            &passphrase,
        ) {
            Ok(report) => {
                self.clear_backup_inputs(window, cx);
                self.status_message = format!(
                    "Encrypted backup exported with {} hosts, {} identities, {} snippets, {} vaults, and {} known hosts.",
                    report.profiles,
                    report.identities,
                    report.snippets,
                    report.vaults,
                    report.known_hosts
                );
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = format!("Failed to export encrypted backup: {error:#}");
            }
        }
        cx.notify();
    }

    fn import_portable_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = FileDialog::new().add_filter("JSON", &["json"]).pick_file() else {
            return;
        };

        match import_portable_data_bundle(&path, &mut self.saved, &self.known_hosts) {
            Ok(report) => {
                let _ = save_saved_state(&self.saved);
                self.load_settings_inputs(window, cx);
                theme::set_theme_preset(self.saved.settings.theme_preset);
                self.status_message = format!(
                    "Imported {} hosts, {} identities, {} snippets, {} vaults, and {} known hosts.",
                    report.profiles,
                    report.identities,
                    report.snippets,
                    report.vaults,
                    report.known_hosts
                );
                self.error_message.clear();
                cx.notify();
            }
            Err(error) => {
                self.error_message = format!("Failed to import data bundle: {error:#}");
                cx.notify();
            }
        }
    }

    fn import_encrypted_portable_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let passphrase = self
            .settings_inputs
            .import_backup_passphrase
            .read(cx)
            .value()
            .to_string();

        if passphrase.trim().is_empty() {
            self.error_message = "Backup passphrase cannot be empty.".to_string();
            cx.notify();
            return;
        }

        let Some(path) = FileDialog::new()
            .add_filter("Encrypted Backup", &["json"])
            .pick_file()
        else {
            return;
        };

        match import_encrypted_portable_data_bundle(
            &path,
            &mut self.saved,
            &self.known_hosts,
            &passphrase,
        ) {
            Ok(report) => {
                let _ = save_saved_state(&self.saved);
                self.load_settings_inputs(window, cx);
                theme::set_theme_preset(self.saved.settings.theme_preset);
                self.status_message = format!(
                    "Encrypted backup imported with {} hosts, {} identities, {} snippets, {} vaults, and {} known hosts.",
                    report.profiles,
                    report.identities,
                    report.snippets,
                    report.vaults,
                    report.known_hosts
                );
                self.error_message.clear();
                cx.notify();
            }
            Err(error) => {
                self.error_message = format!("Failed to import encrypted backup: {error:#}");
                cx.notify();
            }
        }
    }

    fn persist_runtime_state(&mut self) {
        let mut restored_workspaces = Vec::new();
        let mut active_workspace_index = None;

        for workspace in &self.workspaces {
            let mut panes = Vec::new();
            let mut active_pane_index = 0;
            let mut saved_index: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();

            for pane_id in &workspace.pane_ids {
                let Some(pane) = self.pane(*pane_id) else {
                    continue;
                };
                let Some(restorable) = pane.request.to_restorable() else {
                    continue;
                };

                if workspace.active_pane_id == *pane_id {
                    active_pane_index = panes.len();
                }
                saved_index.insert(*pane_id, panes.len());
                panes.push(restorable);
            }

            if panes.is_empty() {
                continue;
            }

            let layout = workspace
                .layout
                .as_ref()
                .and_then(|node| split_node_to_saved(node, &saved_index));

            let mut saved_workspace = SavedWorkspace {
                title: workspace.title.clone(),
                layout,
                active_pane_index,
                panes,
            };
            saved_workspace.normalize();

            if self.active_workspace_id == Some(workspace.id) {
                active_workspace_index = Some(restored_workspaces.len());
            }

            restored_workspaces.push(saved_workspace);
        }

        self.saved.restored_workspaces = restored_workspaces;
        self.saved.active_workspace_index = active_workspace_index;

        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
        }
    }

    /// Capture the current window frame and the display it's on, then schedule
    /// a debounced save so the stream of resize/move events doesn't thrash
    /// `state.json`. The frame is reapplied on the next launch (see `main.rs`).
    fn persist_window_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Ignore the unsettled bounds reported while the window is opening.
        if self.launched_at.elapsed() < Duration::from_millis(1200) {
            return;
        }
        let frame = window.bounds();
        let bounds = SavedWindowBounds {
            x: f32::from(frame.origin.x),
            y: f32::from(frame.origin.y),
            width: f32::from(frame.size.width),
            height: f32::from(frame.size.height),
            display_id: window.display(cx).map(|display| display.id().into()),
        };
        if self.saved.window_bounds == Some(bounds) {
            return;
        }
        self.saved.window_bounds = Some(bounds);
        self._window_bounds_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            let _ = this.update(cx, |this, _| {
                if let Err(error) = save_saved_state(&this.saved) {
                    this.error_message = error.to_string();
                }
            });
        }));
    }

    fn restore_saved_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut saved_workspaces = self.saved.restored_workspaces.clone();
        if saved_workspaces.is_empty() {
            return;
        }

        let restore_active_index = self.saved.active_workspace_index;
        self.saved.restored_workspaces.clear();
        self.saved.active_workspace_index = None;

        let mut restored_workspace_ids = Vec::new();
        let mut restored_panes = 0usize;

        for saved_workspace in &mut saved_workspaces {
            saved_workspace.normalize();
            if saved_workspace.panes.is_empty() {
                continue;
            }

            let workspace_id = self.next_workspace_id();
            let mut pane_ids = Vec::with_capacity(saved_workspace.panes.len());
            let mut active_pane_id = None;

            for (pane_index, pane_state) in saved_workspace.panes.iter().enumerate() {
                let request = pane_state.to_connect_request(self.next_session_id());
                let pane_id = self.spawn_pane(request, window, cx);
                if pane_index == saved_workspace.active_pane_index {
                    active_pane_id = Some(pane_id);
                }
                pane_ids.push(pane_id);
                restored_panes += 1;
            }

            let Some(active_pane_id) = active_pane_id.or_else(|| pane_ids.first().copied()) else {
                continue;
            };

            let title = if saved_workspace.title.trim().is_empty() {
                self.pane(active_pane_id)
                    .map(|pane| pane.title.clone())
                    .unwrap_or_else(|| "Restored session".to_string())
            } else {
                saved_workspace.title.trim().to_string()
            };

            let layout = saved_workspace
                .layout
                .as_ref()
                .and_then(|node| saved_to_split_node(node, &pane_ids))
                .or_else(|| flat_split(&pane_ids, SplitAxis::Horizontal));

            self.workspaces.push(WorkspaceTab {
                id: workspace_id,
                title,
                pane_ids,
                active_pane_id,
                unread_events: 0,
                layout,
                view_mode: WorkspaceViewMode::Terminal,
                sftp: None,
                search_visible: false,
                search_query: String::new(),
                search_results: Vec::new(),
                active_search_index: None,
                broadcast_input: false,
                pending_connect: None,
                pending_connect_mode: ConnectDialogMode::Username,
                pending_connect_protocol: ConnectProtocol::Ssh,
                connect_failure: None,
            });
            restored_workspace_ids.push(workspace_id);
        }

        self.active_workspace_id =
            restore_active_index.and_then(|index| restored_workspace_ids.get(index).copied());

        if let Some(workspace_id) = self.active_workspace_id {
            if let Some(active_pane_id) = self.workspace(workspace_id).map(|w| w.active_pane_id) {
                if let Some(pane) = self.pane(active_pane_id) {
                    pane.terminal_focus.focus(window);
                }
            }
            self.sync_terminal_layout(window, cx);
        }

        if !restored_workspace_ids.is_empty() {
            self.status_message = format!(
                "Restored {} workspace{} and {} pane{}.",
                restored_workspace_ids.len(),
                if restored_workspace_ids.len() == 1 {
                    ""
                } else {
                    "s"
                },
                restored_panes,
                if restored_panes == 1 { "" } else { "s" }
            );
            self.error_message.clear();
            self.persist_runtime_state();
        }
    }

    fn active_workspace(&self) -> Option<&WorkspaceTab> {
        self.active_workspace_id
            .and_then(|id| self.workspaces.iter().find(|item| item.id == id))
    }

    fn workspace(&self, workspace_id: u64) -> Option<&WorkspaceTab> {
        self.workspaces.iter().find(|item| item.id == workspace_id)
    }

    fn workspace_mut(&mut self, workspace_id: u64) -> Option<&mut WorkspaceTab> {
        self.workspaces
            .iter_mut()
            .find(|item| item.id == workspace_id)
    }

    fn reset_workspace_activity(&mut self, workspace_id: u64) {
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.unread_events = 0;
        }
    }

    fn record_workspace_activity(&mut self, workspace_id: u64) {
        if self.active_workspace_id == Some(workspace_id) {
            self.reset_workspace_activity(workspace_id);
            return;
        }

        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.unread_events = workspace.unread_events.saturating_add(1);
        }
    }

    fn reorder_workspace_tabs(
        &mut self,
        dragged_workspace_id: u64,
        target_workspace_id: Option<u64>,
        insert_after: bool,
    ) {
        let Some(from_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == dragged_workspace_id)
        else {
            return;
        };

        let workspace = self.workspaces.remove(from_index);

        let insert_index = match target_workspace_id.and_then(|target_id| {
            self.workspaces
                .iter()
                .position(|workspace| workspace.id == target_id)
        }) {
            Some(target_index) if insert_after => target_index + 1,
            Some(target_index) => target_index,
            None => self.workspaces.len(),
        };

        self.workspaces
            .insert(insert_index.min(self.workspaces.len()), workspace);
        self.persist_runtime_state();
    }

    fn pane(&self, pane_id: u64) -> Option<&SessionPane> {
        self.panes.iter().find(|item| item.id == pane_id)
    }

    fn pane_mut(&mut self, pane_id: u64) -> Option<&mut SessionPane> {
        self.panes.iter_mut().find(|item| item.id == pane_id)
    }

    fn active_pane(&self) -> Option<&SessionPane> {
        let workspace = self.active_workspace()?;
        self.pane(workspace.active_pane_id)
    }

    fn active_pane_mut(&mut self) -> Option<&mut SessionPane> {
        let pane_id = self.active_workspace()?.active_pane_id;
        self.pane_mut(pane_id)
    }

    fn pane_workspace_id(&self, pane_id: u64) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.pane_ids.contains(&pane_id))
            .map(|workspace| workspace.id)
    }

    fn start_workspace_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        self.start_workspace_rename_for(workspace_id, window, cx);
    }

    fn start_workspace_rename_for(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self
            .workspace(workspace_id)
            .map(|workspace| workspace.title.clone())
            .unwrap_or_default();
        if title.is_empty() {
            return;
        }
        self.active_workspace_id = Some(workspace_id);
        self.open_workspace_tab_menu = None;
        self.tab_rename_workspace_id = Some(workspace_id);
        Self::set_input_value(&self.tab_rename_input, title, window, cx);
        self.tab_rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    fn commit_workspace_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.tab_rename_workspace_id else {
            return;
        };
        let new_title = self.tab_rename_input.read(cx).value().trim().to_string();
        if new_title.is_empty() {
            self.cancel_workspace_rename(window, cx);
            return;
        }
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.title = new_title.clone();
        }
        self.tab_rename_workspace_id = None;
        self.persist_runtime_state();
        self.status_message = format!("Workspace renamed to {new_title}.");
        self.error_message.clear();
        if let Some(pane_id) = self.active_pane().map(|pane| pane.id) {
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
        }
        cx.notify();
    }

    fn cancel_workspace_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_rename_workspace_id = None;
        if let Some(pane_id) = self.active_pane().map(|pane| pane.id) {
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
        }
        cx.notify();
    }

    fn start_pane_rename(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .pane(pane_id)
            .map(|pane| pane.title.clone())
            .unwrap_or_default();
        self.pane_rename_id = Some(pane_id);
        Self::set_input_value(&self.pane_rename_input, title, window, cx);
        self.pane_rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    fn commit_pane_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) = self.pane_rename_id else {
            return;
        };
        let new_title = self.pane_rename_input.read(cx).value().trim().to_string();
        if new_title.is_empty() {
            self.cancel_pane_rename(window, cx);
            return;
        }
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.title = new_title.clone();
            pane.request.title = new_title.clone();
        }
        self.pane_rename_id = None;
        self.persist_runtime_state();
        self.status_message = format!("Pane renamed to {new_title}.");
        self.error_message.clear();
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        cx.notify();
    }

    fn cancel_pane_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane_id = self.pane_rename_id.take();
        if let Some(pane_id) = pane_id {
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
        }
        cx.notify();
    }

    fn cycle_active_workspace(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspaces.len() < 2 {
            return false;
        }
        let Some(current) = self.active_workspace_id else {
            if let Some(first) = self.workspaces.first().map(|w| w.id) {
                self.activate_workspace(first, window, cx);
                return true;
            }
            return false;
        };
        let Some(index) = self.workspaces.iter().position(|w| w.id == current) else {
            return false;
        };
        let count = self.workspaces.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        let next_id = self.workspaces[next].id;
        if next_id != current {
            self.activate_workspace(next_id, window, cx);
            return true;
        }
        false
    }

    fn activate_workspace(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        let search_query = workspace.search_query.clone();
        let active_pane_id = workspace.active_pane_id;

        self.active_workspace_id = Some(workspace_id);
        self.reset_workspace_activity(workspace_id);
        self.nav_section = NavSection::Hosts;
        self.set_terminal_search_input(search_query, window, cx);
        if let Some(pane) = self.pane(active_pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.sync_terminal_layout(window, cx);
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    fn next_session_id(&mut self) -> u64 {
        let session_id = self.next_session_id;
        self.next_session_id += 1;
        session_id
    }

    fn next_workspace_id(&mut self) -> u64 {
        let workspace_id = self.next_workspace_id;
        self.next_workspace_id += 1;
        workspace_id
    }

    fn next_sftp_operation_id(&mut self) -> u64 {
        let operation_id = self.next_sftp_operation_id;
        self.next_sftp_operation_id += 1;
        operation_id
    }

    fn selected_workspace_sftp_entry(&self, workspace_id: u64) -> Option<RemoteFileEntry> {
        let workspace = self.workspace(workspace_id)?;
        let browser = workspace.sftp.as_ref()?;
        let selected_path = browser.selected_path.as_deref()?;
        browser
            .entries
            .iter()
            .find(|entry| entry.path == selected_path)
            .cloned()
    }

    fn open_workspace_files_for_pane(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.pane(pane_id) else {
            return;
        };
        if pane.request.is_local_shell() {
            self.error_message = "Remote files are only available for SSH sessions.".to_string();
            cx.notify();
            return;
        }

        let endpoint = pane.endpoint.clone();
        let request = pane.request.clone();
        let path = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .filter(|browser| browser.pane_id == pane_id)
            .map(|browser| browser.current_path.clone())
            .or_else(|| request.startup_directory.clone())
            .unwrap_or_else(|| ".".to_string());

        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.view_mode = WorkspaceViewMode::Files;
            if workspace
                .sftp
                .as_ref()
                .is_none_or(|browser| browser.pane_id != pane_id)
            {
                workspace.sftp = Some(WorkspaceSftpState::new(pane_id, request, path.clone()));
            }
        }

        self.status_message = format!("Loading remote files from {endpoint}...");
        self.error_message.clear();
        self.load_workspace_directory(workspace_id, path);
        cx.notify();
    }

    fn open_active_workspace_files(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            return;
        };
        self.open_workspace_files_for_pane(workspace_id, pane_id, cx);
    }

    fn show_active_workspace_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.view_mode = WorkspaceViewMode::Terminal;
        }
        self.status_message = "Back to terminal view.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn load_workspace_directory(&mut self, workspace_id: u64, path: String) {
        let operation_id = self.next_sftp_operation_id();
        let Some((request, load_path)) = self.workspace_mut(workspace_id).and_then(|workspace| {
            let browser = workspace.sftp.as_mut()?;
            browser.current_path = path.clone();
            browser.loading = true;
            browser.pending_operation_id = Some(operation_id);
            browser.selected_path = None;
            Some((browser.request.clone(), browser.current_path.clone()))
        }) else {
            return;
        };

        spawn_list_directory(
            workspace_id,
            operation_id,
            request,
            self.known_hosts.clone(),
            load_path,
            self.sftp_event_tx.clone(),
        );
    }

    fn refresh_workspace_files(&mut self, workspace_id: u64) {
        let Some(path) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .map(|browser| browser.current_path.clone())
        else {
            return;
        };
        self.load_workspace_directory(workspace_id, path);
    }

    fn select_workspace_file_entry(
        &mut self,
        workspace_id: u64,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(browser) = self
            .workspace_mut(workspace_id)
            .and_then(|workspace| workspace.sftp.as_mut())
        {
            browser.selected_path = Some(path);
        }
        cx.notify();
    }

    fn open_selected_workspace_file_entry(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(entry) = self.selected_workspace_sftp_entry(workspace_id) else {
            self.error_message = "Select a folder first.".to_string();
            cx.notify();
            return;
        };
        if !entry.is_dir {
            self.error_message = "Only folders can be opened in the remote browser.".to_string();
            cx.notify();
            return;
        }

        self.status_message = format!("Opening {}...", entry.path);
        self.error_message.clear();
        self.load_workspace_directory(workspace_id, entry.path);
        cx.notify();
    }

    fn navigate_workspace_files_up(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(parent_path) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .and_then(|browser| remote_parent_path(&browser.current_path))
        else {
            return;
        };

        self.status_message = format!("Opening {parent_path}...");
        self.error_message.clear();
        self.load_workspace_directory(workspace_id, parent_path);
        cx.notify();
    }

    fn upload_workspace_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some((request, current_path)) = self.workspace(workspace_id).and_then(|workspace| {
            workspace
                .sftp
                .as_ref()
                .map(|browser| (browser.request.clone(), browser.current_path.clone()))
        }) else {
            return;
        };
        let Some(local_path) = FileDialog::new().pick_file() else {
            return;
        };

        let operation_id = self.next_sftp_operation_id();
        if let Some(browser) = self
            .workspace_mut(workspace_id)
            .and_then(|workspace| workspace.sftp.as_mut())
        {
            browser.loading = true;
            browser.pending_operation_id = Some(operation_id);
        }

        self.status_message = format!("Uploading {}...", local_path.display());
        self.error_message.clear();
        spawn_upload_file(
            workspace_id,
            operation_id,
            request,
            self.known_hosts.clone(),
            current_path,
            local_path,
            self.sftp_event_tx.clone(),
        );
        let _ = window;
        cx.notify();
    }

    fn download_workspace_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(entry) = self.selected_workspace_sftp_entry(workspace_id) else {
            self.error_message = "Select a remote file first.".to_string();
            cx.notify();
            return;
        };
        if entry.is_dir {
            self.error_message =
                "Folders are not downloadable yet. Open the folder instead.".to_string();
            cx.notify();
            return;
        }
        let Some(request) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .map(|browser| browser.request.clone())
        else {
            return;
        };
        let Some(local_path) = FileDialog::new().set_file_name(&entry.name).save_file() else {
            return;
        };

        let operation_id = self.next_sftp_operation_id();
        if let Some(browser) = self
            .workspace_mut(workspace_id)
            .and_then(|workspace| workspace.sftp.as_mut())
        {
            browser.loading = true;
            browser.pending_operation_id = Some(operation_id);
        }

        self.status_message = format!("Downloading {}...", entry.path);
        self.error_message.clear();
        spawn_download_file(
            workspace_id,
            operation_id,
            request,
            self.known_hosts.clone(),
            entry.path,
            local_path,
            self.sftp_event_tx.clone(),
        );
        cx.notify();
    }

    fn delete_workspace_file(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(entry) = self.selected_workspace_sftp_entry(workspace_id) else {
            self.error_message = "Select a remote file or folder first.".to_string();
            cx.notify();
            return;
        };
        let Some(request) = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .map(|browser| browser.request.clone())
        else {
            return;
        };

        let operation_id = self.next_sftp_operation_id();
        if let Some(browser) = self
            .workspace_mut(workspace_id)
            .and_then(|workspace| workspace.sftp.as_mut())
        {
            browser.loading = true;
            browser.pending_operation_id = Some(operation_id);
        }

        self.status_message = format!("Deleting {}...", entry.path);
        self.error_message.clear();
        spawn_delete_path(
            workspace_id,
            operation_id,
            request,
            self.known_hosts.clone(),
            entry.path,
            entry.is_dir,
            self.sftp_event_tx.clone(),
        );
        cx.notify();
    }

    fn build_request_for_current_draft(&mut self, cx: &App) -> anyhow::Result<ConnectRequest> {
        let mut draft = self.current_profile_draft(cx)?;

        if draft.auth_mode == AuthMode::Password {
            let password = draft.password.trim().to_string();
            if password.is_empty()
                && draft.password_credential_id.is_none()
                && !self.saved.identities.is_empty()
            {
                let identity = self.saved.identities[0].clone();
                draft.auth_mode = AuthMode::PrivateKey;
                draft.identity_id = Some(identity.id.clone());
                draft.key_path = identity.key_path.clone();
            } else if !password.is_empty() {
                let credential_id = self.draft_password_credential_id(&draft)?;
                self.persist_password_to_keychain(&credential_id, &password)?;
                draft.password_credential_id = Some(credential_id.clone());

                if let Some(profile) = self.selected_profile_mut() {
                    profile.password_credential_id = Some(credential_id);
                }
                let _ = save_saved_state(&self.saved);
            }
        }

        let jump_host_id = draft.jump_host_id.clone();
        let mut request = draft.to_connect_request(self.next_session_id())?;
        if let Some(jump_host_id) = jump_host_id {
            request.jump_host = Some(self.resolve_jump_host_connection(&jump_host_id)?);
        }

        Ok(request)
    }

    fn spawn_pane(
        &mut self,
        request: ConnectRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let pane_id = request.session_id;
        let endpoint = request.endpoint_label();
        let title = request.title.clone();
        let terminal_scrollback_rows = request.terminal_scrollback_rows;
        eprintln!("[app] spawn_pane: pane_id={pane_id} title='{title}' endpoint={endpoint}");
        let terminal_focus = cx.focus_handle().tab_stop(true);
        let runtime = if request.kind == ConnectionKind::LocalShell {
            spawn_local_session(request.clone(), self.event_tx.clone())
        } else {
            spawn_session(
                request.clone(),
                self.known_hosts.clone(),
                self.event_tx.clone(),
                self.saved.settings.ssh_keepalive_secs,
            )
        };
        eprintln!("[app] spawn_pane: session spawned, creating pane state...");

        let log_entry = SessionLogEntry::new(&request);
        let log_id = log_entry.id.clone();
        self.saved.record_session_log(log_entry);
        let _ = save_saved_state(&self.saved);

        self.panes.push(SessionPane {
            id: pane_id,
            request,
            title,
            endpoint,
            terminal: TerminalState::new(TerminalSize::default(), terminal_scrollback_rows),
            terminal_focus,
            last_size: None,
            runtime,
            connected: false,
            closed: false,
            status: "Connecting".to_string(),
            selection: None,
            dragging_selection: false,
            log_id,
            current_input: String::new(),
            selected_autocomplete_index: None,
            auto_reconnect_attempts: 0,
            auto_reconnect_at: None,
            user_closed: false,
        });

        let _ = window;
        pane_id
    }

    fn open_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = ConnectRequest::local_shell_with_config(
            0,
            self.saved.settings.default_local_shell.clone(),
        );
        let Some((_, pane_id)) = self.open_request_workspace(request.clone(), window, cx) else {
            return;
        };
        self.show_editor_panel = false;
        self.mark_onboarding_complete();
        self.status_message = "Opening local terminal...".to_string();
        self.error_message.clear();
        self.set_terminal_search_input("", window, cx);
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn open_request_workspace(
        &mut self,
        mut request: ConnectRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(u64, u64)> {
        request.session_id = self.next_session_id();
        let pane_id = self.spawn_pane(request.clone(), window, cx);
        let workspace_id = self.next_workspace_id();

        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title: request.title.clone(),
            pane_ids: vec![pane_id],
            active_pane_id: pane_id,
            unread_events: 0,
            layout: Some(SplitNode::Leaf(pane_id)),
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: None,
            pending_connect_mode: ConnectDialogMode::Username,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });

        self.active_workspace_id = Some(workspace_id);
        Some((workspace_id, pane_id))
    }

    fn connect_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("[app] connect_current: building request from draft...");
        let _ = self.ensure_default_identity_selected(window, cx);
        match self.build_request_for_current_draft(cx) {
            Ok(request) => {
                eprintln!(
                    "[app] connect_current: request ok — title='{}' address='{}' auth={:?}",
                    request.title,
                    request.address(),
                    match request.auth.as_ref() {
                        Some(AuthConfig::Password { .. }) => "password",
                        Some(AuthConfig::PasswordRef { .. }) => "stored-password",
                        Some(AuthConfig::PrivateKey { key_path, .. }) => key_path.as_str(),
                        None => "none",
                    }
                );
                let pane_id = self.spawn_pane(request.clone(), window, cx);
                eprintln!("[app] connect_current: pane spawned, pane_id={pane_id}");
                let workspace_id = self.next_workspace_id();

                self.workspaces.push(WorkspaceTab {
                    id: workspace_id,
                    title: request.title.clone(),
                    pane_ids: vec![pane_id],
                    active_pane_id: pane_id,
                    unread_events: 0,
                    layout: Some(SplitNode::Leaf(pane_id)),
                    view_mode: WorkspaceViewMode::Terminal,
                    sftp: None,
                    search_visible: false,
                    search_query: String::new(),
                    search_results: Vec::new(),
                    active_search_index: None,
                    broadcast_input: false,
                    pending_connect: None,
                    pending_connect_mode: ConnectDialogMode::Username,
                    pending_connect_protocol: ConnectProtocol::Ssh,
                    connect_failure: None,
                });

                self.active_workspace_id = Some(workspace_id);
                self.show_editor_panel = false;
                self.mark_onboarding_complete();
                self.status_message = if request.kind == ConnectionKind::LocalShell {
                    "Opening local terminal...".to_string()
                } else {
                    format!("Connecting to {}...", request.address())
                };
                self.error_message.clear();
                Self::set_input_value(&self.inputs.password, "", window, cx);
                self.set_terminal_search_input("", window, cx);
                eprintln!("[app] connect_current: syncing terminal layout...");
                self.sync_terminal_layout(window, cx);
                if let Some(pane) = self.pane(pane_id) {
                    pane.terminal_focus.focus(window);
                }
                self.persist_runtime_state();
                eprintln!("[app] connect_current: done, workspace_id={workspace_id}");
                cx.notify();
            }
            Err(error) => {
                eprintln!("[app] connect_current: draft error — {error}");
                self.error_message = error.to_string();
                cx.notify();
            }
        }
    }

    fn maybe_schedule_auto_reconnect(&mut self, pane_id: u64) -> bool {
        let max_attempts = self.saved.settings.auto_reconnect_attempts;
        let delay_secs = self.saved.settings.auto_reconnect_delay_secs;
        if max_attempts == 0 {
            return false;
        }
        let Some(pane) = self.pane_mut(pane_id) else {
            return false;
        };
        if pane.user_closed || pane.request.is_local_shell() {
            return false;
        }
        if pane.auto_reconnect_attempts >= max_attempts {
            return false;
        }
        pane.auto_reconnect_attempts += 1;
        pane.auto_reconnect_at =
            Some(current_unix_millis() + u64::from(delay_secs).saturating_mul(1000));
        pane.status = format!(
            "Reconnecting in {delay_secs}s ({}/{max_attempts})",
            pane.auto_reconnect_attempts
        );
        true
    }

    fn process_pending_auto_reconnects(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = current_unix_millis();
        let max_attempts = self.saved.settings.auto_reconnect_attempts;

        let mut status_changed = false;
        let mut due_pane_ids: Vec<u64> = Vec::new();
        for pane in self.panes.iter_mut() {
            let Some(target) = pane.auto_reconnect_at else {
                continue;
            };
            if pane.user_closed {
                continue;
            }
            if now >= target {
                due_pane_ids.push(pane.id);
                continue;
            }
            let remaining_secs = ((target - now) + 999) / 1000;
            let next_status = format!(
                "Reconnecting in {remaining_secs}s ({}/{max_attempts})",
                pane.auto_reconnect_attempts
            );
            if pane.status != next_status {
                pane.status = next_status;
                status_changed = true;
            }
        }
        if due_pane_ids.is_empty() {
            if status_changed {
                cx.notify();
            }
            return status_changed;
        }
        for pane_id in due_pane_ids {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.auto_reconnect_at = None;
                pane.status = "Reconnecting...".to_string();
            }
            self.reconnect_pane(pane_id, window, cx);
        }
        true
    }

    fn reconnect_pane(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(pane_id) else {
            return;
        };
        if pane.connected {
            return;
        }
        let mut request = pane.request.clone();
        let workspace_id = self.pane_workspace_id(pane_id);

        // A passphrase-protected key fails the first connect with an empty
        // passphrase. The stored request still carries `None`, so pick up a
        // passphrase the user has since typed in the host editor.
        if let Some(AuthConfig::PrivateKey { passphrase, .. }) = request.auth.as_mut() {
            let entered = self
                .inputs
                .key_passphrase
                .read(cx)
                .value()
                .trim()
                .to_string();
            if !entered.is_empty() {
                *passphrase = Some(entered);
            }
        }

        request.session_id = self.next_session_id();
        let new_pane_id = self.spawn_pane(request.clone(), window, cx);

        if let Some(workspace_id) = workspace_id {
            if let Some(workspace) = self.workspace_mut(workspace_id) {
                if let Some(pos) = workspace.pane_ids.iter().position(|id| *id == pane_id) {
                    workspace.pane_ids[pos] = new_pane_id;
                    if workspace.active_pane_id == pane_id {
                        workspace.active_pane_id = new_pane_id;
                    }
                }
                if let Some(browser) = workspace.sftp.as_mut() {
                    if browser.pane_id == pane_id {
                        browser.pane_id = new_pane_id;
                        browser.request = request.clone();
                    }
                }
            }
        }

        if let Some(old_pane) = self.pane(pane_id) {
            let _ = old_pane.runtime.command_tx.send(SessionCommand::Disconnect);
        }
        self.panes.retain(|p| p.id != pane_id);

        self.status_message = if request.kind == ConnectionKind::LocalShell {
            "Reopening local terminal...".to_string()
        } else {
            format!("Reconnecting to {}...", request.address())
        };
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(new_pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn reconnect_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let pane_ids: Vec<u64> = {
            let Some(workspace) = self.workspace(workspace_id) else {
                return;
            };
            workspace.pane_ids.clone()
        };

        let mut reconnect_count = 0;
        for pane_id in pane_ids {
            if let Some(pane) = self.pane(pane_id) {
                if !pane.connected {
                    self.reconnect_pane(pane_id, window, cx);
                    reconnect_count += 1;
                }
            }
        }

        if reconnect_count == 0 {
            self.status_message = "No disconnected panes to reconnect.".to_string();
            cx.notify();
        } else {
            self.status_message = format!(
                "Reconnecting {} pane{}...",
                reconnect_count,
                if reconnect_count == 1 { "" } else { "s" }
            );
            cx.notify();
        }
    }

    fn quick_connect(
        &mut self,
        qc: QuickConnect,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = self.next_session_id();
        let credential_id =
            credentials::connection_password_credential_id(&qc.username, &qc.host, qc.port);
        let password = password.unwrap_or_default().trim().to_string();
        let auth = if !password.is_empty() {
            if let Err(error) = self.persist_password_to_keychain(&credential_id, &password) {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
            AuthConfig::PasswordRef { credential_id }
        } else if credentials::load_password(&credential_id).is_ok() {
            AuthConfig::PasswordRef { credential_id }
        } else if let Some(identity) = self.preferred_identity().cloned() {
            AuthConfig::PrivateKey {
                key_path: identity.key_path,
                passphrase: None,
            }
        } else {
            self.error_message = format!(
                "Quick connect needs a password, a stored system password, or an SSH key in {}.",
                ssh_directory_label()
            );
            cx.notify();
            return;
        };

        let request = qc.to_connect_request(session_id, auth);
        let pane_id = self.spawn_pane(request.clone(), window, cx);
        let workspace_id = self.next_workspace_id();

        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title: request.title.clone(),
            pane_ids: vec![pane_id],
            active_pane_id: pane_id,
            unread_events: 0,
            layout: Some(SplitNode::Leaf(pane_id)),
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: None,
            pending_connect_mode: ConnectDialogMode::Username,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });

        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
        self.mark_onboarding_complete();
        self.status_message = format!("Connecting to {}...", request.address());
        self.error_message.clear();
        self.set_terminal_search_input("", window, cx);
        self.set_quick_connect_password_input("", window, cx);
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn try_quick_connect_from_search(&self, cx: &App) -> Option<QuickConnect> {
        let query = self.host_search_query(cx);
        QuickConnect::parse(&query)
    }

    fn split_active_workspace(
        &mut self,
        axis: SplitAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        if workspace.pane_ids.len() >= MAX_SPLIT_PANES {
            self.error_message = format!("Split panes are capped at {} for now.", MAX_SPLIT_PANES);
            cx.notify();
            return;
        }

        let Some(base_request) = self
            .pane(workspace.active_pane_id)
            .map(|pane| pane.request.clone())
        else {
            return;
        };

        let mut request = base_request;
        request.session_id = self.next_session_id();
        let pane_id = self.spawn_pane(request.clone(), window, cx);

        if let Some(workspace) = self.workspace_mut(workspace_id) {
            let target = workspace.active_pane_id;
            let inserted = workspace
                .layout
                .as_mut()
                .map(|layout| layout.split_leaf(target, &SplitNode::Leaf(pane_id), axis, false))
                .unwrap_or(false);
            if !inserted {
                workspace.layout = Some(SplitNode::Leaf(pane_id));
            }
            workspace.sync_pane_ids();
            workspace.active_pane_id = pane_id;
            workspace.view_mode = WorkspaceViewMode::Terminal;
            workspace.title = request.title.clone();
        }

        self.status_message = if request.kind == ConnectionKind::LocalShell {
            "Launching split local terminal...".to_string()
        } else {
            format!("Launching split pane for {}...", request.address())
        };
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn disconnect_workspace(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        let pane_ids = workspace.pane_ids.clone();

        for pane_id in pane_ids {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.user_closed = true;
                pane.auto_reconnect_at = None;
                let _ = pane.runtime.command_tx.send(SessionCommand::Disconnect);
                pane.connected = false;
                pane.closed = true;
                pane.status = "Closing".to_string();
            }
        }

        self.status_message = "Disconnecting workspace...".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn close_pane(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.user_closed = true;
            pane.auto_reconnect_at = None;
            let _ = pane.runtime.command_tx.send(SessionCommand::Disconnect);
        }

        let Some(workspace_id) = self.pane_workspace_id(pane_id) else {
            return;
        };

        let mut remove_workspace = false;
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.layout = workspace
                .layout
                .take()
                .and_then(|layout| layout.without_pane(pane_id));
            workspace.sync_pane_ids();
            if workspace
                .sftp
                .as_ref()
                .is_some_and(|browser| browser.pane_id == pane_id)
            {
                workspace.sftp = None;
                workspace.view_mode = WorkspaceViewMode::Terminal;
            }
            remove_workspace = workspace.pane_ids.is_empty();
        }

        self.panes.retain(|item| item.id != pane_id);

        if remove_workspace {
            self.close_workspace(workspace_id, cx);
        } else {
            self.status_message = "Pane closed.".to_string();
            self.error_message.clear();
            self.persist_runtime_state();
            cx.notify();
        }
    }

    fn close_workspace(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        let pane_ids = workspace.pane_ids.clone();

        for pane_id in &pane_ids {
            if let Some(pane) = self.pane(*pane_id) {
                let _ = pane.runtime.command_tx.send(SessionCommand::Disconnect);
            }
        }

        self.workspaces.retain(|item| item.id != workspace_id);
        self.panes.retain(|pane| !pane_ids.contains(&pane.id));

        if self.active_workspace_id == Some(workspace_id) {
            self.active_workspace_id = self.workspaces.last().map(|item| item.id);
        }

        if self.active_workspace_id.is_none() {
            self.status_message = "Workspace closed. Back to hosts.".to_string();
        } else {
            self.status_message = "Workspace closed.".to_string();
        }
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    fn process_events(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut panes_to_refresh = Vec::new();
        let mut sftp_directories_to_refresh = HashSet::new();

        while let Ok(event) = self.event_rx.try_recv() {
            changed = true;

            match event {
                SshEvent::Connected {
                    session_id,
                    trusted_new_host,
                } => {
                    eprintln!(
                        "[app] event: Connected session_id={session_id} trusted_new={trusted_new_host}"
                    );
                    if let Some(workspace_id) = self.pane_workspace_id(session_id) {
                        self.record_workspace_activity(workspace_id);
                    }
                    if let Some(pane) = self.pane_mut(session_id) {
                        pane.connected = true;
                        pane.closed = false;
                        pane.status = "Live".to_string();
                        let log_id = pane.log_id.clone();
                        self.saved
                            .update_session_log(&log_id, |e| e.mark_connected());
                        let _ = save_saved_state(&self.saved);
                    } else {
                        eprintln!(
                            "[app] WARNING: Connected event for unknown pane session_id={session_id}"
                        );
                    }

                    let local_shell = self
                        .pane(session_id)
                        .is_some_and(|pane| pane.request.is_local_shell());
                    let open_files_on_connect = self.pane(session_id).is_some_and(|pane| {
                        pane.request.start_in_files && !pane.request.is_local_shell()
                    });
                    let workspace_to_open_files = if open_files_on_connect {
                        self.pane_workspace_id(session_id)
                    } else {
                        None
                    };
                    let ran_startup = !local_shell && self.send_startup_actions(session_id);
                    if let Some(workspace_id) = workspace_to_open_files {
                        self.open_workspace_files_for_pane(workspace_id, session_id, cx);
                    }
                    self.status_message = if local_shell {
                        "Local terminal ready.".to_string()
                    } else if open_files_on_connect && ran_startup && trusted_new_host {
                        "SSH session connected. Files view opened, new host key trusted and pinned, startup actions queued.".to_string()
                    } else if open_files_on_connect && ran_startup {
                        "SSH session connected. Files view opened and startup actions queued."
                            .to_string()
                    } else if open_files_on_connect && trusted_new_host {
                        "SSH session connected. Files view opened and new host key trusted."
                            .to_string()
                    } else if open_files_on_connect {
                        "SSH session connected. Files view opened.".to_string()
                    } else if ran_startup && trusted_new_host {
                        "SSH session connected. New host key trusted and pinned. Startup actions queued.".to_string()
                    } else if ran_startup {
                        "SSH session connected. Startup actions queued.".to_string()
                    } else if trusted_new_host {
                        "SSH session connected. New host key trusted and pinned.".to_string()
                    } else {
                        "SSH session connected.".to_string()
                    };
                    self.error_message.clear();
                }
                SshEvent::Output { session_id, data } => {
                    if let Some(workspace_id) = self.pane_workspace_id(session_id) {
                        self.record_workspace_activity(workspace_id);
                    }
                    if let Some(pane) = self.pane_mut(session_id) {
                        pane.terminal.process_bytes(&data);
                        if pane.selection.is_some() {
                            pane.selection = None;
                            pane.dragging_selection = false;
                        }
                        panes_to_refresh.push(session_id);
                    }
                }
                SshEvent::Error {
                    session_id,
                    message,
                } => {
                    eprintln!("[app] event: Error session_id={session_id} message={message}");
                    if let Some(workspace_id) = self.pane_workspace_id(session_id) {
                        self.record_workspace_activity(workspace_id);
                    }
                    if let Some(pane) = self.pane_mut(session_id) {
                        pane.connected = false;
                        pane.closed = true;
                        pane.status = "Error".to_string();
                        let log_id = pane.log_id.clone();
                        self.saved
                            .update_session_log(&log_id, |e| e.mark_error(&message));
                        let _ = save_saved_state(&self.saved);
                    }
                    let scheduled_reconnect = self.maybe_schedule_auto_reconnect(session_id);

                    self.error_message = message;
                    if scheduled_reconnect {
                        self.status_message = self
                            .pane(session_id)
                            .map(|pane| pane.status.clone())
                            .unwrap_or_default();
                    }
                }
                SshEvent::Disconnected {
                    session_id,
                    message,
                } => {
                    eprintln!(
                        "[app] event: Disconnected session_id={session_id} message={message}"
                    );
                    if let Some(workspace_id) = self.pane_workspace_id(session_id) {
                        self.record_workspace_activity(workspace_id);
                    }
                    let was_user_closed = self
                        .pane(session_id)
                        .map(|pane| pane.user_closed)
                        .unwrap_or(true);
                    if let Some(pane) = self.pane_mut(session_id) {
                        pane.connected = false;
                        pane.closed = true;
                        pane.status = "Closed".to_string();
                        let log_id = pane.log_id.clone();
                        self.saved
                            .update_session_log(&log_id, |e| e.mark_disconnected());
                        let _ = save_saved_state(&self.saved);
                    }
                    let mut scheduled = false;
                    if !was_user_closed {
                        scheduled = self.maybe_schedule_auto_reconnect(session_id);
                    }

                    self.status_message = if scheduled {
                        self.pane(session_id)
                            .map(|pane| pane.status.clone())
                            .unwrap_or_default()
                    } else if self
                        .pane(session_id)
                        .is_some_and(|pane| pane.request.is_local_shell())
                    {
                        "Local terminal closed.".to_string()
                    } else {
                        "SSH session closed.".to_string()
                    };
                    if !scheduled && self.error_message.is_empty() {
                        self.error_message = message;
                    }
                }
            }
        }

        while let Ok(event) = self.sftp_event_rx.try_recv() {
            changed = true;

            match event {
                SftpEvent::DirectoryLoaded {
                    workspace_id,
                    operation_id,
                    path,
                    entries,
                } => {
                    if let Some(browser) = self
                        .workspace_mut(workspace_id)
                        .and_then(|workspace| workspace.sftp.as_mut())
                    {
                        if browser.pending_operation_id != Some(operation_id) {
                            continue;
                        }

                        browser.current_path = path.clone();
                        browser.entries = entries;
                        browser.loading = false;
                        browser.pending_operation_id = None;
                        browser.selected_path =
                            browser.entries.first().map(|entry| entry.path.clone());
                    }

                    self.status_message = format!("Loaded remote files for {path}.");
                    self.error_message.clear();
                }
                SftpEvent::UploadComplete {
                    workspace_id,
                    operation_id,
                    remote_path,
                } => {
                    if let Some(browser) = self
                        .workspace_mut(workspace_id)
                        .and_then(|workspace| workspace.sftp.as_mut())
                    {
                        if browser.pending_operation_id == Some(operation_id) {
                            browser.loading = false;
                            browser.pending_operation_id = None;
                        }
                    }

                    self.status_message = format!("Uploaded {remote_path}.");
                    self.error_message.clear();
                    sftp_directories_to_refresh.insert(workspace_id);
                }
                SftpEvent::DownloadComplete {
                    workspace_id,
                    operation_id,
                    remote_path,
                    local_path,
                } => {
                    if let Some(browser) = self
                        .workspace_mut(workspace_id)
                        .and_then(|workspace| workspace.sftp.as_mut())
                    {
                        if browser.pending_operation_id == Some(operation_id) {
                            browser.loading = false;
                            browser.pending_operation_id = None;
                        }
                    }

                    self.status_message = format!("Downloaded {remote_path} to {local_path}.");
                    self.error_message.clear();
                }
                SftpEvent::DeleteComplete {
                    workspace_id,
                    operation_id,
                    remote_path,
                } => {
                    if let Some(browser) = self
                        .workspace_mut(workspace_id)
                        .and_then(|workspace| workspace.sftp.as_mut())
                    {
                        if browser.pending_operation_id == Some(operation_id) {
                            browser.loading = false;
                            browser.pending_operation_id = None;
                        }
                    }

                    self.status_message = format!("Deleted {remote_path}.");
                    self.error_message.clear();
                    sftp_directories_to_refresh.insert(workspace_id);
                }
                SftpEvent::Error {
                    workspace_id,
                    operation_id,
                    message,
                } => {
                    if let Some(browser) = self
                        .workspace_mut(workspace_id)
                        .and_then(|workspace| workspace.sftp.as_mut())
                    {
                        if browser.pending_operation_id == Some(operation_id) {
                            browser.loading = false;
                            browser.pending_operation_id = None;
                        }
                    }

                    self.error_message = message;
                }
            }
        }

        for workspace_id in sftp_directories_to_refresh {
            self.refresh_workspace_files(workspace_id);
        }

        if let Some(active_workspace_id) = self.active_workspace_id {
            let input_query = self
                .shell_inputs
                .terminal_search
                .read(cx)
                .value()
                .to_string();
            let search_changed = self
                .workspace(active_workspace_id)
                .map(|workspace| workspace.search_query != input_query)
                .unwrap_or(false);
            let output_changed = panes_to_refresh
                .iter()
                .copied()
                .any(|pane_id| self.pane_workspace_id(pane_id) == Some(active_workspace_id));

            if search_changed || output_changed {
                self.refresh_workspace_search(active_workspace_id, cx);
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn terminal_metrics(&self, window: &Window, cx: &Context<Self>) -> (f32, f32) {
        let font_size = px(self.terminal_font_size());
        let font_id = window
            .text_system()
            .resolve_font(&font(self.terminal_font_family(cx)));
        let char_width = window
            .text_system()
            .ch_advance(font_id, font_size)
            .map(|width| {
                let width: f32 = width.into();
                width.max(1.0)
            })
            .unwrap_or(8.0);
        let line_height = (self.terminal_font_size() * TERMINAL_LINE_HEIGHT).max(1.0);
        (char_width, line_height)
    }

    /// Compute pixel rects for every pane leaf and divider of the active
    /// workspace's split tree, in body-local coordinates (origin at the top-left
    /// of the terminal body, below the chrome and any search/autocomplete rows).
    fn workspace_split_rects(&self, window: &Window) -> (Vec<PaneRect>, Vec<DividerRect>) {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        let Some(workspace) = self.active_workspace() else {
            return (panes, dividers);
        };
        let Some(layout) = workspace.layout.as_ref() else {
            return (panes, dividers);
        };
        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let search_height = if workspace.search_visible {
            WORKSPACE_SEARCH_ROW_HEIGHT
        } else {
            0.0
        };
        let body_width = viewport_width.max(320.0);
        let body_height = (viewport_height - theme::CHROME_HEIGHT - search_height).max(180.0);
        compute_split_layout(
            layout,
            0.0,
            0.0,
            body_width,
            body_height,
            &mut panes,
            &mut dividers,
        );
        (panes, dividers)
    }

    fn pane_layouts(&self, window: &Window, cx: &Context<Self>) -> Vec<PaneLayout> {
        let Some(workspace) = self.active_workspace() else {
            return Vec::new();
        };
        let search_height = if workspace.search_visible {
            WORKSPACE_SEARCH_ROW_HEIGHT
        } else {
            0.0
        };
        let body_origin_y = theme::CHROME_HEIGHT + search_height;
        let (char_width, line_height) = self.terminal_metrics(window, cx);
        let (panes, _) = self.workspace_split_rects(window);

        panes
            .into_iter()
            .map(|rect| {
                let cell_width = (rect.width - TERMINAL_INNER_PADDING_X * 2.0).max(32.0);
                let cell_height = (rect.height - TERMINAL_INNER_PADDING_Y * 2.0).max(24.0);
                let cols = (cell_width / char_width).floor().max(1.0) as u16;
                let rows = (cell_height / line_height).floor().max(1.0) as u16;
                PaneLayout {
                    pane_id: rect.pane_id,
                    cell_x: rect.x + TERMINAL_INNER_PADDING_X,
                    cell_y: rect.y + body_origin_y + TERMINAL_INNER_PADDING_Y,
                    cell_width,
                    cell_height,
                    cols,
                    rows,
                    char_width,
                    line_height,
                }
            })
            .collect()
    }

    fn pane_layout_for(
        &self,
        pane_id: u64,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<PaneLayout> {
        self.pane_layouts(window, cx)
            .into_iter()
            .find(|layout| layout.pane_id == pane_id)
    }

    fn sync_terminal_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layouts = self.pane_layouts(window, cx);
        let mut changed = false;

        for layout in layouts {
            let size = TerminalSize::new(
                layout.cols,
                layout.rows,
                layout.cell_width.round().clamp(1.0, u16::MAX as f32) as u16,
                layout.cell_height.round().clamp(1.0, u16::MAX as f32) as u16,
            );

            if let Some(pane) = self.pane_mut(layout.pane_id) {
                if pane.last_size == Some(size) {
                    continue;
                }

                pane.terminal.resize(size);
                pane.last_size = Some(size);
                if !pane.closed {
                    let _ = pane.runtime.command_tx.send(SessionCommand::Resize(size));
                }
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn workspace_id_for_pane(&self, pane_id: u64) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.pane_ids.contains(&pane_id))
            .map(|workspace| workspace.id)
    }

    /// Begin dragging the divider between split panes `index` and `index + 1`.
    fn start_divider_drag(
        &mut self,
        workspace_id: u64,
        divider_id: u64,
        axis: SplitAxis,
        span: f32,
        ratio: f32,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let origin = match axis {
            SplitAxis::Horizontal => f32::from(position.x),
            SplitAxis::Vertical => f32::from(position.y),
        };
        self.divider_drag = Some(DividerDrag {
            workspace_id,
            divider_id,
            axis,
            origin,
            start_ratio: ratio,
            span,
        });
        cx.notify();
    }

    fn handle_divider_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.divider_drag else {
            return;
        };
        let pos = match drag.axis {
            SplitAxis::Horizontal => f32::from(position.x),
            SplitAxis::Vertical => f32::from(position.y),
        };
        let new_ratio = drag.start_ratio + (pos - drag.origin) / drag.span.max(1.0);
        if let Some(workspace) = self.workspace_mut(drag.workspace_id) {
            if let Some(layout) = workspace.layout.as_mut() {
                layout.set_ratio(drag.divider_id, new_ratio);
            }
        }
        cx.notify();
    }

    fn handle_divider_drag_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.divider_drag.take().is_none() {
            return;
        }
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    fn send_input_bytes_broadcast(
        &mut self,
        pane_id: u64,
        data: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        let broadcast_targets: Vec<u64> = self
            .workspace_id_for_pane(pane_id)
            .and_then(|workspace_id| self.workspace(workspace_id))
            .filter(|workspace| workspace.broadcast_input && workspace.pane_ids.len() > 1)
            .map(|workspace| {
                workspace
                    .pane_ids
                    .iter()
                    .copied()
                    .filter(|id| *id != pane_id)
                    .collect()
            })
            .unwrap_or_default();

        let primary = self.send_input_bytes(pane_id, data.clone(), cx);
        for target in broadcast_targets {
            self.send_input_bytes(target, data.clone(), cx);
        }
        primary
    }

    fn open_pane_context_menu(
        &mut self,
        pane_id: u64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_pane(pane_id, window, cx);
        self.pane_context_menu = Some((pane_id, position));
        cx.stop_propagation();
        cx.notify();
    }

    fn clear_pane_screen(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.terminal.reset_scrollback();
        }
        if !self
            .pane(pane_id)
            .map(|pane| pane.connected)
            .unwrap_or(false)
        {
            cx.notify();
            return;
        }
        let bytes = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
        let _ = self.send_input_bytes(pane_id, bytes, cx);
        self.status_message = "Terminal cleared.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    /// Merge every pane from `source_workspace_id` into the workspace that owns
    /// `target_pane_id`, as split panes. `zone` decides the split axis and
    /// whether the merged panes land before or after the target pane.
    fn merge_tab_as_split(
        &mut self,
        source_workspace_id: u64,
        target_pane_id: u64,
        zone: DropZone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_workspace_id) = self.workspace_id_for_pane(target_pane_id) else {
            return;
        };
        if source_workspace_id == target_workspace_id {
            self.split_drop_target = None;
            cx.notify();
            return;
        }
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == source_workspace_id)
        else {
            return;
        };
        let Some(source_layout) = self.workspaces[source_index].layout.clone() else {
            return;
        };
        let source_count = source_layout.leaf_ids().len();
        let target_count = self
            .workspace(target_workspace_id)
            .map(|workspace| workspace.pane_ids.len())
            .unwrap_or(0);
        if target_count + source_count > MAX_SPLIT_PANES {
            self.error_message = format!("Split panes are capped at {MAX_SPLIT_PANES} for now.");
            self.split_drop_target = None;
            cx.notify();
            return;
        }
        let axis = match zone {
            DropZone::Left | DropZone::Right => SplitAxis::Horizontal,
            DropZone::Top | DropZone::Bottom => SplitAxis::Vertical,
        };
        let new_first = matches!(zone, DropZone::Left | DropZone::Top);
        let first_source_pane = source_layout.first_leaf();
        // Drop the source workspace tab, but keep its panes alive — they are
        // re-parented into the target workspace, not closed.
        self.workspaces
            .retain(|workspace| workspace.id != source_workspace_id);
        if let Some(target) = self.workspace_mut(target_workspace_id) {
            let inserted = target
                .layout
                .as_mut()
                .map(|layout| layout.split_leaf(target_pane_id, &source_layout, axis, new_first))
                .unwrap_or(false);
            if !inserted {
                target.layout = Some(source_layout);
            }
            target.sync_pane_ids();
            target.active_pane_id = first_source_pane;
            target.view_mode = WorkspaceViewMode::Terminal;
        }
        self.active_workspace_id = Some(target_workspace_id);
        self.split_drop_target = None;
        self.status_message = "Merged tab into a split.".to_string();
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(first_source_pane) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn move_pane_to_new_workspace(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.workspace_id_for_pane(pane_id) else {
            return;
        };
        let single_pane = self
            .workspace(workspace_id)
            .map(|workspace| workspace.pane_ids.len() <= 1)
            .unwrap_or(true);
        if single_pane {
            self.error_message = "This pane is already in its own workspace.".to_string();
            cx.notify();
            return;
        }
        let title = self
            .pane(pane_id)
            .map(|pane| pane.title.clone())
            .unwrap_or_default();

        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.layout = workspace
                .layout
                .take()
                .and_then(|layout| layout.without_pane(pane_id));
            workspace.sync_pane_ids();
            if workspace
                .sftp
                .as_ref()
                .is_some_and(|browser| browser.pane_id == pane_id)
            {
                workspace.sftp = None;
                workspace.view_mode = WorkspaceViewMode::Terminal;
            }
        }

        let new_workspace_id = self.next_workspace_id();
        self.workspaces.push(WorkspaceTab {
            id: new_workspace_id,
            title,
            pane_ids: vec![pane_id],
            active_pane_id: pane_id,
            unread_events: 0,
            layout: Some(SplitNode::Leaf(pane_id)),
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: None,
            pending_connect_mode: ConnectDialogMode::Username,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });
        self.active_workspace_id = Some(new_workspace_id);
        self.status_message = "Pane detached into a new workspace tab.".to_string();
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn duplicate_workspace_in_new_tab(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .workspace(workspace_id)
            .and_then(|workspace| self.pane(workspace.active_pane_id))
            .map(|pane| pane.request.clone())
        else {
            return;
        };
        let Some((_, pane_id)) = self.open_request_workspace(request.clone(), window, cx) else {
            return;
        };
        self.open_workspace_tab_menu = None;
        self.status_message = format!("Duplicating {} in a new tab...", request.address());
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn split_workspace_horizontally(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_workspace(workspace_id, window, cx);
        self.open_workspace_tab_menu = None;
        self.split_active_workspace(SplitAxis::Horizontal, window, cx);
    }

    fn start_multiplayer_for_workspace(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        self.open_workspace_tab_menu = None;
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.broadcast_input = true;
        }
        self.status_message =
            "Multiplayer-style broadcast input started for this workspace.".to_string();
        self.error_message.clear();
        cx.notify();
    }

    fn toggle_workspace_broadcast(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        let mut now_on = false;
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.broadcast_input = !workspace.broadcast_input;
            now_on = workspace.broadcast_input;
        }
        self.status_message = if now_on {
            "Broadcasting input to every pane in this workspace.".to_string()
        } else {
            "Broadcast input disabled.".to_string()
        };
        self.error_message.clear();
        cx.notify();
    }

    fn send_input_bytes(&mut self, pane_id: u64, data: Vec<u8>, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.pane_mut(pane_id) else {
            return false;
        };
        if !pane.connected {
            return false;
        }

        let mut notify = false;
        if pane.terminal.scrollback() > 0 {
            pane.terminal.reset_scrollback();
            notify = true;
        }

        if pane
            .runtime
            .command_tx
            .send(SessionCommand::Input(data.clone()))
            .is_err()
        {
            self.error_message = "Unable to send input to the SSH runtime.".to_string();
            cx.notify();
            return false;
        }

        self.record_command_input(pane_id, &data);
        self.error_message.clear();
        if notify {
            cx.notify();
        }
        true
    }

    fn record_command_input(&mut self, pane_id: u64, data: &[u8]) {
        let mut completed_commands = Vec::new();
        let mut input_changed = false;
        let Some((scope_key, scope_label, alternate_screen)) = self.pane(pane_id).map(|pane| {
            (
                pane.request.history_scope_key(),
                pane.request.history_scope_label(),
                pane.terminal.alternate_screen(),
            )
        }) else {
            return;
        };
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };

        if alternate_screen {
            pane.current_input.clear();
            pane.selected_autocomplete_index = None;
            return;
        }

        if data.starts_with(b"\x1b") {
            return;
        }

        for &byte in data {
            match byte {
                b'\r' | b'\n' => {
                    let command = pane.current_input.trim().to_string();
                    if !command.is_empty() {
                        let explicit_disconnect = pane.request.kind == ConnectionKind::Ssh
                            && matches!(command.split_whitespace().next(), Some("exit" | "logout"));
                        if shell_command_requires_continuation(&command) {
                            if !pane.current_input.ends_with('\n') {
                                pane.current_input.push('\n');
                            }
                        } else if !pane.current_input.contains('\n') {
                            if explicit_disconnect {
                                pane.user_closed = true;
                                pane.auto_reconnect_at = None;
                            }
                            completed_commands.push(command);
                            pane.current_input.clear();
                        } else {
                            pane.current_input.clear();
                        }
                    } else {
                        pane.current_input.clear();
                    }
                    input_changed = true;
                }
                0x08 | 0x7f => {
                    input_changed |= pane.current_input.pop().is_some();
                }
                0x03 | 0x04 | 0x15 | 0x1b => {
                    input_changed |= !pane.current_input.is_empty();
                    pane.current_input.clear();
                }
                b'\t' => {}
                byte if byte.is_ascii_control() => {}
                _ => {
                    pane.current_input.push(byte as char);
                    input_changed = true;
                }
            }
        }

        if input_changed {
            pane.selected_autocomplete_index = None;
        }

        for command in completed_commands {
            self.saved
                .record_command_history_for_scope(&command, &scope_key, &scope_label);
        }
        let _ = save_saved_state(&self.saved);
    }

    fn workspace_autocomplete_candidates(&self) -> Vec<AutocompleteCandidate> {
        let Some(pane) = self.active_pane() else {
            return Vec::new();
        };
        if !pane.connected
            || pane.closed
            || pane.terminal.alternate_screen()
            || pane.current_input.contains('\n')
            || pane.current_input.trim().is_empty()
        {
            return Vec::new();
        }

        let path_context = self
            .active_workspace()
            .map(|workspace| PathSuggestionContext {
                current_path: workspace
                    .sftp
                    .as_ref()
                    .map(|browser| browser.current_path.clone())
                    .or_else(|| pane.request.startup_directory.clone()),
                startup_directory: pane.request.startup_directory.clone(),
                entries: workspace
                    .sftp
                    .as_ref()
                    .map(|browser| browser.entries.clone())
                    .unwrap_or_default(),
            });
        let output_context = self
            .active_workspace()
            .map(|workspace| OutputSuggestionContext {
                current_path: workspace
                    .sftp
                    .as_ref()
                    .map(|browser| browser.current_path.clone())
                    .or_else(|| pane.request.startup_directory.clone()),
                recent_lines: pane_recent_output_lines(pane, 80),
            });

        collect_autocomplete_candidates(
            &pane.current_input,
            &self.saved.command_history,
            &self.saved.scoped_command_history,
            &pane.request.history_scope_key(),
            &self.saved.snippets,
            path_context.as_ref(),
            output_context.as_ref(),
        )
    }

    fn command_palette_query(&self, cx: &App) -> String {
        self.shell_inputs
            .command_palette
            .read(cx)
            .value()
            .trim()
            .to_string()
    }

    fn command_palette_candidates(&self, cx: &App) -> Vec<CommandPaletteCandidate> {
        let Some(pane) = self.active_pane() else {
            return Vec::new();
        };

        collect_command_palette_candidates(
            &self.command_palette_query(cx),
            &self.saved.command_history,
            &self.saved.scoped_command_history,
            &pane.request.history_scope_key(),
            &self.saved.snippets,
            Some(&OutputSuggestionContext {
                current_path: self
                    .active_workspace()
                    .and_then(|workspace| workspace.sftp.as_ref())
                    .map(|browser| browser.current_path.clone())
                    .or_else(|| pane.request.startup_directory.clone()),
                recent_lines: pane_recent_output_lines(pane, 80),
            }),
        )
    }

    fn selected_command_palette_index(&self, candidate_count: usize) -> usize {
        self.selected_command_palette_index
            .min(candidate_count.saturating_sub(1))
    }

    fn move_command_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let candidates = self.command_palette_candidates(cx);
        if candidates.is_empty() {
            return false;
        }

        self.selected_command_palette_index =
            (self.selected_command_palette_index(candidates.len()) as isize + delta)
                .rem_euclid(candidates.len() as isize) as usize;
        cx.notify();
        true
    }

    fn run_selected_command_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let candidates = self.command_palette_candidates(cx);
        if candidates.is_empty() {
            return false;
        }
        let candidate = &candidates[self.selected_command_palette_index(candidates.len())];
        if self.run_command_in_active_pane(
            &candidate.command,
            "Command sent to the active session.",
            cx,
        ) {
            self.close_command_palette(window, cx);
            return true;
        }
        false
    }

    fn selected_autocomplete_index(&self, candidate_count: usize) -> usize {
        self.active_pane()
            .and_then(|pane| pane.selected_autocomplete_index)
            .filter(|index| *index < candidate_count)
            .unwrap_or(0)
    }

    fn move_autocomplete_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let candidates = self.workspace_autocomplete_candidates();
        if candidates.is_empty() {
            return false;
        }

        let next = (self.selected_autocomplete_index(candidates.len()) as isize + delta)
            .rem_euclid(candidates.len() as isize) as usize;
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            return false;
        };
        let Some(pane) = self.pane_mut(pane_id) else {
            return false;
        };
        pane.selected_autocomplete_index = Some(next);
        cx.notify();
        true
    }

    fn accept_selected_autocomplete(&mut self, cx: &mut Context<Self>) -> bool {
        let candidates = self.workspace_autocomplete_candidates();
        if candidates.is_empty() {
            return false;
        }
        let candidate = candidates[self.selected_autocomplete_index(candidates.len())].clone();
        self.apply_autocomplete_candidate(&candidate.command, candidate.source, cx)
    }

    fn apply_autocomplete_candidate(
        &mut self,
        command: &str,
        source: AutocompleteSource,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            return false;
        };
        let Some(current_input) = self.active_pane().map(|pane| pane.current_input.clone()) else {
            return false;
        };
        if current_input.trim().is_empty() || current_input.trim() == command {
            return false;
        }

        let mut bytes = vec![0x7f; current_input.chars().count()];
        bytes.extend_from_slice(command.as_bytes());
        if self.send_input_bytes(pane_id, bytes, cx) {
            self.status_message = format!("Autocomplete applied from {}.", source.label());
            self.error_message.clear();
            cx.notify();
            return true;
        }
        false
    }

    fn toggle_workspace_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let mut search_visible = false;
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.search_visible = !workspace.search_visible;
            search_visible = workspace.search_visible;
            if !workspace.search_visible {
                workspace.search_results.clear();
                workspace.active_search_index = None;
                workspace.search_query.clear();
            }
        }

        if search_visible {
            self.show_command_palette = false;
            self.selected_command_palette_index = 0;
            self.set_command_palette_input("", window, cx);
            self.focus_terminal_search(window, cx);
        } else {
            self.set_terminal_search_input("", window, cx);
            if let Some(pane) = self.active_pane() {
                pane.terminal_focus.focus(window);
            }
        }
        self.sync_terminal_layout(window, cx);
        cx.notify();
    }

    fn refresh_workspace_search(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace(workspace_id) else {
            return;
        };
        let active_pane_id = workspace.active_pane_id;
        let query = self
            .shell_inputs
            .terminal_search
            .read(cx)
            .value()
            .trim()
            .to_string();

        let rows = self
            .pane(active_pane_id)
            .map(|pane| pane.terminal.all_rows_text())
            .unwrap_or_default();
        let results = search_rows(&rows, &query);

        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.search_query = query;
            workspace.search_results = results;
            workspace.active_search_index = if workspace.search_results.is_empty() {
                None
            } else {
                Some(
                    workspace
                        .active_search_index
                        .unwrap_or(0)
                        .min(workspace.search_results.len() - 1),
                )
            };
        }
    }

    fn jump_workspace_search(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let (pane_id, result) = {
            let Some(workspace) = self.workspace_mut(workspace_id) else {
                return;
            };
            if workspace.search_results.is_empty() {
                return;
            }

            let current = workspace.active_search_index.unwrap_or(0) as i32;
            let len = workspace.search_results.len() as i32;
            let next = (current + delta).rem_euclid(len) as usize;
            workspace.active_search_index = Some(next);
            (workspace.active_pane_id, workspace.search_results[next])
        };

        self.reveal_search_match(pane_id, result, cx);
        cx.notify();
    }

    fn reveal_search_match(&mut self, pane_id: u64, result: SearchMatch, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        let viewport_rows = usize::from(pane.terminal.size().rows.max(1));
        let max_scrollback = pane.terminal.max_scrollback();
        let total_rows = max_scrollback + viewport_rows;
        let max_start = total_rows.saturating_sub(viewport_rows);
        let desired_start = result
            .full_row
            .saturating_sub(viewport_rows.saturating_div(2))
            .min(max_start);
        let scrollback = max_scrollback.saturating_sub(desired_start);
        pane.terminal.set_scrollback(scrollback);
        pane.selection = None;
        pane.dragging_selection = false;
        let _ = cx;
    }

    fn scroll_active_pane_top(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_pane_mut() {
            let max_scrollback = pane.terminal.max_scrollback();
            pane.terminal.set_scrollback(max_scrollback);
            cx.notify();
        }
    }

    fn scroll_active_pane_bottom(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_pane_mut() {
            pane.terminal.reset_scrollback();
            cx.notify();
        }
    }

    fn copy_active_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selection) = self.active_pane().and_then(|pane| pane.selection) else {
            return false;
        };
        let Some(pane) = self.active_pane() else {
            return false;
        };
        let Some(selection) = normalized_selection(selection) else {
            return false;
        };
        let text = pane.terminal.contents_between(
            selection.anchor.row,
            selection.anchor.col,
            selection.head.row,
            selection.head.col,
        );
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status_message = "Selection copied to clipboard.".to_string();
        self.error_message.clear();
        true
    }

    fn paste_to_active_pane(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            return false;
        };
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let mut text = clipboard.text().unwrap_or_default();
        if text.is_empty() {
            return false;
        }
        text = text.replace("\r\n", "\n");

        if self.saved.settings.confirm_multiline_paste && text.contains('\n') {
            self.pending_paste = Some(PendingPaste {
                pane_id,
                text: text.clone(),
            });
            let line_count = text.matches('\n').count() + 1;
            self.status_message =
                format!("{line_count} lines on the clipboard. Confirm to send to the active pane.");
            self.error_message.clear();
            cx.notify();
            return true;
        }

        self.send_paste_bytes(pane_id, text, cx)
    }

    fn send_paste_bytes(&mut self, pane_id: u64, text: String, cx: &mut Context<Self>) -> bool {
        let mut bytes = Vec::new();
        if self
            .pane(pane_id)
            .map(|pane| pane.terminal.bracketed_paste())
            .unwrap_or(false)
        {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.as_bytes());
        }

        self.send_input_bytes_broadcast(pane_id, bytes, cx)
    }

    fn confirm_pending_paste(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_paste.take() else {
            return false;
        };
        let result = self.send_paste_bytes(pending.pane_id, pending.text, cx);
        if result {
            self.status_message = "Multi-line paste delivered.".to_string();
        }
        cx.notify();
        result
    }

    fn cancel_pending_paste(&mut self, cx: &mut Context<Self>) {
        if self.pending_paste.take().is_some() {
            self.status_message = "Paste cancelled.".to_string();
            self.error_message.clear();
            cx.notify();
        }
    }

    fn handle_terminal_key(
        &mut self,
        pane_id: u64,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.modifiers.secondary() {
            match event.keystroke.key.as_str() {
                "c" => {
                    if self.copy_active_selection(cx) {
                        return true;
                    }
                }
                "v" => {
                    if self.paste_to_active_pane(cx) {
                        return true;
                    }
                }
                "f" => {
                    self.toggle_workspace_search(window, cx);
                    return true;
                }
                "k" => {
                    self.toggle_command_palette(window, cx);
                    return true;
                }
                "w" => {
                    if let Some(workspace_id) = self.active_workspace_id {
                        self.close_workspace(workspace_id, cx);
                        return true;
                    }
                }
                _ => {}
            }
        }

        let Some(pane) = self.pane(pane_id) else {
            return false;
        };
        if !pane.connected {
            return false;
        }

        if event.keystroke.modifiers.shift {
            let rows = i32::from(pane.terminal.size().rows.max(1));
            match event.keystroke.key.as_str() {
                "pageup" => {
                    if let Some(pane) = self.pane_mut(pane_id) {
                        pane.terminal.scroll_scrollback(rows);
                    }
                    cx.notify();
                    return true;
                }
                "pagedown" => {
                    if let Some(pane) = self.pane_mut(pane_id) {
                        pane.terminal.scroll_scrollback(-rows);
                    }
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }

        if event.keystroke.modifiers.platform {
            return false;
        }

        let application_cursor = self
            .pane(pane_id)
            .map(|pane| pane.terminal.application_cursor())
            .unwrap_or(false);
        let Some(bytes) = encode_terminal_input(&event.keystroke, application_cursor) else {
            return false;
        };

        self.send_input_bytes_broadcast(pane_id, bytes, cx)
    }

    fn activate_pane(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.pane_workspace_id(pane_id) {
            if let Some(workspace) = self.workspace_mut(workspace_id) {
                workspace.active_pane_id = pane_id;
            }
            self.active_workspace_id = Some(workspace_id);
        }
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    fn mouse_cell_position(
        &self,
        pane_id: u64,
        position: Point<Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<TerminalCellPos> {
        let layout = self.pane_layout_for(pane_id, window, cx)?;
        let pos_x: f32 = position.x.into();
        let pos_y: f32 = position.y.into();
        let min_x = layout.cell_x;
        let min_y = layout.cell_y;
        let max_x = min_x + layout.cell_width;
        let max_y = min_y + layout.cell_height;
        let x = pos_x.clamp(min_x, max_x - 1.0);
        let y = pos_y.clamp(min_y, max_y - 1.0);
        let col = ((x - min_x) / layout.char_width).floor().max(0.0) as u16;
        let row = ((y - min_y) / layout.line_height).floor().max(0.0) as u16;

        Some(TerminalCellPos {
            row: row.min(layout.rows.saturating_sub(1)),
            col: col.min(layout.cols.saturating_sub(1)),
        })
    }

    fn pane_uses_mouse_reporting(&self, pane_id: u64) -> bool {
        self.pane(pane_id)
            .map(|pane| pane.terminal.mouse_protocol_mode() != MouseProtocolMode::None)
            .unwrap_or(false)
    }

    fn handle_pane_mouse_down(
        &mut self,
        pane_id: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_pane(pane_id, window, cx);

        if self.pane_uses_mouse_reporting(pane_id) {
            if let Some(data) = self.mouse_report_bytes(
                pane_id,
                event.position,
                MouseEventKind::Press,
                event.modifiers,
                window,
                cx,
            ) {
                let _ = self.send_input_bytes(pane_id, data, cx);
            }
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }

        let Some(pos) = self.mouse_cell_position(pane_id, event.position, window, cx) else {
            return;
        };

        if event.click_count == 2 {
            self.select_word_at(pane_id, pos, cx);
            return;
        }

        if let Some(pane) = self.pane_mut(pane_id) {
            pane.selection = Some(SelectionRange {
                anchor: pos,
                head: pos,
            });
            pane.dragging_selection = true;
        }
        cx.notify();
    }

    fn handle_pane_mouse_move(
        &mut self,
        pane_id: u64,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.divider_drag.is_some() {
            return;
        }
        if self.pane_uses_mouse_reporting(pane_id) {
            if let Some(data) = self.mouse_report_bytes(
                pane_id,
                event.position,
                MouseEventKind::Move {
                    dragging: event.dragging(),
                },
                event.modifiers,
                window,
                cx,
            ) {
                let _ = self.send_input_bytes(pane_id, data, cx);
            }
            return;
        }

        if !event.dragging() {
            return;
        }

        let Some(pos) = self.mouse_cell_position(pane_id, event.position, window, cx) else {
            return;
        };
        if let Some(pane) = self.pane_mut(pane_id) {
            if let Some(selection) = pane.selection.as_mut() {
                selection.head = pos;
                pane.dragging_selection = true;
                cx.notify();
            }
        }
    }

    fn handle_pane_mouse_up(
        &mut self,
        pane_id: u64,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_uses_mouse_reporting(pane_id) {
            if let Some(data) = self.mouse_report_bytes(
                pane_id,
                event.position,
                MouseEventKind::Release,
                event.modifiers,
                window,
                cx,
            ) {
                let _ = self.send_input_bytes(pane_id, data, cx);
            }
            return;
        }

        let copy_on_select = self.saved.settings.copy_on_select;
        let mut copy_text: Option<String> = None;
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.dragging_selection = false;
            if let Some(selection) = pane.selection {
                if selection.anchor == selection.head {
                    pane.selection = None;
                }
            }
            if copy_on_select {
                if let Some(selection) = pane.selection.and_then(normalized_selection) {
                    let text = pane.terminal.contents_between(
                        selection.anchor.row,
                        selection.anchor.col,
                        selection.head.row,
                        selection.head.col,
                    );
                    if !text.is_empty() {
                        copy_text = Some(text);
                    }
                }
            }
        }
        if let Some(text) = copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    fn handle_pane_scroll(
        &mut self,
        pane_id: u64,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_uses_mouse_reporting(pane_id) {
            if let Some(data) = self.mouse_report_bytes(
                pane_id,
                event.position,
                MouseEventKind::Wheel { delta: event.delta },
                event.modifiers,
                window,
                cx,
            ) {
                let _ = self.send_input_bytes(pane_id, data, cx);
            }
            return;
        }

        let line_height = px(self.terminal_font_size() * TERMINAL_LINE_HEIGHT);
        let delta = event.delta.pixel_delta(line_height);
        let delta_y: f32 = delta.y.into();
        let line_height_px: f32 = line_height.into();
        let lines = (delta_y / line_height_px).round() as i32;
        if lines != 0 {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.terminal.scroll_scrollback(lines);
            }
        }
        cx.notify();
    }

    fn mouse_report_bytes(
        &self,
        pane_id: u64,
        position: Point<Pixels>,
        kind: MouseEventKind,
        modifiers: Modifiers,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Vec<u8>> {
        let pane = self.pane(pane_id)?;
        let mode = pane.terminal.mouse_protocol_mode();
        let encoding = pane.terminal.mouse_protocol_encoding();
        let pos = self.mouse_cell_position(pane_id, position, window, cx)?;
        encode_mouse_report(mode, encoding, kind, pos, modifiers)
    }

    fn select_word_at(&mut self, pane_id: u64, pos: TerminalCellPos, cx: &mut Context<Self>) {
        let row_text = self
            .pane(pane_id)
            .and_then(|pane| pane.terminal.visible_row_text(pos.row))
            .unwrap_or_default();
        let chars = row_text.chars().collect::<Vec<_>>();
        if chars.is_empty() {
            return;
        }
        let mut index = usize::from(pos.col.min(chars.len().saturating_sub(1) as u16));
        while index > 0 && chars.get(index).is_none() {
            index -= 1;
        }
        let is_word = chars.get(index).copied().map(is_word_char).unwrap_or(false);
        let mut start = index;
        let mut end = index + 1;
        while start > 0 && chars.get(start - 1).copied().map(is_word_char) == Some(is_word) {
            start -= 1;
        }
        while end < chars.len() && chars.get(end).copied().map(is_word_char) == Some(is_word) {
            end += 1;
        }

        if let Some(pane) = self.pane_mut(pane_id) {
            pane.selection = Some(SelectionRange {
                anchor: TerminalCellPos {
                    row: pos.row,
                    col: start as u16,
                },
                head: TerminalCellPos {
                    row: pos.row,
                    col: end.saturating_sub(1) as u16,
                },
            });
            pane.dragging_selection = false;
        }
        cx.notify();
    }

    fn workspace_visible_matches(
        &self,
        workspace: &WorkspaceTab,
        pane: &SessionPane,
    ) -> Vec<(usize, SearchMatch, bool)> {
        let snapshot_rows = pane.terminal.snapshot().rows.len();
        let visible_start = pane.terminal.visible_row_start();
        let visible_end = visible_start + snapshot_rows;

        workspace
            .search_results
            .iter()
            .enumerate()
            .filter_map(|(index, result)| {
                if (visible_start..visible_end).contains(&result.full_row) {
                    Some((
                        result.full_row - visible_start,
                        *result,
                        workspace.active_search_index == Some(index),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    fn render_saved_group_cards(&self, cx: &Context<Self>) -> Option<Div> {
        if self.saved.host_groups.is_empty() {
            return None;
        }

        Some(
            v_flex()
                .w_full()
                .gap_3()
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child("Saved Groups"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(
                                    "Select a group, target it for bulk assignment, or load its defaults into the editor.",
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_wrap()
                        .gap_3()
                        .children(self.saved.host_groups.iter().enumerate().map(|(index, group)| {
                            let group_name = group.label.clone();
                            let select_group_name = group.label.clone();
                            let bulk_group_name = group.label.clone();
                            let load_group_name = group.label.clone();
                            let (visible_count, total_count) = self.group_host_counts(&group.label, cx);
                            let mut chips = Vec::new();
                            if let Some(username) = group.username.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("User: {}", username),
                                        theme::library_bg(),
                                        theme::slate(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if !group.tags.is_empty() {
                                chips.push(
                                    self.status_badge(
                                        format!("Tags: {}", group.tags.join(", ")),
                                        theme::library_bg(),
                                        theme::slate(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if let Some(identity_id) = group.identity_id.as_deref() {
                                if let Some(identity) = self.identity_by_id(identity_id) {
                                    chips.push(
                                        self.status_badge(
                                            format!("Identity: {}", identity.label),
                                            theme::library_bg(),
                                            theme::success(),
                                        )
                                        .into_any_element(),
                                    );
                                }
                            }
                            if let Some(jump_host_id) = group.jump_host_id.as_deref() {
                                if let Some(jump_host) = self.jump_host_display_name(jump_host_id) {
                                    chips.push(
                                        self.status_badge(
                                            format!("Jump: {}", jump_host),
                                            theme::library_bg(),
                                            theme::accent(),
                                        )
                                        .into_any_element(),
                                    );
                                }
                            }
                            if let Some(directory) = group.startup_directory.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("Dir: {}", directory),
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if let Some(command) = group.startup_command.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("Cmd: {}", command),
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if !group.port_forward_rules.is_empty() {
                                chips.push(
                                    self.status_badge(
                                        if group.port_forward_rules.len() == 1 {
                                            format!(
                                                "Forward: {}",
                                                group.port_forward_rules[0].display_name()
                                            )
                                        } else {
                                            format!("{} Forwards", group.port_forward_rules.len())
                                        },
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }

                            v_flex()
                                .id(("saved-group-card", index))
                                .min_w(px(HOST_CARD_WIDTH))
                                .max_w(px(HOST_CARD_WIDTH * 1.3))
                                .flex_1()
                                .gap_3()
                                .p_4()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .font_semibold()
                                                .text_color(theme::text_main())
                                                .child(group_name),
                                        )
                                        .child(self.status_badge(
                                            if visible_count == total_count {
                                                format!("{} hosts", total_count)
                                            } else {
                                                format!("{} visible • {} total", visible_count, total_count)
                                            },
                                            theme::library_bg(),
                                            theme::text_muted(),
                                        )),
                                )
                                .when(!chips.is_empty(), |this| {
                                    this.child(h_flex().gap_2().flex_wrap().children(chips))
                                })
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(
                                            Button::new(("saved-group-select", index))
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label("Select Hosts")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_filtered_group_hosts(
                                                        &select_group_name,
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .child(
                                            Button::new(("saved-group-bulk", index))
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::AccentSoft,
                                                    cx,
                                                ))
                                                .label("Use as Bulk")
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.prepare_bulk_group_assignment(
                                                        &bulk_group_name,
                                                        window,
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .child(
                                            Button::new(("saved-group-load", index))
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label("Load Defaults")
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    Self::set_input_value(
                                                        &this.inputs.group,
                                                        load_group_name.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                    this.apply_group_defaults_to_editor(window, cx);
                                                })),
                                        ),
                                )
                                .into_any_element()
                        })),
                ),
        )
    }

    fn render_identity_picker(&self, cx: &Context<Self>) -> Div {
        let selected_path = self.current_key_path(cx);
        let identities = self.saved.identities.clone();

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child("Saved identities"),
            )
            .when(identities.is_empty(), |this| {
                this.child(
                    div()
                        .p_3()
                        .rounded(px(12.))
                        .bg(theme::with_alpha(theme::hover(), 0.72))
                        .border_1()
                        .border_color(theme::border())
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child("No saved identities yet. Add a key file or use one imported at launch."),
                )
            })
            .when(!identities.is_empty(), |this| {
                this.child(
                    v_flex()
                        .max_h(px(148.))
                        .overflow_y_scrollbar()
                        .gap_2()
                        .children(identities.iter().enumerate().map(
                            |(index, identity)| {
                                let display_identity = identity.clone();
                                let click_identity = identity.clone();
                                let is_selected = display_identity.key_path == selected_path;

                                h_flex()
                                    .id(("editor-identity", index))
                                    .w_full()
                                    .justify_between()
                                    .items_center()
                                    .gap_2()
                                    .p_2()
                                    .rounded(px(12.))
                                    .bg(if is_selected {
                                        theme::accent_soft()
                                    } else {
                                        theme::with_alpha(theme::hover(), 0.72)
                                    })
                                    .border_1()
                                    .border_color(if is_selected {
                                        theme::with_alpha(theme::accent(), 0.42)
                                    } else {
                                        theme::border()
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.use_identity(&click_identity, window, cx);
                                    }))
                                    .child(
                                        v_flex()
                                            .gap(px(1.))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(display_identity.label.clone()),
                                                    )
                                                    .when(index == 0, |this| {
                                                        this.child(self.status_badge(
                                                            "Default",
                                                            theme::library_card(),
                                                            theme::accent(),
                                                        ))
                                                    })
                                                    .when(
                                                        display_identity.source
                                                            == crate::models::IdentitySource::Imported,
                                                        |this| {
                                                            this.child(self.status_badge(
                                                                "Imported",
                                                                theme::library_card(),
                                                                theme::slate(),
                                                            ))
                                                        },
                                                    )
                                                    .when(is_selected, |this| {
                                                        this.child(self.status_badge(
                                                            "Selected",
                                                            theme::library_card(),
                                                            theme::success(),
                                                        ))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted())
                                                    .child(display_identity.kind.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(theme::text_muted())
                                                    .child(display_identity.key_path.clone()),
                                            ),
                                    )
                                    .into_any_element()
                            },
                        )),
                )
            })
    }

    fn render_vault_picker(
        &self,
        selected_vault_id: Option<&str>,
        on_select: impl Fn(String, &mut Self, &mut Window, &mut Context<Self>) + 'static + Clone,
        cx: &Context<Self>,
    ) -> Div {
        let selected_vault_id = self.effective_vault_id(selected_vault_id);
        let vaults = self.saved.vaults.clone();

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child("Vault"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(vaults.iter().enumerate().map(|(index, vault)| {
                        let is_selected = vault.id == selected_vault_id;
                        let click_id = vault.id.clone();
                        let click = on_select.clone();

                        div()
                            .id(("vault-pill", index))
                            .px_3()
                            .py(px(7.))
                            .rounded(px(999.))
                            .bg(if is_selected {
                                theme::accent_soft()
                            } else {
                                theme::with_alpha(theme::hover(), 0.72)
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::with_alpha(theme::accent(), 0.42)
                            } else {
                                theme::border()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::hover()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                click(click_id.clone(), this, window, cx);
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(if is_selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(vault.display_name()),
                                    )
                                    .when(vault.kind == VaultKind::Shared, |this| {
                                        this.child(self.status_badge(
                                            vault.kind.label(),
                                            theme::library_bg(),
                                            theme::slate(),
                                        ))
                                    }),
                            )
                            .into_any_element()
                    })),
            )
    }

    fn render_editor_panel(&self, cx: &Context<Self>) -> Div {
        let auth_mode = self.draft_auth_mode;
        let group_name = self.inputs.group.read(cx).value().trim().to_string();
        let saved_group = self.host_group_by_label(&group_name).cloned();
        let selected_identity = self
            .draft_identity_id
            .as_deref()
            .and_then(|identity_id| self.identity_by_id(identity_id))
            .cloned()
            .or_else(|| {
                self.identity_for_key_path(&self.current_key_path(cx))
                    .cloned()
            });
        let has_stored_password = self
            .selected_profile_id
            .as_ref()
            .and_then(|profile_id| {
                self.saved
                    .profiles
                    .iter()
                    .find(|item| &item.id == profile_id)
                    .and_then(|profile| profile.password_credential_id.as_ref())
            })
            .is_some();

        v_flex()
            .w_full()
            .gap_4()
            .child(self.form_field("Label", Input::new(&self.inputs.label)))
            .child(self.form_field(
                "Description",
                Input::new(&self.inputs.description),
            ))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Color tag"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                div()
                                    .id("draft-color-clear")
                                    .px_3()
                                    .py(px(6.))
                                    .rounded(px(999.))
                                    .border_1()
                                    .border_color(if self.draft_color_tag.is_none() {
                                        theme::accent()
                                    } else {
                                        theme::border()
                                    })
                                    .bg(if self.draft_color_tag.is_none() {
                                        theme::accent_soft()
                                    } else {
                                        theme::with_alpha(theme::hover(), 0.6)
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .text_size(px(13.))
                                    .text_color(theme::text_main())
                                    .child("Auto")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.draft_color_tag = None;
                                        cx.notify();
                                    })),
                            )
                            .children(HostColorTag::all().into_iter().enumerate().map(
                                |(index, tag)| {
                                    let active = self.draft_color_tag == Some(tag);
                                    let color: Hsla = gpui::rgb(tag.rgb_hex()).into();
                                    div()
                                        .id(("draft-color-tag", index))
                                        .h(px(28.))
                                        .px_3()
                                        .gap_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(999.))
                                        .border_1()
                                        .border_color(if active { color } else { theme::border() })
                                        .bg(if active {
                                            theme::with_alpha(color, 0.18)
                                        } else {
                                            theme::with_alpha(theme::hover(), 0.6)
                                        })
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme::hover()))
                                        .child(
                                            div().size(px(12.)).rounded(px(999.)).bg(color),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(theme::text_main())
                                                .child(tag.label()),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.draft_color_tag = Some(tag);
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                "Tags drive the avatar tint on host cards and the status dot on connected panes — handy for prod / staging / dev color-coding.",
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Library priority"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .children([false, true].into_iter().map(|favorite| {
                                let active = self.draft_profile_favorite == favorite;
                                Button::new(("draft-profile-favorite", favorite as usize))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if favorite { "Starred" } else { "Standard" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_draft_profile_favorite(favorite, cx);
                                    }))
                                    .into_any_element()
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                "Starred hosts stay pinned to the top of the library for your most-used machines.",
                            ),
                    ),
            )
            .child(self.render_vault_picker(
                self.draft_vault_id.as_deref(),
                |vault_id, this, _, cx| {
                    this.draft_vault_id = Some(vault_id.clone());
                    this.selected_vault_id = Some(vault_id.clone());
                    this.status_message =
                        format!("Assigning this host to {}.", this.effective_vault_name(Some(&vault_id)));
                    this.error_message.clear();
                    cx.notify();
                },
                cx,
            ))
            .child(self.form_field("Group", Input::new(&self.inputs.group)))
            .when(!group_name.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted())
                                        .child(format!("Group defaults for '{}'", group_name)),
                                )
                                .when(saved_group.is_some(), |this| {
                                    this.child(self.status_badge(
                                        "Saved",
                                        theme::library_bg(),
                                        theme::success(),
                                    ))
                                })
                                .when(saved_group.is_none(), |this| {
                                    this.child(self.status_badge(
                                        "Ad hoc",
                                        theme::library_bg(),
                                        theme::text_muted(),
                                    ))
                                }),
                        )
                        .when_some(saved_group.clone(), |this, group| {
                            let mut chips = Vec::new();
                            if let Some(username) = group.username.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("User: {}", username),
                                        theme::library_bg(),
                                        theme::slate(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if !group.tags.is_empty() {
                                chips.push(
                                    self.status_badge(
                                        format!("Tags: {}", group.tags.join(", ")),
                                        theme::library_bg(),
                                        theme::slate(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if let Some(identity_id) = group.identity_id.as_deref() {
                                if let Some(identity) = self.identity_by_id(identity_id) {
                                    chips.push(
                                        self.status_badge(
                                            format!("Identity: {}", identity.label),
                                            theme::library_bg(),
                                            theme::success(),
                                        )
                                        .into_any_element(),
                                    );
                                }
                            }
                            if let Some(jump_host_id) = group.jump_host_id.as_deref() {
                                if let Some(jump_host) = self.jump_host_display_name(jump_host_id) {
                                    chips.push(
                                        self.status_badge(
                                            format!("Jump: {}", jump_host),
                                            theme::library_bg(),
                                            theme::accent(),
                                        )
                                        .into_any_element(),
                                    );
                                }
                            }
                            if let Some(directory) = group.startup_directory.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("Dir: {}", directory),
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if let Some(command) = group.startup_command.as_deref() {
                                chips.push(
                                    self.status_badge(
                                        format!("Cmd: {}", command),
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }
                            if !group.port_forward_rules.is_empty() {
                                let label = if group.port_forward_rules.len() == 1 {
                                    format!("Forward: {}", group.port_forward_rules[0].display_name())
                                } else {
                                    format!("{} Forwards", group.port_forward_rules.len())
                                };
                                chips.push(
                                    self.status_badge(
                                        label,
                                        theme::library_bg(),
                                        theme::warning(),
                                    )
                                    .into_any_element(),
                                );
                            }

                            this.when(!chips.is_empty(), |this| {
                                this.child(h_flex().gap_2().flex_wrap().children(chips))
                            })
                        })
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("group-defaults-save")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::AccentSoft,
                                            cx,
                                        ))
                                        .label("Save Group Defaults")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_group_defaults_from_draft(cx);
                                        })),
                                )
                                .when(saved_group.is_some(), |this| {
                                    this.child(
                                        Button::new("group-defaults-apply")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Neutral,
                                                cx,
                                            ))
                                            .label("Load Defaults")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.apply_group_defaults_to_editor(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("group-defaults-remove")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Danger,
                                                cx,
                                            ))
                                            .label("Delete Defaults")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.remove_group_defaults(cx);
                                            })),
                                    )
                                }),
                        )
                        .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                    "Blank host fields can inherit username, tags, identity, jump host, startup settings, and saved forwarding rules from the group defaults.",
                                ),
                        ),
                )
            })
            .child(self.form_field("Tags", Input::new(&self.inputs.tags)))
            .child(self.form_field("Jump Host", Input::new(&self.inputs.jump_host)))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Startup"),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                self.form_field(
                                    "Remote Directory",
                                    Input::new(&self.inputs.startup_directory),
                                )
                                .flex_1(),
                            )
                            .child(
                                self.form_field(
                                    "Startup Command",
                                    Input::new(&self.inputs.startup_command),
                                )
                                .flex_1(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                "When the SSH shell opens, the app can change into a saved directory and optionally run one startup command.",
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(self.form_field(
                        "Environment",
                        Input::new(&self.inputs.environment),
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                "One KEY=value per line. Variables are exported into the remote shell before the startup directory and command run, with proper single-quote escaping.",
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Session"),
                    )
                    .child(
                        h_flex()
                            .p(px(3.))
                            .rounded(px(8.))
                            .bg(theme::hover())
                            .child(
                                div()
                                    .id("connect-view-terminal")
                                    .flex_1()
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .text_size(px(14.))
                                    .font_medium()
                                    .cursor_pointer()
                                    .when(!self.draft_start_in_files, |this| {
                                        this.bg(theme::library_card())
                                            .shadow_sm()
                                            .text_color(theme::text_main())
                                    })
                                    .when(self.draft_start_in_files, |this| {
                                        this.text_color(theme::text_muted())
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_draft_connect_view(false, cx);
                                    }))
                                    .child("Open Terminal"),
                            )
                            .child(
                                div()
                                    .id("connect-view-files")
                                    .flex_1()
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .text_size(px(14.))
                                    .font_medium()
                                    .cursor_pointer()
                                    .when(self.draft_start_in_files, |this| {
                                        this.bg(theme::library_card())
                                            .shadow_sm()
                                            .text_color(theme::text_main())
                                    })
                                    .when(!self.draft_start_in_files, |this| {
                                        this.text_color(theme::text_muted())
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_draft_connect_view(true, cx);
                                    }))
                                    .child("Open Files"),
                            ),
                    )
                    .child(
                        self.form_field(
                            "Scrollback Rows",
                            Input::new(&self.inputs.terminal_scrollback_rows),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(
                                "Choose whether this host lands in Terminal or Files after connect, and set how many terminal rows to keep in local scrollback.",
                            ),
                    ),
            )
            .child(self.form_field("Host", Input::new(&self.inputs.host)))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        self.form_field("Port", Input::new(&self.inputs.port))
                            .flex_1(),
                    )
                    .child(
                        self.form_field("Username", Input::new(&self.inputs.username))
                            .flex_1(),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Port Forwarding Rules"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child("Save local tunnels, remote reverse tunnels, or a dynamic SOCKS5 proxy and launch them automatically with the host."),
                    )
                    .child(
                        h_flex()
                            .p(px(3.))
                            .rounded(px(8.))
                            .bg(theme::hover())
                            .children(
                                [
                                    PortForwardKind::Local,
                                    PortForwardKind::Remote,
                                    PortForwardKind::Dynamic,
                                ]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, kind)| {
                                        let active = self.draft_port_forward_kind == kind;
                                        Button::new(("forward-kind", index))
                                            .small()
                                            .custom(Self::segmented_button_style(active, cx))
                                            .label(kind.label())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_draft_port_forward_kind(kind, cx);
                                            }))
                                            .into_any_element()
                                    }),
                            ),
                    )
                    .when(!self.draft_port_forward_rules.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_2()
                                .children(
                                    self.draft_port_forward_rules
                                        .iter()
                                        .cloned()
                                        .enumerate()
                                        .map(|(index, rule)| {
                                            let remove_index = index;
                                            h_flex()
                                                .id(("draft-forward-rule", index))
                                                .items_center()
                                                .justify_between()
                                                .gap_2()
                                                .px_3()
                                                .py(px(8.))
                                                .rounded(px(10.))
                                                .bg(theme::with_alpha(theme::hover(), 0.78))
                                                .border_1()
                                                .border_color(theme::border())
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(self.status_badge(
                                                            rule.kind().label(),
                                                            theme::library_bg(),
                                                            theme::accent(),
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
                                                                .font_medium()
                                                                .text_color(theme::text_main())
                                                                .child(rule.display_name()),
                                                        ),
                                                )
                                                .child(
                                                    Button::new(("remove-forward-rule", index))
                                                        .small()
                                                        .custom(Self::action_button_style(
                                                            theme::ActionTone::Danger,
                                                            cx,
                                                        ))
                                                        .label("Remove")
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                this.remove_draft_port_forward_rule(
                                                                    remove_index,
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                                .into_any_element()
                                        }),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                self.form_field(
                                    if self.draft_port_forward_kind == PortForwardKind::Remote {
                                        "Local Target Port"
                                    } else {
                                        "Local Port"
                                    },
                                    Input::new(&self.inputs.forward_local_port),
                                )
                                    .flex_1(),
                            )
                            .when(
                                self.draft_port_forward_kind != PortForwardKind::Dynamic,
                                |this| {
                                    this.child(
                                        self.form_field(
                                            if self.draft_port_forward_kind == PortForwardKind::Remote {
                                                "Remote Bind Host"
                                            } else {
                                                "Remote Host"
                                            },
                                            Input::new(&self.inputs.forward_remote_host),
                                        )
                                        .flex_1(),
                                    )
                                    .child(
                                        self.form_field(
                                            if self.draft_port_forward_kind == PortForwardKind::Remote {
                                                "Remote Bind Port"
                                            } else {
                                                "Remote Port"
                                            },
                                            Input::new(&self.inputs.forward_remote_port),
                                        )
                                        .flex_1(),
                                    )
                                },
                            )
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("add-forward-rule")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Add Rule")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_draft_port_forward_rule(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(match self.draft_port_forward_kind {
                                        PortForwardKind::Local => {
                                            "Local rules bind 127.0.0.1 and forward to a specific remote host and port."
                                        }
                                        PortForwardKind::Remote => {
                                            "Remote rules open a server-side listening port and send connections back to local 127.0.0.1 on the target port."
                                        }
                                        PortForwardKind::Dynamic => {
                                            "Dynamic rules expose a local SOCKS5 proxy port for ad hoc tunneling through the SSH session."
                                        }
                                    }),
                            ),
                    )
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Auth"),
                    )
                    .child(
                        h_flex()
                            .p(px(3.))
                            .rounded(px(8.))
                            .bg(theme::hover())
                            .child(
                                div()
                                    .id("auth-password")
                                    .flex_1()
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .text_size(px(14.))
                                    .font_medium()
                                    .cursor_pointer()
                                    .when(auth_mode == AuthMode::Password, |this| {
                                        this.bg(theme::library_card())
                                            .shadow_sm()
                                            .text_color(theme::text_main())
                                    })
                                    .when(auth_mode != AuthMode::Password, |this| {
                                        this.text_color(theme::text_muted())
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_auth_mode(AuthMode::Password, cx);
                                    }))
                                    .child("Password"),
                            )
                            .child(
                                div()
                                    .id("auth-key")
                                    .flex_1()
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .text_size(px(14.))
                                    .font_medium()
                                    .cursor_pointer()
                                    .when(auth_mode == AuthMode::PrivateKey, |this| {
                                        this.bg(theme::library_card())
                                            .shadow_sm()
                                            .text_color(theme::text_main())
                                    })
                                    .when(auth_mode != AuthMode::PrivateKey, |this| {
                                        this.text_color(theme::text_muted())
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_auth_mode(AuthMode::PrivateKey, cx);
                                        let _ = this.ensure_default_identity_selected(window, cx);
                                    }))
                                    .child("Private Key"),
                            ),
                    ),
            )
            .when(auth_mode == AuthMode::Password, |this| {
                this.child(v_flex().gap_2().child(self.form_field(
                    "Password",
                    Input::new(&self.inputs.password).mask_toggle(),
                ))
                .when(has_stored_password, |this| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::success())
                            .child("A saved password is already available from the system credential store."),
                    )
                }))
            })
            .when(auth_mode == AuthMode::PrivateKey, |this| {
                this.child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_medium()
                                .text_color(theme::text_main())
                                .child("Private key"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Input::new(&self.inputs.key_path).flex_1())
                                .child(
                                    Button::new("pick-key-file")
                                        .small()
                                        .ghost()
                                        .icon(IconName::FolderOpen)
                                        .label("Browse")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick_key_file(window, cx);
                                        })),
                                ),
                        )
                        .when_some(selected_identity, |this, identity| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::success())
                                    .child(format!(
                                        "Selected identity: {} ({})",
                                        identity.label, identity.kind
                                    )),
                            )
                        })
                        .child(self.form_field(
                            "Passphrase",
                            Input::new(&self.inputs.key_passphrase).mask_toggle(),
                        ))
                        .child(self.render_identity_picker(cx)),
                )
            })
    }

    fn form_field(&self, label: &str, input: Input) -> Div {
        let label = label.to_string();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(label),
            )
            .child(input)
    }

    fn render_library_empty_state<E: IntoElement>(
        &self,
        icon: E,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
    ) -> Div {
        let title = title.into();
        let description = description.into();
        v_flex()
            .items_center()
            .justify_center()
            .max_w(px(520.))
            .mx_auto()
            .p_8()
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .gap_3()
            .child(
                div()
                    .size(px(56.))
                    .rounded(px(18.))
                    .bg(theme::with_alpha(theme::accent(), 0.08))
                    .border_1()
                    .border_color(theme::with_alpha(theme::accent(), 0.2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .text_center()
                    .child(description),
            )
    }

    fn render_workspace_empty_state<E: IntoElement>(
        &self,
        icon: E,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
    ) -> Div {
        let title = title.into();
        let description = description.into();
        v_flex()
            .items_center()
            .justify_center()
            .max_w(px(520.))
            .mx_auto()
            .p_8()
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::terminal_panel())
            .border_1()
            .border_color(theme::border_dark())
            .gap_3()
            .child(
                div()
                    .size(px(56.))
                    .rounded(px(18.))
                    .bg(theme::with_alpha(theme::accent(), 0.12))
                    .border_1()
                    .border_color(theme::with_alpha(theme::accent(), 0.28))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .font_semibold()
                    .text_color(theme::text_on_dark())
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted_dark())
                    .text_center()
                    .child(description),
            )
    }

    fn render_hosts_onboarding(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        if !self.should_show_onboarding() {
            return None;
        }

        let imported_hosts = self.imported_host_count();
        let saved_hosts = self.user_host_count();
        let identities = self.saved.identities.len();
        let snippets = self.saved.snippets.len();
        let title = if imported_hosts > 0 || identities > 0 {
            "Welcome to your host library"
        } else {
            "Start your SSH workspace"
        };
        let description = if imported_hosts > 0 || identities > 0 {
            format!(
                "We found {} imported hosts and {} reusable identities from {}. Search, quick connect, or save local hosts to start building your workspace.",
                imported_hosts,
                identities,
                ssh_directory_label()
            )
        } else {
            format!(
                "Save your first host, add a key from {}, or open a local terminal while you build the library.",
                ssh_directory_label()
            )
        };

        Some(
            v_flex()
                .w_full()
                .gap_3()
                .p_5()
                .rounded(px(theme::CARD_RADIUS))
                .bg(theme::library_card())
                .border_1()
                .border_color(theme::with_alpha(theme::accent(), 0.25))
                .child(
                    h_flex()
                        .justify_between()
                        .items_start()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::SquareTerminal)
                                                .size(px(16.))
                                                .text_color(theme::accent()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(18.))
                                                .font_semibold()
                                                .text_color(theme::text_main())
                                                .child(title),
                                        ),
                                )
                                .child(
                                    div()
                                        .max_w(px(760.))
                                        .text_size(px(13.))
                                        .line_height(relative(1.55))
                                        .text_color(theme::text_muted())
                                        .child(description),
                                ),
                        )
                        .child(
                            Button::new("hosts-onboarding-dismiss")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dismiss_onboarding(cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(self.status_badge(
                            format!("{saved_hosts} saved"),
                            theme::library_bg(),
                            theme::text_muted(),
                        ))
                        .child(self.status_badge(
                            format!("{imported_hosts} imported"),
                            theme::library_bg(),
                            theme::accent(),
                        ))
                        .child(self.status_badge(
                            format!("{identities} identities"),
                            theme::library_bg(),
                            theme::success(),
                        ))
                        .child(self.status_badge(
                            format!("{snippets} snippets"),
                            theme::library_bg(),
                            theme::warning(),
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("hosts-onboarding-new")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("New Host")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_editor_for_new_host(window, cx);
                                })),
                        )
                        .child(
                            Button::new("hosts-onboarding-key")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .icon(IconName::FolderOpen)
                                .label("Add Key File")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_key_file(window, cx);
                                    this.nav_section = NavSection::Hosts;
                                    this.show_editor_panel = true;
                                    this.draft_auth_mode = AuthMode::PrivateKey;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("hosts-onboarding-local")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::AccentSoft,
                                    cx,
                                ))
                                .label("Local Terminal")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_local_terminal(window, cx);
                                })),
                        )
                        .when(imported_hosts > 0 || saved_hosts > 0, |this| {
                            this.child(
                                Button::new("hosts-onboarding-search")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Focus Search")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.focus_host_search(window, cx);
                                        this.status_message = "Host search focused.".to_string();
                                        this.error_message.clear();
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap_4()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Quick connect from search: `user@host` or `ssh user@host:port`"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "Shortcuts: {}+1..7 for sections, {}+L for host search, {}+K for command palette",
                                    primary_shortcut_label(),
                                    primary_shortcut_label(),
                                    primary_shortcut_label()
                                )),
                        ),
                ),
        )
    }

    fn render_recent_hosts_row(&self, cx: &Context<Self>) -> Option<Div> {
        let mut seen = HashSet::new();
        let mut recent: Vec<(HostProfile, u64)> = Vec::new();
        for log in self.saved.session_logs.iter().rev() {
            if log.started_at == 0 {
                continue;
            }
            if let Some(profile) = self.saved.profiles.iter().find(|profile| {
                profile.host == log.host
                    && profile.port == log.port
                    && profile.username == log.username
            }) {
                if seen.insert(profile.id.clone()) {
                    recent.push((profile.clone(), log.started_at));
                    if recent.len() >= 6 {
                        break;
                    }
                }
            }
        }
        if recent.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_medium()
                        .text_color(theme::text_muted())
                        .child("RECENT"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children(recent.into_iter().enumerate().map(|(index, (profile, _))| {
                            let profile_id = profile.id.clone();
                            let display = profile.display_name();
                            let chip_color = match profile.color_tag {
                                Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
                                None => theme::host_chip_color(&display),
                            };
                            h_flex()
                                .id(("recent-host", index))
                                .gap_2()
                                .items_center()
                                .px(px(10.))
                                .py(px(5.))
                                .rounded(px(8.))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::card_hover_subtle()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_editor_panel = false;
                                    this.load_profile_into_inputs(&profile_id, window, cx);
                                    this.connect_current(window, cx);
                                }))
                                .child(div().size(px(7.)).rounded(px(999.)).bg(chip_color))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_main())
                                        .child(display),
                                )
                                .into_any_element()
                        })),
                ),
        )
    }

    fn render_library_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.nav_section {
            NavSection::Hosts => self.render_hosts_view(window, cx).into_any_element(),
            NavSection::Sftp => self.render_sftp_view(cx).into_any_element(),
            NavSection::Vaults => self.render_vaults_view(cx).into_any_element(),
            NavSection::Keychain => self.render_keychain_view(cx).into_any_element(),
            NavSection::Snippets => self.render_snippets_view(cx).into_any_element(),
            NavSection::Settings => self.render_settings_view(cx).into_any_element(),
            NavSection::KnownHosts => self.render_known_hosts_view(cx).into_any_element(),
            NavSection::Logs => self.render_logs_view(cx).into_any_element(),
        }
    }

    fn render_library_shell(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let sidebar_visible = self.nav_section != NavSection::Sftp;
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .bg(theme::library_bg())
            .when(sidebar_visible, |this| {
                this.child(self.render_library_sidebar(cx))
            })
            .child(self.render_library_content(window, cx))
    }

    fn status_badge(
        &self,
        label: impl Into<SharedString>,
        background: Hsla,
        foreground: Hsla,
    ) -> Div {
        let label: SharedString = label.into();

        div()
            .px_2()
            .py_0p5()
            .rounded(px(999.))
            .bg(background)
            .border_1()
            .border_color(theme::with_alpha(foreground, 0.24))
            .text_size(px(12.))
            .font_medium()
            .text_color(foreground)
            .child(label)
    }

    fn action_button_style(tone: theme::ActionTone, cx: &App) -> ButtonCustomVariant {
        ButtonCustomVariant::new(cx)
            .color(theme::action_fill(tone))
            .foreground(theme::action_foreground(tone))
            .border(theme::action_border(tone))
            .hover(theme::action_hover(tone))
            .active(theme::action_active(tone))
    }

    fn segmented_button_style(active: bool, cx: &App) -> ButtonCustomVariant {
        Self::action_button_style(
            if active {
                theme::ActionTone::Accent
            } else {
                theme::ActionTone::Neutral
            },
            cx,
        )
    }
}

impl Render for TermiRustApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // When the active workspace changes, scroll the tab strip so the
        // active tab is visible. scroll_to_item keeps the request pending
        // until the tab has layout bounds, so this also works for a tab
        // created this very frame.
        if self.active_workspace_id != self.tab_strip_scrolled_to {
            self.tab_strip_scrolled_to = self.active_workspace_id;
            if let Some(active_id) = self.active_workspace_id {
                if let Some(index) = self
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == active_id)
                {
                    self.tab_strip_scroll.scroll_to_item(index);
                }
            }
        }

        let content = if self.active_workspace_id.is_some() {
            self.render_workspace_shell(window, cx).into_any_element()
        } else {
            self.render_library_shell(window, cx).into_any_element()
        };

        div()
            .size_full()
            .relative()
            .bg(theme::app_bg())
            .font_family(cx.theme().font_family.clone())
            .text_color(theme::text_main())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_global_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.divider_drag.is_some() {
                    this.handle_divider_drag_move(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    if this.divider_drag.is_some() {
                        this.handle_divider_drag_end(window, cx);
                    }
                    if this.split_drop_target.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(
                v_flex()
                    .size_full()
                    .child(self.render_top_chrome(window, cx))
                    .child(div().flex_1().min_h_0().flex().flex_col().child(content)),
            )
            .when(
                self.show_editor_panel
                    && !(self.active_workspace_id.is_none()
                        && self.nav_section == NavSection::Hosts),
                |this| this.child(self.render_editor_dialog(window, cx)),
            )
            .when(self.show_command_palette, |this| {
                this.child(self.render_command_palette(window, cx))
            })
            .when_some(self.open_workspace_tab_menu, |this, workspace_id| {
                this.child(self.render_workspace_tab_context_menu_layer(workspace_id, cx))
            })
            .when_some(self.pane_context_menu, |this, (pane_id, position)| {
                this.child(self.render_pane_context_menu_layer(pane_id, position, cx))
            })
    }
}

impl TermiRustApp {
    fn handle_command_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.modifiers.secondary() && event.keystroke.key.as_str() == "k" {
            self.close_command_palette(window, cx);
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_command_palette(window, cx);
                true
            }
            "up" => self.move_command_palette_selection(-1, cx),
            "down" => self.move_command_palette_selection(1, cx),
            "enter" => self.run_selected_command_palette(window, cx),
            _ => false,
        }
    }

    fn handle_global_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.show_command_palette {
            return false;
        }

        if event.keystroke.key.as_str() == "escape" {
            if self.tab_rename_workspace_id.is_some() {
                self.cancel_workspace_rename(window, cx);
                return true;
            }
            if self.pane_rename_id.is_some() {
                self.cancel_pane_rename(window, cx);
                return true;
            }
            if self.pending_snippet_prompts.is_some() {
                self.cancel_snippet_prompts(cx);
                return true;
            }
            if self.pending_paste.is_some() {
                self.cancel_pending_paste(cx);
                return true;
            }
            if self.show_editor_panel {
                self.close_editor_dialog(window, cx);
                return true;
            }
            if self
                .active_workspace()
                .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Files)
            {
                self.show_active_workspace_terminal(cx);
                return true;
            }
        }
        if event.keystroke.key.as_str() == "enter" && !event.keystroke.modifiers.secondary() {
            if self.tab_rename_workspace_id.is_some() {
                self.commit_workspace_rename(window, cx);
                return true;
            }
            if self.pane_rename_id.is_some() {
                self.commit_pane_rename(window, cx);
                return true;
            }
        }

        if !event.keystroke.modifiers.secondary() {
            return false;
        }

        if event.keystroke.modifiers.alt && self.workspaces.len() > 1 {
            match event.keystroke.key.as_str() {
                "right" | "tab" => {
                    if self.cycle_active_workspace(true, window, cx) {
                        return true;
                    }
                }
                "left" => {
                    if self.cycle_active_workspace(false, window, cx) {
                        return true;
                    }
                }
                _ => {}
            }
        }

        if event.keystroke.modifiers.shift {
            match event.keystroke.key.as_str() {
                "f" => {
                    if self.active_workspace_id.is_some() {
                        self.open_active_workspace_files(cx);
                        return true;
                    }
                }
                "t" => {
                    // Cmd+Shift+T: toggle Files/Terminal view
                    if self.active_workspace_id.is_some() {
                        if self.active_workspace().is_some_and(|workspace| {
                            workspace.view_mode == WorkspaceViewMode::Files
                        }) {
                            self.show_active_workspace_terminal(cx);
                        } else {
                            self.open_active_workspace_files(cx);
                        }
                    }
                    return true;
                }
                "b" => {
                    if let Some(workspace_id) = self.active_workspace_id {
                        self.toggle_workspace_broadcast(workspace_id, cx);
                        return true;
                    }
                }
                "l" => {
                    if let Some(pane_id) = self.active_pane().map(|pane| pane.id) {
                        self.clear_pane_screen(pane_id, cx);
                        return true;
                    }
                }
                _ => {}
            }
        }

        if !event.keystroke.modifiers.shift && event.keystroke.key.as_str() == "t" {
            self.open_local_terminal(window, cx);
            return true;
        }

        match event.keystroke.key.as_str() {
            "1" => {
                self.activate_library_section(NavSection::Hosts, window, cx);
                true
            }
            "2" => {
                self.activate_library_section(NavSection::Vaults, window, cx);
                true
            }
            "3" => {
                self.activate_library_section(NavSection::Keychain, window, cx);
                true
            }
            "4" => {
                self.activate_library_section(NavSection::Snippets, window, cx);
                true
            }
            "5" => {
                self.activate_library_section(NavSection::Settings, window, cx);
                true
            }
            "6" => {
                self.activate_library_section(NavSection::KnownHosts, window, cx);
                true
            }
            "7" => {
                self.activate_library_section(NavSection::Logs, window, cx);
                true
            }
            "," => {
                self.activate_library_section(NavSection::Settings, window, cx);
                true
            }
            "l" => {
                // Cmd+L: from library → focus host search; from workspace → go to logs
                if self.active_workspace_id.is_some() {
                    self.activate_library_section(NavSection::Logs, window, cx);
                    self.status_message = "Switched to Logs view.".to_string();
                } else if self.nav_section == NavSection::Logs {
                    self.activate_library_section(NavSection::Hosts, window, cx);
                    self.focus_host_search(window, cx);
                    self.status_message = "Host search focused.".to_string();
                } else {
                    self.activate_library_section(NavSection::Hosts, window, cx);
                    self.focus_host_search(window, cx);
                    self.status_message = "Host search focused.".to_string();
                }
                self.error_message.clear();
                cx.notify();
                true
            }
            "n" => {
                if self.active_workspace_id.is_none() {
                    self.activate_library(window, cx);
                    self.open_editor_for_new_host(window, cx);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn render_editor_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = if self.selected_profile_id.is_some() {
            "Host Details"
        } else {
            "New Host"
        };

        div()
            .id("editor-dialog-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::modal_scrim())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_editor_dialog(window, cx);
                }),
            )
            .child(
                v_flex()
                    .id("editor-dialog-card")
                    .w(px(460.))
                    .max_h(px(640.))
                    .overflow_y_scrollbar()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(12.))
                            .right(px(12.))
                            .id("editor-dialog-close")
                            .size(px(28.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(theme::hover())
                            .border_1()
                            .border_color(theme::border())
                            .hover(|style| style.bg(theme::card_hover()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor_dialog(window, cx);
                            }))
                            .child(
                                app_icon(ICON_X)
                                    .size(px(14.))
                                    .text_color(theme::text_main()),
                            ),
                    )
                    .child(
                        v_flex().px_5().pt_5().pb_2().child(
                            div()
                                .text_size(px(18.))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child(title),
                        ),
                    )
                    .child(
                        v_flex()
                            .px_5()
                            .pb_5()
                            .gap_4()
                            .child(self.render_editor_panel(cx))
                            .child(self.render_editor_actions(cx)),
                    ),
            )
    }
}

fn search_rows(rows: &[String], query: &str) -> Vec<SearchMatch> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    let needle = query.to_ascii_lowercase();
    let mut matches = Vec::new();

    for (row_ix, row) in rows.iter().enumerate() {
        let haystack = row.to_ascii_lowercase();
        let mut offset = 0usize;
        while let Some(index) = haystack[offset..].find(&needle) {
            let start = offset + index;
            let end = start + needle.len();
            matches.push(SearchMatch {
                full_row: row_ix,
                start_col: start,
                end_col: end,
            });
            offset = end;
        }
    }

    matches
}

fn nav_section_key(section: NavSection) -> u64 {
    match section {
        NavSection::Hosts => 0,
        NavSection::Vaults => 1,
        NavSection::Keychain => 2,
        NavSection::Snippets => 3,
        NavSection::Settings => 4,
        NavSection::KnownHosts => 5,
        NavSection::Logs => 6,
        NavSection::Sftp => 7,
    }
}

fn workspace_runtime_summary(
    indicators: WorkspaceIndicators,
) -> Option<(String, WorkspaceRuntimeTone)> {
    let mut parts = Vec::new();
    if indicators.error_panes > 0 {
        parts.push(format_count_label(
            indicators.error_panes,
            "Error",
            "Errors",
        ));
    }
    if indicators.connecting_panes > 0 {
        parts.push(format_count_label(
            indicators.connecting_panes,
            "Connecting",
            "Connecting",
        ));
    }
    if indicators.live_panes > 0 {
        parts.push(format_count_label(indicators.live_panes, "Live", "Live"));
    }
    if indicators.closed_panes > 0 {
        parts.push(format_count_label(
            indicators.closed_panes,
            "Closed",
            "Closed",
        ));
    }

    let tone = if indicators.error_panes > 0 {
        WorkspaceRuntimeTone::Error
    } else if indicators.connecting_panes > 0 {
        WorkspaceRuntimeTone::Connecting
    } else if indicators.live_panes > 0 {
        WorkspaceRuntimeTone::Live
    } else if indicators.closed_panes > 0 {
        WorkspaceRuntimeTone::Closed
    } else {
        return None;
    };

    Some((
        parts.into_iter().take(2).collect::<Vec<_>>().join(" • "),
        tone,
    ))
}

fn merge_port_forward_rules(
    current: &[PortForwardRule],
    inherited: &[PortForwardRule],
) -> Vec<PortForwardRule> {
    let mut merged = current.to_vec();
    for rule in inherited {
        if !merged.contains(rule) {
            merged.push(rule.clone());
        }
    }
    merged
}

fn draft_has_pending_forward_input(draft: &DraftProfile) -> bool {
    !draft.forward_local_port.trim().is_empty()
        || !draft.forward_remote_host.trim().is_empty()
        || !draft.forward_remote_port.trim().is_empty()
}

fn apply_group_defaults_to_draft(
    mut draft: DraftProfile,
    group: Option<&SavedHostGroup>,
    identities: &[SavedIdentity],
) -> DraftProfile {
    let Some(group) = group else {
        return draft;
    };

    if draft.username.trim().is_empty() {
        draft.username = group.username.clone().unwrap_or_default();
    }
    if draft.tags.trim().is_empty() && !group.tags.is_empty() {
        draft.tags = format_tag_values(&group.tags);
    }
    if draft.identity_id.is_none() {
        draft.identity_id = group.identity_id.clone();
    }
    if draft.key_path.trim().is_empty() {
        if let Some(identity_id) = draft.identity_id.as_deref() {
            if let Some(identity) = identities
                .iter()
                .find(|identity| identity.id == identity_id)
            {
                draft.key_path = identity.key_path.clone();
            }
        }
    }
    if draft.jump_host_id.is_none() {
        draft.jump_host_id = group.jump_host_id.clone();
    }
    if draft.startup_directory.trim().is_empty() {
        draft.startup_directory = group.startup_directory.clone().unwrap_or_default();
    }
    if draft.startup_command.trim().is_empty() {
        draft.startup_command = group.startup_command.clone().unwrap_or_default();
    }
    if draft.saved_port_forward_rules.is_empty()
        && !draft_has_pending_forward_input(&draft)
        && !group.port_forward_rules.is_empty()
    {
        draft.saved_port_forward_rules = group.port_forward_rules.clone();
    }

    draft
}

#[cfg(test)]
mod tests {
    use super::{
        AutocompleteSource, OutputSuggestionContext, PathSuggestionContext, TermiRustApp,
        WorkspaceIndicators, WorkspaceRuntimeTone, apply_group_defaults_to_draft,
        collect_autocomplete_candidates, collect_command_palette_candidates,
        extract_snippet_prompt_names, shell_command_requires_continuation,
        startup_bytes_for_request, substitute_snippet_placeholders, substitute_snippet_prompts,
        workspace_runtime_summary,
    };
    use crate::models::{
        AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DraftProfile, LocalPortForward,
        PortForwardKind, PortForwardRule, SavedHostGroup, SavedIdentity, SavedState,
    };
    use crate::sftp::RemoteFileEntry;
    use crate::test_support::{DockerSshServer, TestIsolation};
    use crate::ui::shell::shell_single_quote;
    use crate::ui::util::format_relative_time_for;
    use gpui::{AppContext as _, Entity, TestAppContext, WindowHandle};
    use gpui_component::Root;
    use std::time::{Duration, Instant};

    fn docker_ssh_request(server: &DockerSshServer) -> ConnectRequest {
        ConnectRequest {
            session_id: 0,
            title: "Docker SSH".to_string(),
            kind: ConnectionKind::Ssh,
            host: server.host().to_string(),
            port: server.port,
            username: server.username().to_string(),
            auth: Some(AuthConfig::Password {
                password: server.password().to_string(),
            }),
            jump_host: None,
            startup_directory: None,
            startup_command: Some("printf 'termirust-ui-ready\\n'".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    fn open_test_app(cx: &mut TestAppContext) -> (Entity<TermiRustApp>, WindowHandle<Root>) {
        let mut app_entity = None;
        let window = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
            cx.open_window(Default::default(), |window, cx| {
                let app = cx.new(|cx| TermiRustApp::new(SavedState::default(), window, cx));
                app_entity = Some(app.clone());
                cx.new(|cx| Root::new(app, window, cx))
            })
            .unwrap()
        });

        (app_entity.expect("app entity should exist"), window)
    }

    fn wait_for_app_state<R>(
        cx: &mut TestAppContext,
        app: &Entity<TermiRustApp>,
        timeout: Duration,
        mut check: impl FnMut(&mut TermiRustApp) -> Option<R>,
    ) -> R {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = app.update(cx, |app, cx| {
                app.process_events(cx);
                check(app)
            }) {
                return result;
            }

            if Instant::now() >= deadline {
                let terminal_dump = app.read_with(cx, |app, _| {
                    app.panes
                        .iter()
                        .map(|pane| {
                            format!(
                                "pane {} status={} connected={} closed={} rows={:?}",
                                pane.id,
                                pane.status,
                                pane.connected,
                                pane.closed,
                                pane.terminal.all_rows_text()
                            )
                        })
                        .collect::<Vec<_>>()
                });
                panic!("timed out waiting for app state: {terminal_dump:#?}");
            }

            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn snippet_prompts_extract_unique_named_placeholders() {
        let cmd = "echo {{?Name}} and {{?Place}} again {{?Name}} {{?  Place }}";
        let names = extract_snippet_prompt_names(cmd);
        assert_eq!(names, vec!["Name".to_string(), "Place".to_string()]);
    }

    #[test]
    fn snippet_prompt_substitution_fills_named_values() {
        let cmd = "kubectl --context {{?Cluster}} get pods -n {{?Namespace}}";
        let values = vec![
            ("Cluster".to_string(), "prod-east".to_string()),
            ("Namespace".to_string(), "payments".to_string()),
        ];
        assert_eq!(
            substitute_snippet_prompts(cmd, &values),
            "kubectl --context prod-east get pods -n payments"
        );
        // Missing values become empty strings, unbalanced braces preserved.
        assert_eq!(
            substitute_snippet_prompts("a {{?X}} b {{?Y}} c {{", &[]),
            "a  b  c {{"
        );
    }

    #[test]
    fn snippet_placeholders_substitute_known_names_only() {
        let request = ConnectRequest {
            session_id: 1,
            title: "Production".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 2222,
            username: "deploy".to_string(),
            auth: None,
            jump_host: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };

        assert_eq!(
            substitute_snippet_placeholders("ssh-copy-id {{USER}}@{{HOST}}", &request),
            "ssh-copy-id deploy@prod.example.com"
        );
        assert_eq!(
            substitute_snippet_placeholders("scp file {{USERNAME}}@{{ADDRESS}}:/tmp/", &request),
            "scp file deploy@prod.example.com:2222:/tmp/"
        );
        assert_eq!(
            substitute_snippet_placeholders("kubectl --context {{TITLE}} get pods", &request),
            "kubectl --context Production get pods"
        );
        // Unknown placeholders are left intact and unbalanced braces are preserved.
        assert_eq!(
            substitute_snippet_placeholders(
                "echo {{UNKNOWN}} value with {{HOST}} and tail {{",
                &request
            ),
            "echo {{UNKNOWN}} value with prod.example.com and tail {{"
        );
        // No placeholder inputs round-trip identically.
        assert_eq!(
            substitute_snippet_placeholders("docker ps -a", &request),
            "docker ps -a"
        );
    }

    #[test]
    fn relative_time_buckets_into_human_phrases() {
        let now = 1_700_000_000_000u64;
        let ms = |secs: u64| now - secs * 1000;
        assert_eq!(format_relative_time_for(ms(10), now), "just now");
        assert_eq!(format_relative_time_for(ms(59), now), "just now");
        assert_eq!(format_relative_time_for(ms(60), now), "1m ago");
        assert_eq!(format_relative_time_for(ms(60 * 5), now), "5m ago");
        assert_eq!(format_relative_time_for(ms(60 * 60), now), "1h ago");
        assert_eq!(format_relative_time_for(ms(60 * 60 * 23), now), "23h ago");
        assert_eq!(format_relative_time_for(ms(60 * 60 * 24), now), "yesterday");
        assert_eq!(
            format_relative_time_for(ms(60 * 60 * 24 * 3), now),
            "3d ago"
        );
        assert_eq!(
            format_relative_time_for(ms(60 * 60 * 24 * 7), now),
            "1w ago"
        );
        assert_eq!(
            format_relative_time_for(ms(60 * 60 * 24 * 31), now),
            "1mo ago"
        );
        assert_eq!(
            format_relative_time_for(ms(60 * 60 * 24 * 400), now),
            "1y ago"
        );
        assert_eq!(format_relative_time_for(0, now), "just now");
        assert_eq!(format_relative_time_for(now + 5_000, now), "just now");
    }

    #[test]
    fn shell_continuation_detects_unclosed_quotes_and_trailing_operators() {
        assert!(shell_command_requires_continuation("echo \"unterminated"));
        assert!(shell_command_requires_continuation("grep foo |"));
        assert!(shell_command_requires_continuation("echo hello \\"));
        assert!(shell_command_requires_continuation("if [ \"$x\" = \"y\""));
    }

    #[test]
    fn shell_continuation_accepts_complete_single_line_commands() {
        assert!(!shell_command_requires_continuation("git status"));
        assert!(!shell_command_requires_continuation("echo \"done\""));
        assert!(!shell_command_requires_continuation(
            "kubectl get pods | cat"
        ));
    }

    #[test]
    fn startup_actions_export_environment_before_cd_and_command() {
        let request = ConnectRequest {
            session_id: 1,
            title: "Prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth: None,
            jump_host: None,
            startup_directory: Some("/srv".to_string()),
            startup_command: Some("uptime".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: vec![
                ("AWS_PROFILE".to_string(), "prod".to_string()),
                ("MESSAGE".to_string(), "hello it's me".to_string()),
            ],
        };
        let bytes = startup_bytes_for_request(&request, None).unwrap();
        let script = String::from_utf8(bytes).unwrap();
        assert_eq!(
            script,
            "export AWS_PROFILE='prod'\nexport MESSAGE='hello it'\"'\"'s me'\ncd -- '/srv'\nuptime\n"
        );
    }

    #[test]
    fn startup_actions_build_cd_and_command_script() {
        let request = ConnectRequest {
            session_id: 1,
            title: "Prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth: Some(AuthConfig::Password {
                password: "secret".to_string(),
            }),
            jump_host: None,
            startup_directory: Some("/var/www/app's".to_string()),
            startup_command: Some("docker compose logs -f".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };

        let bytes = startup_bytes_for_request(&request, None).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "cd -- '/var/www/app'\"'\"'s'\ndocker compose logs -f\n"
        );
    }

    #[test]
    fn startup_actions_skip_empty_values() {
        let request = ConnectRequest {
            session_id: 1,
            title: "Prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth: None,
            jump_host: None,
            startup_directory: Some(" ".to_string()),
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };

        assert!(startup_bytes_for_request(&request, None).is_none());
        assert_eq!(shell_single_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn group_defaults_fill_blank_draft_values_without_overriding_explicit_fields() {
        let draft = DraftProfile {
            label: "Prod".to_string(),
            vault_id: None,
            favorite: false,
            group: "Operations".to_string(),
            tags: String::new(),
            host: "prod.example.com".to_string(),
            port: "22".to_string(),
            username: String::new(),
            password: String::new(),
            key_path: String::new(),
            identity_id: None,
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: "htop".to_string(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Local,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };
        let group = SavedHostGroup {
            label: "Operations".to_string(),
            vault_id: None,
            username: Some("ubuntu".to_string()),
            tags: vec!["prod".to_string(), "blue".to_string()],
            identity_id: Some("identity-ops".to_string()),
            jump_host_id: Some("bastion".to_string()),
            startup_directory: Some("/srv/app".to_string()),
            startup_command: Some("docker compose logs -f".to_string()),
            port_forward_rules: vec![PortForwardRule::Local {
                forward: LocalPortForward {
                    local_host: "127.0.0.1".to_string(),
                    local_port: 15432,
                    remote_host: "127.0.0.1".to_string(),
                    remote_port: 5432,
                },
            }],
        };
        let identities = vec![SavedIdentity {
            id: "identity-ops".to_string(),
            label: "ops-key".to_string(),
            vault_id: None,
            key_path: "/tmp/id_ops".to_string(),
            kind: "OpenSSH".to_string(),
            source: crate::models::IdentitySource::User,
        }];

        let resolved = apply_group_defaults_to_draft(draft, Some(&group), &identities);
        assert_eq!(resolved.username, "ubuntu");
        assert_eq!(resolved.tags, "prod, blue");
        assert_eq!(resolved.identity_id.as_deref(), Some("identity-ops"));
        assert_eq!(resolved.key_path, "/tmp/id_ops");
        assert_eq!(resolved.jump_host_id.as_deref(), Some("bastion"));
        assert_eq!(resolved.startup_directory, "/srv/app");
        assert_eq!(resolved.startup_command, "htop");
        assert_eq!(resolved.saved_port_forward_rules, group.port_forward_rules);
    }

    #[test]
    fn group_defaults_do_not_override_existing_tags_or_forward_rules() {
        let existing_rule = PortForwardRule::Local {
            forward: LocalPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port: 18080,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 8080,
            },
        };
        let inherited_rule = PortForwardRule::Local {
            forward: LocalPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port: 15432,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 5432,
            },
        };
        let draft = DraftProfile {
            label: "Prod".to_string(),
            vault_id: None,
            favorite: false,
            group: "Operations".to_string(),
            tags: "canary".to_string(),
            host: "prod.example.com".to_string(),
            port: "22".to_string(),
            username: "ubuntu".to_string(),
            password: String::new(),
            key_path: String::new(),
            identity_id: None,
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: vec![existing_rule.clone()],
            forward_kind: PortForwardKind::Local,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::Password,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };
        let group = SavedHostGroup {
            label: "Operations".to_string(),
            vault_id: None,
            username: Some("deploy".to_string()),
            tags: vec!["prod".to_string()],
            identity_id: None,
            jump_host_id: None,
            startup_directory: None,
            startup_command: None,
            port_forward_rules: vec![inherited_rule],
        };

        let resolved = apply_group_defaults_to_draft(draft, Some(&group), &[]);
        assert_eq!(resolved.username, "ubuntu");
        assert_eq!(resolved.tags, "canary");
        assert_eq!(resolved.saved_port_forward_rules, vec![existing_rule]);
    }

    #[test]
    fn path_autocomplete_suggests_remote_entries_for_path_commands() {
        let suggestions = collect_autocomplete_candidates(
            "cd lo",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            Some(&PathSuggestionContext {
                current_path: Some("/var/www".to_string()),
                startup_directory: Some("/srv/app".to_string()),
                entries: vec![
                    RemoteFileEntry {
                        name: "logs".to_string(),
                        path: "/var/www/logs".to_string(),
                        is_dir: true,
                        is_symlink: false,
                        size: None,
                    },
                    RemoteFileEntry {
                        name: "local.env".to_string(),
                        path: "/var/www/local.env".to_string(),
                        is_dir: false,
                        is_symlink: false,
                        size: Some(12),
                    },
                ],
            }),
            None,
        );

        assert_eq!(
            suggestions.first().map(|item| item.command.clone()),
            Some("cd logs/".to_string())
        );
        assert!(
            suggestions
                .iter()
                .all(|item| item.source == AutocompleteSource::Path)
        );
    }

    #[test]
    fn path_autocomplete_uses_startup_and_recent_paths_without_sftp_context() {
        let suggestions = collect_autocomplete_candidates(
            "cd /sr",
            &["tail -f /srv/app/log/app.log".to_string()],
            &[],
            "ssh:prod@example:22",
            &[],
            Some(&PathSuggestionContext {
                current_path: Some("/srv/app".to_string()),
                startup_directory: Some("/srv/app/current".to_string()),
                entries: Vec::new(),
            }),
            None,
        );

        assert!(
            suggestions
                .iter()
                .any(|item| item.command == "cd /srv/app/current/")
        );
        assert!(
            suggestions
                .iter()
                .any(|item| item.command == "cd /srv/app/")
        );
    }

    #[test]
    fn argument_autocomplete_suggests_command_templates_for_known_families() {
        let suggestions = collect_autocomplete_candidates(
            "git ch",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            None,
            None,
        );

        assert!(
            suggestions
                .iter()
                .any(|item| item.command == "git checkout main"
                    && item.source == AutocompleteSource::Argument)
        );
        assert!(
            suggestions
                .iter()
                .all(|item| item.source == AutocompleteSource::Argument)
        );
    }

    #[test]
    fn command_palette_uses_builtin_metadata_for_argument_templates() {
        let suggestions = collect_command_palette_candidates(
            "docker comp",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            None,
        );

        assert!(suggestions.iter().any(|item| {
            item.command == "docker compose logs -f"
                && item.source == AutocompleteSource::Argument
                && item.detail == "Stream logs for a compose project"
        }));
        assert!(suggestions.iter().any(|item| {
            item.command == "docker compose up -d"
                && item.detail == "Start compose services in the background"
        }));
    }

    #[test]
    fn workspace_runtime_summary_prioritizes_errors_and_connecting_states() {
        let summary = workspace_runtime_summary(WorkspaceIndicators {
            live_panes: 2,
            connecting_panes: 1,
            error_panes: 1,
            closed_panes: 0,
            split_count: 4,
            unread_events: 0,
        });

        assert_eq!(
            summary,
            Some((
                "1 Error • 1 Connecting".to_string(),
                WorkspaceRuntimeTone::Error
            ))
        );
    }

    #[test]
    fn workspace_runtime_summary_reports_live_and_closed_counts() {
        let summary = workspace_runtime_summary(WorkspaceIndicators {
            live_panes: 2,
            connecting_panes: 0,
            error_panes: 0,
            closed_panes: 1,
            split_count: 3,
            unread_events: 0,
        });

        assert_eq!(
            summary,
            Some(("2 Live • 1 Closed".to_string(), WorkspaceRuntimeTone::Live))
        );
    }

    #[test]
    fn context_autocomplete_uses_recent_git_output() {
        let suggestions = collect_autocomplete_candidates(
            "git checkout ma",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            None,
            Some(&OutputSuggestionContext {
                current_path: Some("/srv/app".to_string()),
                recent_lines: vec![
                    "On branch main".to_string(),
                    "  release/2026".to_string(),
                    "  feature/auth".to_string(),
                ],
            }),
        );

        assert!(suggestions.iter().any(|item| {
            item.command == "git checkout main"
                && item.source == AutocompleteSource::Context
                && item
                    .scope_label
                    .as_deref()
                    .is_some_and(|detail| detail.contains("Git branch"))
        }));
    }

    #[test]
    fn command_palette_uses_recent_output_context_for_kubernetes_targets() {
        let suggestions = collect_command_palette_candidates(
            "kubectl logs ap",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            Some(&OutputSuggestionContext {
                current_path: Some("/srv/app".to_string()),
                recent_lines: vec![
                    "NAME READY STATUS RESTARTS AGE".to_string(),
                    "api-7bcdf9d4d8-ptx2m 1/1 Running 0 4m".to_string(),
                    "worker-5cb88df4f7-cvt9k 1/1 Running 0 4m".to_string(),
                ],
            }),
        );

        assert!(suggestions.iter().any(|item| {
            item.command == "kubectl logs api-7bcdf9d4d8-ptx2m"
                && item.source == AutocompleteSource::Context
                && item.detail.contains("Kubernetes pod")
        }));
    }

    #[test]
    fn context_autocomplete_prefers_targets_matching_current_directory() {
        let suggestions = collect_autocomplete_candidates(
            "kubectl logs ",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            None,
            Some(&OutputSuggestionContext {
                current_path: Some("/srv/services/api".to_string()),
                recent_lines: vec![
                    "NAME READY STATUS RESTARTS AGE".to_string(),
                    "worker-5cb88df4f7-cvt9k 1/1 Running 0 4m".to_string(),
                    "api-7bcdf9d4d8-ptx2m 1/1 Running 0 4m".to_string(),
                ],
            }),
        );

        assert_eq!(
            suggestions.first().map(|item| item.command.as_str()),
            Some("kubectl logs api-7bcdf9d4d8-ptx2m")
        );
    }

    #[test]
    fn command_palette_prefers_context_targets_matching_current_directory() {
        let suggestions = collect_command_palette_candidates(
            "docker logs ",
            &[],
            &[],
            "ssh:prod@example:22",
            &[],
            Some(&OutputSuggestionContext {
                current_path: Some("/srv/services/worker".to_string()),
                recent_lines: vec![
                    "CONTAINER ID IMAGE COMMAND CREATED STATUS PORTS NAMES".to_string(),
                    "9c4bb3f2ad91 api:latest \"/entrypoint\" 2 minutes ago Up 2 minutes api"
                        .to_string(),
                    "8a4cb3e9dd12 worker:latest \"/entrypoint\" 2 minutes ago Up 2 minutes worker"
                        .to_string(),
                ],
            }),
        );

        assert_eq!(
            suggestions.first().map(|item| item.command.as_str()),
            Some("docker logs worker")
        );
    }

    #[gpui::test]
    fn e2e_ssh_workspace_connects_renders_output_and_closes(cx: &mut TestAppContext) {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping app ssh e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh fixture");
        let (app, window) = open_test_app(cx);
        let request = docker_ssh_request(&server);

        let (_, pane_id) = window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.open_request_workspace(request, window, cx)
                        .expect("workspace should open")
                })
            })
            .expect("window update should succeed");

        wait_for_app_state(cx, &app, Duration::from_secs(20), |app| {
            let pane = app.pane(pane_id)?;
            let ready = pane
                .terminal
                .all_rows_text()
                .iter()
                .any(|row| row.contains("termirust-ui-ready"));
            ready.then_some(())
        });

        window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, _| {
                    let pane = app.pane(pane_id).expect("pane should exist");
                    window.focus(&pane.terminal_focus);
                })
            })
            .expect("window focus update should succeed");

        cx.simulate_keystrokes(*window, "e c h o space a p p o k enter");

        wait_for_app_state(cx, &app, Duration::from_secs(10), |app| {
            let pane = app.pane(pane_id)?;
            pane.terminal
                .all_rows_text()
                .iter()
                .any(|row| row.contains("appok"))
                .then_some(())
        });

        cx.simulate_keystrokes(*window, "e x i t enter");

        wait_for_app_state(cx, &app, Duration::from_secs(10), |app| {
            let pane = app.pane(pane_id)?;
            (pane.closed && !pane.connected && pane.status == "Closed").then_some(())
        });

        let log_status = app.read_with(cx, |app, _| {
            let pane = app.pane(pane_id).expect("pane should still exist");
            app.saved
                .session_logs
                .iter()
                .find(|entry| entry.id == pane.log_id)
                .map(|entry| entry.status)
        });
        assert_eq!(
            log_status,
            Some(crate::models::SessionLogStatus::Disconnected)
        );
    }
}
