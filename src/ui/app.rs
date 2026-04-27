use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    ClipboardItem, InteractiveElement as _, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement as _,
    font, *,
};
use gpui_component::IconName;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, Disableable, Icon, Sizable, StyledExt as _, h_flex, v_flex};
use rfd::FileDialog;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::credentials;
use crate::local::spawn_local_session;
use crate::models::{
    AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DEFAULT_VAULT_ID, DraftProfile,
    HostColorTag, HostProfile, JumpHostConnection, PortForwardKind, PortForwardRule, ProfileSource,
    QuickConnect, SavedCommandHistoryEntry, SavedHostGroup, SavedIdentity, SavedSnippet,
    SavedState, SavedVault, SavedVaultMember, SavedWorkspace, SessionLogEntry, SessionLogStatus,
    SplitAxis, ThemePreset, VaultKind, VaultMemberRole,
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
use crate::terminal::{TerminalCell, TerminalRow, TerminalSize, TerminalState, TerminalStyle};
use crate::ui::theme;

const TERMINAL_LINE_HEIGHT: f32 = 1.3;
const LIBRARY_TOOLBAR_HEIGHT: f32 = 56.0;
const WORKSPACE_SEARCH_ROW_HEIGHT: f32 = 52.0;
const WORKSPACE_PADDING: f32 = 18.0;
const PANE_GAP: f32 = 12.0;
const PANE_HEADER_HEIGHT: f32 = 38.0;
const WORKSPACE_AUTOCOMPLETE_HEIGHT: f32 = 56.0;
const WORKSPACE_QUICK_ACTIONS_HEIGHT: f32 = 38.0;
const TERMINAL_INNER_PADDING_X: f32 = 20.0;
const TERMINAL_INNER_PADDING_Y: f32 = 14.0;
const MAX_SPLIT_PANES: usize = 4;
const HOST_CARD_WIDTH: f32 = 300.0;
const ICON_KEY: &str = "icons/key.svg";
const ICON_SHIELD_CHECK: &str = "icons/shield-check.svg";
const ICON_VAULT: &str = "icons/vault.svg";
const ICON_X: &str = "icons/x.svg";

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
            Self::Vaults => app_icon(ICON_VAULT),
            Self::Keychain => app_icon(ICON_KEY),
            Self::Snippets => IconName::BookOpen.into(),
            Self::Settings => IconName::Settings.into(),
            Self::KnownHosts => app_icon(ICON_SHIELD_CHECK),
            Self::Logs => IconName::BookOpen.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TerminalCellPos {
    row: u16,
    col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionRange {
    anchor: TerminalCellPos,
    head: TerminalCellPos,
}

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
            label: cx.new(|cx| InputState::new(window, cx).placeholder("New host label")),
            group: cx.new(|cx| InputState::new(window, cx).placeholder("Production / Staging")),
            tags: cx.new(|cx| InputState::new(window, cx).placeholder("prod, blue, kubernetes")),
            jump_host: cx.new(|cx| InputState::new(window, cx).placeholder("Optional saved host")),
            startup_directory: cx.new(|cx| InputState::new(window, cx).placeholder("/var/www/app")),
            startup_command: cx
                .new(|cx| InputState::new(window, cx).placeholder("docker compose logs -f")),
            terminal_scrollback_rows: cx.new(|cx| InputState::new(window, cx).placeholder("10000")),
            host: cx.new(|cx| InputState::new(window, cx).placeholder("user@hostname or IP")),
            port: cx.new(|cx| InputState::new(window, cx).default_value("22")),
            username: cx.new(|cx| InputState::new(window, cx).placeholder("root")),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Session-only password")
            }),
            key_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("Path to private key file")),
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
    split_axis: SplitAxis,
    view_mode: WorkspaceViewMode,
    sftp: Option<WorkspaceSftpState>,
    search_visible: bool,
    search_query: String,
    search_results: Vec<SearchMatch>,
    active_search_index: Option<usize>,
    broadcast_input: bool,
}

#[derive(Clone)]
struct WorkspaceTabDrag {
    workspace_id: u64,
    title: String,
}

struct WorkspaceTabDragPreview {
    title: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceRuntimeTone {
    Live,
    Connecting,
    Error,
    Closed,
}

#[derive(Clone)]
struct AutocompleteCandidate {
    command: String,
    source: AutocompleteSource,
    scope_label: Option<String>,
}

#[derive(Clone)]
struct CommandPaletteCandidate {
    command: String,
    title: String,
    detail: String,
    source: AutocompleteSource,
    pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteSource {
    Path,
    Context,
    Argument,
    History,
    Snippet,
    Builtin,
}

impl AutocompleteSource {
    fn priority(self) -> u8 {
        match self {
            Self::Path => 0,
            Self::Context => 1,
            Self::Argument => 2,
            Self::History => 3,
            Self::Snippet => 4,
            Self::Builtin => 5,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Context => "context",
            Self::Argument => "argument",
            Self::History => "history",
            Self::Snippet => "snippet",
            Self::Builtin => "builtin",
        }
    }
}

#[derive(Clone, Default)]
struct PathSuggestionContext {
    current_path: Option<String>,
    startup_directory: Option<String>,
    entries: Vec<RemoteFileEntry>,
}

#[derive(Clone, Default)]
struct OutputSuggestionContext {
    current_path: Option<String>,
    recent_lines: Vec<String>,
}

#[derive(Clone)]
struct ContextCommandTemplate {
    command: String,
    detail: String,
    rank: u8,
    ordinal: usize,
}

#[derive(Clone, Copy)]
struct BuiltinCommandTemplate {
    command: &'static str,
    detail: &'static str,
    source: AutocompleteSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AutocompleteMatchKind {
    Prefix,
    TokenPrefix,
    Substring,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WorkspaceViewMode {
    #[default]
    Terminal,
    Files,
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
    selected_command_palette_index: usize,
    tab_rename_workspace_id: Option<u64>,
    tab_rename_input: Entity<InputState>,
    pane_rename_id: Option<u64>,
    pane_rename_input: Entity<InputState>,
    pending_paste: Option<PendingPaste>,
    pending_snippet_prompts: Option<PendingSnippetPrompts>,
    sync_pull_force: bool,
    sync_pull_pending_warning: bool,
    _window_bounds_subscription: Option<Subscription>,
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
            selected_command_palette_index: 0,
            tab_rename_workspace_id: None,
            tab_rename_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            pane_rename_id: None,
            pane_rename_input: cx.new(|cx| InputState::new(window, cx).placeholder("Pane name")),
            pending_paste: None,
            pending_snippet_prompts: None,
            sync_pull_force: false,
            sync_pull_pending_warning: false,
            _window_bounds_subscription: None,
        };

        app.load_settings_inputs(window, cx);

        if app.saved.settings.restore_workspaces_on_launch {
            app.restore_saved_workspaces(window, cx);
        }

        if app.workspaces.is_empty() {
            if let Some(profile_id) = app.selected_profile_id.clone() {
                app.show_editor_panel = true;
                app.load_profile_into_inputs(&profile_id, window, cx);
            }
        }

        let window_bounds_subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_terminal_layout(window, cx);
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
        self.draft_identity_id = None;
        self.selected_profile_id = None;
        self.saved.selected_profile_id = None;
        self.draft_auth_mode = AuthMode::Password;
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
        if let Some(path) = FileDialog::new().pick_file() {
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
        groups
    }

    fn open_editor_for_new_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.nav_section = NavSection::Hosts;
        self.clear_profile_form(window, cx);
        self.show_editor_panel = true;
    }

    fn close_editor_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_editor_panel = false;
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
                panes.push(restorable);
            }

            if panes.is_empty() {
                continue;
            }

            let mut saved_workspace = SavedWorkspace {
                title: workspace.title.clone(),
                split_axis: workspace.split_axis,
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

            self.workspaces.push(WorkspaceTab {
                id: workspace_id,
                title,
                pane_ids,
                active_pane_id,
                unread_events: 0,
                split_axis: saved_workspace.split_axis,
                view_mode: WorkspaceViewMode::Terminal,
                sftp: None,
                search_visible: false,
                search_query: String::new(),
                search_results: Vec::new(),
                active_search_index: None,
                broadcast_input: false,
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
        let title = self
            .workspace(workspace_id)
            .map(|workspace| workspace.title.clone())
            .unwrap_or_default();
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
            if !password.is_empty() {
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
            self.next_session_id(),
            self.saved.settings.default_local_shell.clone(),
        );
        let pane_id = self.spawn_pane(request.clone(), window, cx);
        let workspace_id = self.next_workspace_id();

        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title: request.title.clone(),
            pane_ids: vec![pane_id],
            active_pane_id: pane_id,
            unread_events: 0,
            split_axis: SplitAxis::Horizontal,
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
        });

        self.active_workspace_id = Some(workspace_id);
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
                    split_axis: SplitAxis::Horizontal,
                    view_mode: WorkspaceViewMode::Terminal,
                    sftp: None,
                    search_visible: false,
                    search_query: String::new(),
                    search_results: Vec::new(),
                    active_search_index: None,
                    broadcast_input: false,
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
            split_axis: SplitAxis::Horizontal,
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
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
            workspace.pane_ids.push(pane_id);
            workspace.active_pane_id = pane_id;
            workspace.split_axis = axis;
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
            workspace.pane_ids.retain(|item| *item != pane_id);
            if workspace.active_pane_id == pane_id {
                if let Some(next_id) = workspace.pane_ids.last().copied() {
                    workspace.active_pane_id = next_id;
                }
            }
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

    fn pane_layouts(&self, window: &Window, cx: &Context<Self>) -> Vec<PaneLayout> {
        let Some(workspace) = self.active_workspace() else {
            return Vec::new();
        };

        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let search_height = if workspace.search_visible {
            WORKSPACE_SEARCH_ROW_HEIGHT
        } else {
            0.0
        };
        let autocomplete_height = if self.workspace_autocomplete_candidates().is_empty() {
            0.0
        } else {
            WORKSPACE_AUTOCOMPLETE_HEIGHT
        };
        let available_x = WORKSPACE_PADDING;
        let available_y = theme::CHROME_HEIGHT
            + theme::WORKSPACE_HEADER_HEIGHT
            + WORKSPACE_QUICK_ACTIONS_HEIGHT
            + search_height
            + WORKSPACE_PADDING;
        let available_width = (viewport_width - WORKSPACE_PADDING * 2.0).max(320.0);
        let available_height = (viewport_height
            - theme::CHROME_HEIGHT
            - theme::WORKSPACE_HEADER_HEIGHT
            - WORKSPACE_QUICK_ACTIONS_HEIGHT
            - search_height
            - autocomplete_height
            - theme::STATUS_HEIGHT
            - WORKSPACE_PADDING * 2.0)
            .max(180.0);
        let pane_count = workspace.pane_ids.len().max(1);
        let (char_width, line_height) = self.terminal_metrics(window, cx);
        let mut layouts = Vec::with_capacity(pane_count);

        for (index, pane_id) in workspace.pane_ids.iter().copied().enumerate() {
            let (pane_width, pane_height, pane_x, pane_y) = match workspace.split_axis {
                SplitAxis::Horizontal => {
                    let width = ((available_width - PANE_GAP * (pane_count as f32 - 1.0))
                        / pane_count as f32)
                        .max(120.0);
                    (
                        width,
                        available_height,
                        available_x + index as f32 * (width + PANE_GAP),
                        available_y,
                    )
                }
                SplitAxis::Vertical => {
                    let height = ((available_height - PANE_GAP * (pane_count as f32 - 1.0))
                        / pane_count as f32)
                        .max(120.0);
                    (
                        available_width,
                        height,
                        available_x,
                        available_y + index as f32 * (height + PANE_GAP),
                    )
                }
            };

            let cell_width = (pane_width - TERMINAL_INNER_PADDING_X * 2.0).max(32.0);
            let cell_height =
                (pane_height - PANE_HEADER_HEIGHT - TERMINAL_INNER_PADDING_Y * 2.0).max(24.0);
            let cols = (cell_width / char_width).floor().max(1.0) as u16;
            let rows = (cell_height / line_height).floor().max(1.0) as u16;

            layouts.push(PaneLayout {
                pane_id,
                cell_x: pane_x + TERMINAL_INNER_PADDING_X,
                cell_y: pane_y + PANE_HEADER_HEIGHT + TERMINAL_INNER_PADDING_Y,
                cell_width,
                cell_height,
                cols,
                rows,
                char_width,
                line_height,
            });
        }

        layouts
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
            workspace.pane_ids.retain(|id| *id != pane_id);
            if workspace.active_pane_id == pane_id {
                if let Some(next) = workspace.pane_ids.last().copied() {
                    workspace.active_pane_id = next;
                }
            }
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
            split_axis: SplitAxis::Horizontal,
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
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

    fn duplicate_pane(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.workspace_id_for_pane(pane_id) else {
            return;
        };
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            workspace.active_pane_id = pane_id;
        }
        if self.active_workspace_id != Some(workspace_id) {
            self.active_workspace_id = Some(workspace_id);
        }
        let axis = self
            .workspace(workspace_id)
            .map(|workspace| workspace.split_axis)
            .unwrap_or(SplitAxis::Horizontal);
        self.split_active_workspace(axis, window, cx);
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
                        if shell_command_requires_continuation(&command) {
                            if !pane.current_input.ends_with('\n') {
                                pane.current_input.push('\n');
                            }
                        } else if !pane.current_input.contains('\n') {
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
                "up" => {
                    if self.active_pane().is_some_and(|pane| pane.id == pane_id)
                        && self.move_autocomplete_selection(-1, cx)
                    {
                        return true;
                    }
                }
                "down" => {
                    if self.active_pane().is_some_and(|pane| pane.id == pane_id)
                        && self.move_autocomplete_selection(1, cx)
                    {
                        return true;
                    }
                }
                "enter" => {
                    if self.active_pane().is_some_and(|pane| pane.id == pane_id)
                        && self.accept_selected_autocomplete(cx)
                    {
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
        if let Some(pane) = self.pane_mut(pane_id) {
            if lines < 0 {
                pane.terminal.scroll_scrollback(lines.abs());
            } else if lines > 0 {
                pane.terminal.scroll_scrollback(-lines);
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

    fn workspace_indicators(&self, workspace: &WorkspaceTab) -> WorkspaceIndicators {
        let mut indicators = WorkspaceIndicators {
            split_count: workspace.pane_ids.len(),
            unread_events: workspace.unread_events,
            ..WorkspaceIndicators::default()
        };

        for pane_id in &workspace.pane_ids {
            if let Some(pane) = self.pane(*pane_id) {
                if pane.connected {
                    indicators.live_panes += 1;
                } else if pane.closed {
                    if pane.status == "Error" {
                        indicators.error_panes += 1;
                    } else {
                        indicators.closed_panes += 1;
                    }
                } else {
                    indicators.connecting_panes += 1;
                }
            }
        }

        indicators
    }

    fn render_workspace_indicators(
        &self,
        indicators: WorkspaceIndicators,
        active: bool,
    ) -> AnyElement {
        let mut nodes = Vec::new();

        if indicators.unread_events > 0 {
            nodes.push(
                div()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(6.))
                    .rounded(px(999.))
                    .bg(theme::accent())
                    .text_size(px(11.))
                    .font_semibold()
                    .text_color(theme::library_card())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(indicators.unread_events.min(99).to_string())
                    .into_any_element(),
            );
        }

        if indicators.error_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::danger())
                    .into_any_element(),
            );
        } else if indicators.connecting_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::warning())
                    .into_any_element(),
            );
        } else if indicators.live_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::success())
                    .opacity(if active { 1.0 } else { 0.86 })
                    .into_any_element(),
            );
        } else if indicators.closed_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::with_alpha(theme::text_muted_dark(), 0.45))
                    .into_any_element(),
            );
        }

        if indicators.split_count > 1 {
            nodes.push(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(999.))
                    .bg(theme::with_alpha(theme::text_muted_dark(), 0.12))
                    .text_size(px(11.))
                    .font_medium()
                    .text_color(theme::text_muted_dark())
                    .child(format!("{}p", indicators.split_count))
                    .into_any_element(),
            );
        }

        h_flex()
            .gap_1()
            .items_center()
            .children(nodes)
            .into_any_element()
    }

    fn render_chrome_tab(
        &self,
        id: impl Into<ElementId>,
        icon: Icon,
        label: impl Into<SharedString>,
        active: bool,
        indicators: Option<WorkspaceIndicators>,
        close_button: Option<AnyElement>,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        h_flex()
            .id(id)
            .gap(px(7.))
            .items_center()
            .pl(px(12.))
            .pr(if close_button.is_some() {
                px(6.)
            } else {
                px(14.)
            })
            .h(px(34.))
            .rounded(px(8.))
            .bg(if active {
                theme::chrome_tab_active()
            } else {
                gpui::transparent_black()
            })
            .when(!active, |this| {
                this.hover(|style| style.bg(theme::chrome_tab()))
            })
            .child(icon.size(px(14.)).text_color(if active {
                theme::accent()
            } else {
                theme::text_muted_dark()
            }))
            .child(
                div()
                    .max_w(px(140.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(if active {
                        theme::text_on_dark()
                    } else {
                        theme::with_alpha(theme::text_on_dark(), 0.72)
                    })
                    .child(label),
            )
            .when_some(indicators, |this, indicators| {
                this.child(self.render_workspace_indicators(indicators, active))
            })
            .when_some(close_button, |this, button| this.child(button))
    }

    fn render_top_chrome(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library_active = self.active_workspace_id.is_none();

        h_flex()
            .h(px(theme::CHROME_HEIGHT))
            .w_full()
            .pl(px(theme::CHROME_INSET_LEFT))
            .pr(px(12.))
            .gap(px(3.))
            .items_center()
            .bg(theme::chrome_bg())
            .border_b_1()
            .border_color(theme::border_dark())
            .child(
                self.render_chrome_tab(
                    "chrome-hosts",
                    Icon::new(IconName::Globe),
                    "Hosts",
                    library_active,
                    None,
                    None,
                )
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.activate_library(window, cx);
                })),
            )
            .when(!self.workspaces.is_empty(), |this| {
                this.child(
                    div()
                        .w(px(1.))
                        .h(px(20.))
                        .mx(px(4.))
                        .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                )
            })
            .children(self.workspaces.iter().map(|workspace| {
                let workspace_id = workspace.id;
                let close_id = workspace.id;
                let active = self.active_workspace_id == Some(workspace.id);
                let drag_info = WorkspaceTabDrag {
                    workspace_id,
                    title: workspace.title.clone(),
                };
                let indicators = self.workspace_indicators(workspace);
                self.render_chrome_tab(
                    ("chrome-workspace", workspace.id),
                    Icon::new(IconName::SquareTerminal),
                    workspace.title.clone(),
                    active,
                    Some(indicators),
                    Some(
                        div()
                            .id(("chrome-close-wrap", workspace.id))
                            .size(px(18.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| {
                                style.bg(theme::with_alpha(theme::text_muted_dark(), 0.2))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_workspace(close_id, cx);
                            }))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(12.))
                                    .text_color(theme::text_muted_dark()),
                            )
                            .into_any_element(),
                    ),
                )
                .on_drag(drag_info, |drag: &WorkspaceTabDrag, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| WorkspaceTabDragPreview {
                        title: drag.title.clone(),
                    })
                })
                .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                    if drag.workspace_id == workspace_id {
                        style
                    } else {
                        style
                            .ml(px(2.))
                            .border_l_2()
                            .border_color(theme::accent())
                            .bg(theme::with_alpha(theme::accent(), 0.12))
                    }
                })
                .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, _, cx| {
                    if drag.workspace_id != workspace_id {
                        this.reorder_workspace_tabs(drag.workspace_id, Some(workspace_id), false);
                        this.error_message.clear();
                        cx.notify();
                    }
                }))
                .cursor_grab()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_workspace(workspace_id, window, cx);
                }))
                .into_any_element()
            }))
            .child(
                div()
                    .id("chrome-workspace-drop-tail")
                    .h_full()
                    .flex_1()
                    .min_w(px(24.))
                    .drag_over::<WorkspaceTabDrag>(|style, _, _, _| {
                        style.bg(theme::with_alpha(theme::accent(), 0.08))
                    })
                    .on_drop(cx.listener(|this, drag: &WorkspaceTabDrag, _, cx| {
                        this.reorder_workspace_tabs(drag.workspace_id, None, true);
                        this.error_message.clear();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("chrome-local-btn")
                    .size(px(30.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::with_alpha(theme::text_muted_dark(), 0.15))
                    .hover(|style| {
                        style
                            .bg(theme::chrome_tab())
                            .border_color(theme::with_alpha(theme::text_muted_dark(), 0.3))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_local_terminal(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(14.))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
            .child(
                div()
                    .id("chrome-new-btn")
                    .size(px(30.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::with_alpha(theme::text_muted_dark(), 0.15))
                    .hover(|style| {
                        style
                            .bg(theme::chrome_tab())
                            .border_color(theme::with_alpha(theme::text_muted_dark(), 0.3))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_library(window, cx);
                        this.open_editor_for_new_host(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(14.))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
    }

    fn nav_card(
        &self,
        id: impl Into<ElementId>,
        section: NavSection,
        active: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let _ = cx;
        h_flex()
            .id(id)
            .w_full()
            .items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(10.))
            .h(px(36.))
            .rounded(px(8.))
            .bg(if active {
                theme::library_card()
            } else {
                gpui::transparent_black()
            })
            .when(active, |this| {
                this.border_l_3().border_color(theme::accent()).pl(px(9.))
            })
            .cursor_pointer()
            .hover(|style| style.bg(theme::library_card()))
            .child(section.icon().size(px(16.)).text_color(if active {
                theme::accent()
            } else {
                theme::text_muted()
            }))
            .child(
                div()
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(if active {
                        theme::text_main()
                    } else {
                        theme::text_muted()
                    })
                    .child(section.label()),
            )
    }

    fn render_library_sidebar(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .w(px(theme::HOST_SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .px(px(10.))
            .pt(px(12.))
            .pb(px(12.))
            .bg(theme::library_sidebar())
            .border_r_1()
            .border_color(theme::border())
            .child(
                v_flex().gap(px(2.)).children(
                    [
                        NavSection::Hosts,
                        NavSection::Vaults,
                        NavSection::Keychain,
                        NavSection::Snippets,
                        NavSection::Settings,
                    ]
                    .into_iter()
                    .map(|section| {
                        let active = self.nav_section == section;
                        self.nav_card(("nav-card", nav_section_key(section)), section, active, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_section = section;
                                this.error_message.clear();
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
                ),
            )
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .my(px(8.))
                    .bg(theme::with_alpha(theme::border(), 0.6)),
            )
            .child(
                v_flex().gap(px(2.)).children(
                    [NavSection::KnownHosts, NavSection::Logs]
                        .into_iter()
                        .map(|section| {
                            let active = self.nav_section == section;
                            self.nav_card(
                                ("nav-card", nav_section_key(section)),
                                section,
                                active,
                                cx,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_section = section;
                                this.error_message.clear();
                                cx.notify();
                            }))
                            .into_any_element()
                        }),
                ),
            )
    }

    fn host_card(
        &self,
        card_ix: usize,
        profile: &HostProfile,
        selected: bool,
        batch_selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let profile_id = profile.id.clone();
        let connect_profile_id = profile.id.clone();
        let favorite_profile_id = profile.id.clone();
        let batch_profile_id = profile.id.clone();
        let favorite_selected = profile.favorite;
        let accent = match profile.color_tag {
            Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
            None => theme::host_chip_color(&profile.display_name()),
        };
        let group_label = profile.group.trim().to_string();
        let tags = profile.tags.iter().take(3).cloned().collect::<Vec<_>>();
        let identity_label = profile
            .identity_id
            .as_deref()
            .and_then(|identity_id| self.identity_by_id(identity_id))
            .map(|identity| identity.label.clone());
        let jump_host_label = profile
            .jump_host_id
            .as_deref()
            .and_then(|jump_host_id| self.jump_host_display_name(jump_host_id))
            .map(|label| format!("Via {label}"));
        let startup_label = (profile.startup_directory.is_some()
            || profile.startup_command.is_some())
        .then(|| "Startup".to_string());
        let connect_view_label = profile.start_in_files.then(|| "Files First".to_string());
        let scrollback_label = profile
            .terminal_scrollback_rows
            .map(|rows| format!("{}k Scrollback", rows / 1000))
            .filter(|label| label != "10k Scrollback");
        let forward_count = profile.effective_port_forward_rules().len();
        let forward_label = (forward_count > 0).then(|| {
            if forward_count == 1 {
                "1 Forward".to_string()
            } else {
                format!("{forward_count} Forwards")
            }
        });
        let last_connected_label = self
            .last_connected_at(profile)
            .map(|ts| format!("Last {}", format_relative_time(ts)));
        let protocols = if profile.auth_mode == AuthMode::PrivateKey {
            "key auth"
        } else {
            "password"
        };
        let protocol_icon = if profile.auth_mode == AuthMode::PrivateKey {
            app_icon(ICON_KEY)
        } else {
            Icon::new(IconName::User)
        };

        h_flex()
            .id(("host-card", card_ix))
            .w_full()
            .gap(px(14.))
            .items_center()
            .px(px(16.))
            .py(px(14.))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(if selected || batch_selected {
                theme::accent()
            } else {
                theme::border()
            })
            .shadow(vec![gpui::BoxShadow {
                color: theme::card_shadow_color(),
                offset: point(px(0.), px(1.)),
                blur_radius: px(2.),
                spread_radius: px(0.),
            }])
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(theme::card_hover_subtle())
                    .shadow(vec![gpui::BoxShadow {
                        color: theme::card_shadow_strong_color(),
                        offset: point(px(0.), px(2.)),
                        blur_radius: px(8.),
                        spread_radius: px(0.),
                    }])
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.load_profile_into_inputs(&profile_id, window, cx);
            }))
            .child(
                div().size(px(36.)).rounded(px(10.)).bg(accent).child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::SquareTerminal)
                                .size(px(16.))
                                .text_color(theme::library_card()),
                        ),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(profile.display_name()),
                            )
                            .when(profile.favorite, |this| {
                                this.child(
                                    Icon::new(IconName::Star)
                                        .size(px(12.))
                                        .text_color(theme::warning()),
                                )
                            })
                            .when(batch_selected, |this| {
                                this.child(self.status_badge(
                                    "Selected",
                                    theme::with_alpha(theme::success(), 0.16),
                                    theme::success(),
                                ))
                            })
                            .when(!group_label.is_empty(), |this| {
                                this.child(self.status_badge(
                                    group_label.clone(),
                                    theme::with_alpha(theme::slate(), 0.12),
                                    theme::slate(),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(format!("{}  •  {}", profile.endpoint(), profile.username)),
                    )
                    .child({
                        let mut chips: Vec<String> = Vec::new();
                        chips.push(protocols.to_string());
                        if let Some(label) = identity_label.clone() {
                            chips.push(label);
                        }
                        if let Some(label) = jump_host_label.clone() {
                            chips.push(label);
                        }
                        if startup_label.is_some() {
                            chips.push("startup script".to_string());
                        }
                        if let Some(label) = scrollback_label.clone() {
                            chips.push(label.to_lowercase());
                        }
                        if let Some(label) = forward_label.clone() {
                            chips.push(label.to_lowercase());
                        }
                        if connect_view_label.is_some() {
                            chips.push("files first".to_string());
                        }
                        if profile.source == ProfileSource::SshConfig {
                            chips.push("ssh_config".to_string());
                        }
                        let line = chips.join("  •  ");
                        let last = last_connected_label
                            .clone()
                            .map(|s| s.to_lowercase())
                            .unwrap_or_default();
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(protocol_icon.size(px(10.)).text_color(theme::text_muted()))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(line),
                            )
                            .when(!last.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted())
                                        .child(last),
                                )
                            })
                    })
                    .when(!profile.description.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .line_height(relative(1.4))
                                .text_color(theme::text_muted())
                                .child(profile.description.trim().to_string()),
                        )
                    })
                    .when(!tags.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .children(tags.iter().map(|tag| {
                                    self.status_badge(
                                        format!("#{tag}"),
                                        theme::with_alpha(theme::hover(), 0.72),
                                        theme::text_muted(),
                                    )
                                    .into_any_element()
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new(("favorite-host-card", card_ix))
                            .ghost()
                            .xsmall()
                            .icon(if profile.favorite {
                                IconName::Star
                            } else {
                                IconName::StarOff
                            })
                            .tooltip(if profile.favorite { "Unstar" } else { "Star" })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_profile_favorite(
                                    &favorite_profile_id,
                                    !favorite_selected,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new(("select-host-card", card_ix))
                            .ghost()
                            .xsmall()
                            .icon(if batch_selected {
                                IconName::CircleCheck
                            } else {
                                IconName::Plus
                            })
                            .tooltip(if batch_selected { "Deselect" } else { "Select" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_host_batch_selection(&batch_profile_id, cx);
                            })),
                    )
                    .child(
                        Button::new(("connect-host-card", card_ix))
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label("Connect")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.show_editor_panel = false;
                                this.load_profile_into_inputs(&connect_profile_id, window, cx);
                                this.connect_current(window, cx);
                            })),
                    ),
            )
    }

    fn render_host_grid(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let groups = self.grouped_profiles(cx);

        let mut sections = Vec::new();
        let mut card_ix = 0usize;
        for (group_name, profiles) in &groups {
            let visible_count = profiles.len();
            let total_count = self
                .saved
                .profiles
                .iter()
                .filter(|profile| Self::profile_group_name(profile) == *group_name)
                .count();
            let group_name_for_select = group_name.clone();
            let group_name_for_bulk = group_name.clone();
            let header = h_flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(group_name.clone()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(if visible_count == total_count {
                                    format!(
                                        "{} {}",
                                        visible_count,
                                        if visible_count == 1 { "host" } else { "hosts" }
                                    )
                                } else {
                                    format!("{} visible • {} total", visible_count, total_count)
                                }),
                        )
                        .child(
                            Button::new(("group-select", card_ix))
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label("Select Group")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_filtered_group_hosts(&group_name_for_select, cx);
                                })),
                        )
                        .when(group_name != "Favorites", |this| {
                            this.child(
                                Button::new(("group-bulk-target", card_ix))
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::AccentSoft,
                                        cx,
                                    ))
                                    .label("Use as Bulk")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.prepare_bulk_group_assignment(
                                            &group_name_for_bulk,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        }),
                );

            let cards = div().w_full().flex().flex_wrap().gap_3().children(
                profiles.iter().enumerate().map(|(group_ix, profile)| {
                    self.host_card(
                        card_ix + group_ix,
                        profile,
                        self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                        self.selected_host_ids.contains(profile.id.as_str()),
                        cx,
                    )
                    .min_w(px(HOST_CARD_WIDTH))
                    .max_w(px(HOST_CARD_WIDTH * 1.3))
                    .flex_1()
                    .into_any_element()
                }),
            );

            sections.push(
                v_flex()
                    .gap_3()
                    .child(header)
                    .child(cards)
                    .into_any_element(),
            );
            card_ix += profiles.len();
        }

        v_flex()
            .w_full()
            .gap_5()
            .children(sections)
            .when(groups.is_empty(), |this| {
                let query = self.host_search_query(cx);
                let empty_state = if query.trim().is_empty() {
                    self.render_library_empty_state(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "No saved hosts yet",
                        format!(
                            "Saved hosts will appear here. Imported SSH config entries from {} still load automatically when present.",
                            ssh_config_path_label()
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("hosts-empty-new")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("New Host")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    )
                } else {
                    self.render_library_empty_state(
                        Icon::new(IconName::Search)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "No hosts match this filter",
                        "Try a different search, or save a new host to add it to the library.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("hosts-empty-clear-search")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Clear Search")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        Self::set_input_value(
                                            &this.shell_inputs.host_search,
                                            "",
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("hosts-empty-new-filtered")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("New Host")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    )
                };
                this.child(
                    empty_state,
                )
            })
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
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(
                        "Passwords are stored in the system credential store when you save or reconnect with them. Key paths are stored only for reconnects.",
                    ),
            )
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
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("editor-delete")
                            .small()
                            .ghost()
                            .icon(IconName::Delete)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_selected_profile(window, cx);
                                this.close_editor_dialog(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("editor-save")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::AccentSoft, cx))
                            .icon(IconName::Check)
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_profile(window, cx);
                            })),
                    )
                    .child(
                        Button::new("editor-connect")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::ArrowRight)
                            .label("Connect")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor_dialog(window, cx);
                                this.connect_current(window, cx);
                            })),
                    ),
            )
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

    fn render_hosts_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let quick_connect = self.try_quick_connect_from_search(cx);
        let has_quick_connect = quick_connect.is_some();
        let quick_connect_password = self.current_quick_connect_password(cx);
        let filtered_host_count = self.filtered_profile_ids(cx).len();
        let selected_host_count = self.selected_host_ids.len();

        v_flex()
            .size_full()
            .flex_1()
            .gap_3()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .h(px(LIBRARY_TOOLBAR_HEIGHT))
                    .flex_none()
                    .gap_3()
                    .px_4()
                    .items_center()
                    .child(
                        Input::new(&self.shell_inputs.host_search)
                            .flex_1()
                            .appearance(true)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size(px(14.))
                                    .text_color(theme::text_muted()),
                            ),
                    )
                    .when_some(quick_connect, |this, qc| {
                        let label = format!("Connect {}", qc.display_name());
                        this.child(
                            Input::new(&self.shell_inputs.quick_connect_password)
                                .w(px(220.))
                                .mask_toggle(),
                        )
                        .child(
                            Button::new("library-quick-connect")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Success, cx))
                                .label(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(qc) = this.try_quick_connect_from_search(cx) {
                                        let password = this.current_quick_connect_password(cx);
                                        this.quick_connect(
                                            qc,
                                            if password.trim().is_empty() {
                                                None
                                            } else {
                                                Some(password)
                                            },
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        )
                    })
                    .child(
                        Button::new("library-new-host")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .label("New Host")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor_for_new_host(window, cx);
                            })),
                    )
                    .child(
                        Button::new("library-connect")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label("Connect")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.connect_current(window, cx);
                            })),
                    ),
            )
            .when(!self.saved.profiles.is_empty(), |this| {
                this.child(
                    h_flex()
                        .px_4()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(self.status_badge(
                            format!("{filtered_host_count} visible"),
                            theme::library_card(),
                            theme::text_muted(),
                        ))
                        .when(selected_host_count > 0, |this| {
                            this.child(self.status_badge(
                                format!("{selected_host_count} selected"),
                                theme::library_card(),
                                theme::success(),
                            ))
                        })
                        .child(
                            Button::new("hosts-select-all")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label("Select All")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.select_all_filtered_hosts(cx);
                                })),
                        )
                        .when(selected_host_count > 0, |this| {
                            this.child(
                                Button::new("hosts-clear-selection")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Clear")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_host_batch_selection(cx);
                                    })),
                            )
                            .child(
                                Button::new("hosts-bulk-star")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Success,
                                        cx,
                                    ))
                                    .label("Star")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.bulk_set_selected_hosts_favorite(true, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("hosts-bulk-unstar")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Unstar")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.bulk_set_selected_hosts_favorite(false, window, cx);
                                    })),
                            )
                            .child(Input::new(&self.shell_inputs.bulk_group).w(px(180.)))
                            .child(
                                Button::new("hosts-bulk-apply-group")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::AccentSoft,
                                        cx,
                                    ))
                                    .label("Apply Group")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.bulk_assign_selected_hosts_group(window, cx);
                                    })),
                            )
                        }),
                )
            })
            .when(
                !quick_connect_password.trim().is_empty() && !has_quick_connect,
                |this| {
                    this.child(
                        div()
                            .px_4()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child("Quick-connect password stays local until you connect."),
                    )
                },
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_3()
                    .px_4()
                    .pb_4()
                    .overflow_y_scrollbar()
                    .when_some(
                        self.render_hosts_onboarding(window, cx),
                        |this, onboarding| this.child(onboarding),
                    )
                    .when_some(self.render_saved_group_cards(cx), |this, cards| {
                        this.child(cards)
                    })
                    .when_some(self.render_recent_hosts_row(cx), |this, row| {
                        this.child(row)
                    })
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(theme::text_muted())
                            .child("HOSTS"),
                    )
                    .child(self.render_host_grid(window, cx)),
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

    fn keychain_tab_control(&self, cx: &Context<Self>) -> Div {
        let tab = self.keychain_tab;
        h_flex()
            .p(px(3.))
            .rounded(px(8.))
            .bg(theme::hover())
            .child(
                div()
                    .id("keychain-tab-keys")
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(px(14.))
                    .font_medium()
                    .cursor_pointer()
                    .when(tab == KeychainTab::Keys, |this| {
                        this.bg(theme::library_card())
                            .shadow_sm()
                            .text_color(theme::text_main())
                    })
                    .when(tab != KeychainTab::Keys, |this| {
                        this.text_color(theme::text_muted())
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keychain_tab = KeychainTab::Keys;
                        cx.notify();
                    }))
                    .child(app_icon(ICON_KEY).size(px(12.)).text_color(
                        if tab == KeychainTab::Keys {
                            theme::accent()
                        } else {
                            theme::text_muted()
                        },
                    ))
                    .child("Keys"),
            )
            .child(
                div()
                    .id("keychain-tab-identities")
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(px(14.))
                    .font_medium()
                    .cursor_pointer()
                    .when(tab == KeychainTab::Identities, |this| {
                        this.bg(theme::library_card())
                            .shadow_sm()
                            .text_color(theme::text_main())
                    })
                    .when(tab != KeychainTab::Identities, |this| {
                        this.text_color(theme::text_muted())
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keychain_tab = KeychainTab::Identities;
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::User).size(px(12.)).text_color(
                        if tab == KeychainTab::Identities {
                            theme::accent()
                        } else {
                            theme::text_muted()
                        },
                    ))
                    .child("Identities"),
            )
    }

    fn render_keychain_keys(&self, cx: &Context<Self>) -> Div {
        let identities = self.saved.identities.clone();

        v_flex()
            .flex_1()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme::text_muted())
                            .child("Reusable identities for host authentication."),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(!identities.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme::text_muted())
                                        .child(format!(
                                            "{} {}",
                                            identities.len(),
                                            if identities.len() == 1 {
                                                "key"
                                            } else {
                                                "keys"
                                            }
                                        )),
                                )
                            })
                            .child(
                                Button::new("keychain-browse")
                                    .small()
                                    .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                    .icon(IconName::FolderOpen)
                                    .label("Add Key File")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_key_file(window, cx);
                                        this.nav_section = NavSection::Hosts;
                                        this.show_editor_panel = true;
                                        this.draft_auth_mode = AuthMode::PrivateKey;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .overflow_y_scrollbar()
                    .children(identities.iter().enumerate().map(
                        |(index, identity)| {
                            let card_identity = identity.clone();
                            let button_identity = identity.clone();
                            let vault_label =
                                self.effective_vault_name(identity.vault_id.as_deref());
                            let has_pub = std::path::Path::new(&format!("{}.pub", identity.key_path))
                                .exists();

                            h_flex()
                                .id(("keychain-key", index))
                                .justify_between()
                                .items_center()
                                .gap_4()
                                .p_4()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(if index == 0 {
                                    theme::with_alpha(theme::accent(), 0.28)
                                } else {
                                    theme::border()
                                })
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::card_hover()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.use_identity(&card_identity, window, cx);
                                }))
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            div()
                                                .size(px(36.))
                                                .rounded(px(12.))
                                                .bg(theme::with_alpha(theme::accent(), 0.1))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    app_icon(ICON_KEY)
                                                        .size(px(16.))
                                                        .text_color(theme::accent()),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap(px(2.))
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_size(px(14.))
                                                                .font_semibold()
                                                                .text_color(theme::text_main())
                                                                .child(
                                                                    button_identity.label.clone(),
                                                                ),
                                                        )
                                                        .child(self.status_badge(
                                                            &button_identity.kind,
                                                            theme::library_bg(),
                                                            theme::slate(),
                                                        ))
                                                        .when(
                                                            button_identity.source
                                                                == crate::models::IdentitySource::Imported,
                                                            |this| {
                                                                this.child(self.status_badge(
                                                                    "Imported",
                                                                    theme::library_bg(),
                                                                    theme::accent(),
                                                                ))
                                                            },
                                                        )
                                                        .when(index == 0, |this| {
                                                            this.child(self.status_badge(
                                                                "Default",
                                                                theme::library_bg(),
                                                                theme::accent(),
                                                            ))
                                                        })
                                                        .when(has_pub, |this| {
                                                            this.child(self.status_badge(
                                                                "pub",
                                                                theme::library_bg(),
                                                                theme::success(),
                                                            ))
                                                        })
                                                        .child(self.status_badge(
                                                            vault_label,
                                                            theme::library_bg(),
                                                            theme::accent(),
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .text_color(theme::text_muted())
                                                        .child(button_identity.key_path.clone()),
                                                ),
                                        ),
                                )
                                .child(
                                    Button::new(("keychain-use", index))
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::AccentSoft,
                                            cx,
                                        ))
                                        .label("Use")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.use_identity(&button_identity, window, cx);
                                        })),
                                )
                                .into_any_element()
                        },
                    ))
                    .when(identities.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                app_icon(ICON_KEY)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No identities available",
                                "Add a key file to build a reusable identity library for your hosts.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("keys-empty-add")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
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
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn render_keychain_identities(&self, cx: &Context<Self>) -> Div {
        let profiles_with_password: Vec<_> = self
            .saved
            .profiles
            .iter()
            .filter(|p| p.auth_mode == AuthMode::Password)
            .collect();

        v_flex()
            .flex_1()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme::text_muted())
                            .child("Saved host identities with password authentication."),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{} {}",
                                profiles_with_password.len(),
                                if profiles_with_password.len() == 1 {
                                    "identity"
                                } else {
                                    "identities"
                                }
                            )),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(
                        profiles_with_password
                            .iter()
                            .enumerate()
                            .map(|(index, profile)| {
                                let profile_id = profile.id.clone();
                                let vault_label =
                                    self.effective_vault_name(profile.vault_id.as_deref());
                                h_flex()
                                    .id(("identity-card", index))
                                    .justify_between()
                                    .items_center()
                                    .gap_4()
                                    .p_4()
                                    .rounded(px(theme::CARD_RADIUS))
                                    .bg(theme::library_card())
                                    .border_1()
                                    .border_color(theme::border())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::card_hover()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.load_profile_into_inputs(&profile_id, window, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .items_center()
                                            .child(
                                                div()
                                                    .size(px(36.))
                                                    .rounded(px(12.))
                                                    .bg(theme::with_alpha(theme::accent(), 0.1))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        Icon::new(IconName::User)
                                                            .size(px(16.))
                                                            .text_color(theme::accent()),
                                                    ),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(px(14.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(profile.display_name()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .text_color(theme::text_muted())
                                                            .child(format!(
                                                                "{}@{}",
                                                                profile.username, profile.host
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(self.status_badge(
                                        "password",
                                        theme::library_bg(),
                                        theme::text_muted(),
                                    ))
                                    .child(self.status_badge(
                                        vault_label,
                                        theme::library_bg(),
                                        theme::accent(),
                                    ))
                                    .into_any_element()
                            }),
                    )
                    .when(profiles_with_password.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::User)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No password identities saved",
                                "Save a host with password authentication to keep its secure credential reference here.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("password-identities-open-hosts")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .label("New Host")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_editor_for_new_host(window, cx);
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn render_keychain_view(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex().justify_between().items_center().child(
                    div()
                        .text_size(px(20.))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child("Keys"),
                ),
            )
            .child(self.keychain_tab_control(cx))
            .child(match self.keychain_tab {
                KeychainTab::Keys => self.render_keychain_keys(cx),
                KeychainTab::Identities => self.render_keychain_identities(cx),
            })
    }

    fn render_vaults_view(&self, cx: &Context<Self>) -> Div {
        let vaults = self.saved.vaults.clone();
        let selected_vault = self
            .selected_vault_id
            .as_deref()
            .and_then(|vault_id| self.vault_by_id(vault_id))
            .cloned()
            .or_else(|| self.default_vault().cloned());

        v_flex()
            .flex_1()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Vaults"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{} {}",
                                vaults.len(),
                                if vaults.len() == 1 { "vault" } else { "vaults" }
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Vaults are the top-level containers for hosts, identities, and snippets. Shared vaults are local-only metadata for now; sync comes later."),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field("Name", Input::new(&self.vault_inputs.label)))
                    .child(self.form_field(
                        "Description",
                        Input::new(&self.vault_inputs.description),
                    ))
                    .when_some(selected_vault.as_ref(), |this, vault| {
                        let vault = vault.clone();
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .font_medium()
                                                .text_color(theme::text_main())
                                                .child("Members"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme::text_muted())
                                                .child(format!(
                                                    "{} {}",
                                                    vault.members.len(),
                                                    if vault.members.len() == 1 {
                                                        "member"
                                                    } else {
                                                        "members"
                                                    }
                                                )),
                                        ),
                                )
                                .when(vault.is_personal(), |this| {
                                    this.child(
                                        div()
                                            .p_3()
                                            .rounded(px(12.))
                                            .bg(theme::with_alpha(theme::hover(), 0.72))
                                            .border_1()
                                            .border_color(theme::border())
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child("The personal vault is device-local and keeps a single owner profile."),
                                    )
                                })
                                .when(!vault.is_personal(), |this| {
                                    this.child(self.form_field(
                                        "Member Name",
                                        Input::new(&self.vault_member_inputs.name),
                                    ))
                                    .child(self.form_field(
                                        "Member Email",
                                        Input::new(&self.vault_member_inputs.email),
                                    ))
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .font_medium()
                                                    .text_color(theme::text_main())
                                                    .child("Role"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .children([
                                                        VaultMemberRole::Owner,
                                                        VaultMemberRole::Editor,
                                                        VaultMemberRole::Viewer,
                                                    ]
                                                    .into_iter()
                                                    .enumerate()
                                                    .map(|(index, role)| {
                                                        let selected = self.draft_vault_member_role == role;
                                                        div()
                                                            .id(("vault-member-role", index))
                                                            .px_3()
                                                            .py(px(7.))
                                                            .rounded(px(999.))
                                                            .bg(if selected {
                                                                theme::accent_soft()
                                                            } else {
                                                                theme::with_alpha(theme::hover(), 0.72)
                                                            })
                                                            .border_1()
                                                            .border_color(if selected {
                                                                theme::with_alpha(theme::accent(), 0.42)
                                                            } else {
                                                                theme::border()
                                                            })
                                                            .cursor_pointer()
                                                            .hover(|style| style.bg(theme::hover()))
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                this.draft_vault_member_role = role;
                                                                this.error_message.clear();
                                                                cx.notify();
                                                            }))
                                                            .child(
                                                                div()
                                                                    .text_size(px(13.))
                                                                    .font_medium()
                                                                    .text_color(if selected {
                                                                        theme::text_main()
                                                                    } else {
                                                                        theme::text_muted()
                                                                    })
                                                                    .child(role.label()),
                                                            )
                                                            .into_any_element()
                                                    })),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("vault-member-clear")
                                                            .small()
                                                            .custom(Self::action_button_style(
                                                                theme::ActionTone::Neutral,
                                                                cx,
                                                            ))
                                                            .label("Clear Member")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                this.clear_vault_member_form(window, cx);
                                                            })),
                                                    )
                                                    .child(
                                                        Button::new("vault-member-save")
                                                            .small()
                                                            .custom(Self::action_button_style(
                                                                theme::ActionTone::Accent,
                                                                cx,
                                                            ))
                                                            .label("Save Member")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                this.save_vault_member(window, cx);
                                                            })),
                                                    ),
                                            ),
                                    )
                                })
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .children(vault.members.iter().enumerate().map(|(index, member)| {
                                            let member_id = member.id.clone();
                                            let remove_id = member.id.clone();
                                            let selected = self.selected_vault_member_id.as_deref()
                                                == Some(member.id.as_str());

                                            h_flex()
                                                .id(("vault-member-card", index))
                                                .justify_between()
                                                .items_center()
                                                .gap_3()
                                                .p_3()
                                                .rounded(px(12.))
                                                .bg(if selected {
                                                    theme::with_alpha(theme::accent(), 0.1)
                                                } else {
                                                    theme::with_alpha(theme::hover(), 0.72)
                                                })
                                                .border_1()
                                                .border_color(if selected {
                                                    theme::with_alpha(theme::accent(), 0.42)
                                                } else {
                                                    theme::border()
                                                })
                                                .cursor_pointer()
                                                .hover(|style| style.bg(theme::hover()))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.load_vault_member_into_inputs(&member_id, window, cx);
                                                }))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
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
                                                                        .child(member.display_name()),
                                                                )
                                                                .child(self.status_badge(
                                                                    member.role.label(),
                                                                    theme::library_bg(),
                                                                    if member.role == VaultMemberRole::Owner {
                                                                        theme::accent()
                                                                    } else if member.role == VaultMemberRole::Editor {
                                                                        theme::success()
                                                                    } else {
                                                                        theme::slate()
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
                                                                .text_color(theme::text_muted())
                                                                .child(member.email.clone()),
                                                        ),
                                                )
                                                .when(!vault.is_personal(), |this| {
                                                    this.child(
                                                        Button::new(("vault-member-remove", index))
                                                            .small()
                                                            .ghost()
                                                            .icon(IconName::Delete)
                                                            .label("Remove")
                                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                                this.remove_vault_member(&remove_id, window, cx);
                                                            })),
                                                    )
                                                })
                                                .into_any_element()
                                        })),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("vault-new")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("New")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_vault_form(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("vault-save")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_vault(window, cx);
                                    })),
                            )
                            .when(
                                self.selected_vault_id
                                    .as_deref()
                                    .is_some_and(|vault_id| vault_id != DEFAULT_VAULT_ID),
                                |this| {
                                    this.child(
                                        Button::new("vault-delete")
                                            .small()
                                            .ghost()
                                            .icon(IconName::Delete)
                                            .label("Delete")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.remove_selected_vault(window, cx);
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(vaults.iter().enumerate().map(|(index, vault)| {
                        let vault_id = vault.id.clone();
                        let selected = self.selected_vault_id.as_deref() == Some(vault.id.as_str());
                        let (host_count, identity_count, snippet_count) =
                            self.vault_item_counts(&vault.id);
                        let member_count = vault.members.len();

                        h_flex()
                            .id(("vault-card", index))
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(if selected {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::card_hover_subtle()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_vault_into_inputs(&vault_id, window, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(2.))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(vault.display_name()),
                                            )
                                            .child(self.status_badge(
                                                vault.kind.label(),
                                                theme::library_bg(),
                                                if vault.kind == VaultKind::Personal {
                                                    theme::accent()
                                                } else {
                                                    theme::slate()
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(if vault.description.trim().is_empty() {
                                                "No description yet".to_string()
                                            } else {
                                                vault.description.clone()
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(self.status_badge(
                                                format!("{host_count} hosts"),
                                                theme::library_bg(),
                                                theme::success(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{identity_count} keys"),
                                                theme::library_bg(),
                                                theme::accent(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{snippet_count} snippets"),
                                                theme::library_bg(),
                                                theme::warning(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{member_count} members"),
                                                theme::library_bg(),
                                                theme::slate(),
                                            )),
                                    ),
                            )
                            .into_any_element()
                    })),
            )
    }

    fn render_known_hosts_view(&self, cx: &Context<Self>) -> Div {
        let entries = self.known_hosts.entries().unwrap_or_default();

        v_flex()
            .flex_1()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Known Hosts"),
                    )
                    .when(!entries.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "{} trusted {}",
                                    entries.len(),
                                    if entries.len() == 1 { "host" } else { "hosts" }
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Host keys are pinned on first connect (TOFU). Remove an entry here if a server has legitimately changed its key."),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(entries.iter().enumerate().map(|(index, (endpoint, key))| {
                        let remove_endpoint = endpoint.clone();
                        h_flex()
                            .id(("snippet-card", index))
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                app_icon(ICON_SHIELD_CHECK)
                                                    .size(px(14.))
                                                    .text_color(theme::success()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(endpoint.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(short_host_key(key)),
                                    ),
                            )
                            .child(
                                Button::new(("remove-known-host", index))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        match this.known_hosts.remove(&remove_endpoint) {
                                            Ok(true) => {
                                                this.status_message = format!(
                                                    "Removed known host '{}'.",
                                                    remove_endpoint
                                                );
                                                this.error_message.clear();
                                            }
                                            Ok(false) => {
                                                this.status_message =
                                                    "Host was already removed.".to_string();
                                            }
                                            Err(e) => {
                                                this.error_message = e.to_string();
                                            }
                                        }
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    }))
                    .when(entries.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                app_icon(ICON_SHIELD_CHECK)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No hosts pinned yet",
                                "Trust records appear here after the first successful SSH connection to a host.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("known-hosts-open-hosts")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .label("Open Hosts")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.nav_section = NavSection::Hosts;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn render_logs_view(&self, _cx: &Context<Self>) -> Div {
        let logs: Vec<&SessionLogEntry> = self.saved.session_logs.iter().rev().collect();

        v_flex()
            .flex_1()
            .gap_3()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Session History"),
                    )
                    .when(!logs.is_empty(), |this| {
                        this.child(
                            h_flex().gap_2().items_center().child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::text_muted())
                                    .child(format!("{} sessions", logs.len())),
                            ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(self.panes.iter().filter(|p| p.connected).map(|pane| {
                        h_flex()
                            .justify_between()
                            .items_center()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::with_alpha(theme::success(), 0.3))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div().size(px(10.)).rounded(px(999.)).bg(theme::success()),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(pane.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted())
                                                    .child(pane.endpoint.clone()),
                                            ),
                                    ),
                            )
                            .child(self.status_badge(
                                "Active",
                                theme::library_bg(),
                                theme::success(),
                            ))
                            .into_any_element()
                    }))
                    .children(logs.iter().map(|entry| {
                        let (status_color, status_label) = match entry.status {
                            SessionLogStatus::Connected => (theme::success(), "Connected"),
                            SessionLogStatus::Connecting => (theme::accent(), "Connecting"),
                            SessionLogStatus::Disconnected => (theme::text_muted(), "Closed"),
                            SessionLogStatus::Error => (theme::danger(), "Error"),
                        };

                        h_flex()
                            .justify_between()
                            .items_center()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div()
                                            .size(px(10.))
                                            .rounded(px(999.))
                                            .bg(theme::with_alpha(status_color, 0.5)),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(14.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(entry.title.clone()),
                                                    )
                                                    .child(self.status_badge(
                                                        status_label,
                                                        theme::library_bg(),
                                                        status_color,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted())
                                                    .child(format!(
                                                        "{}  {}@{}",
                                                        entry.endpoint(),
                                                        entry.username,
                                                        entry.host,
                                                    )),
                                            )
                                            .child(
                                                h_flex().gap_2().child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .text_color(theme::text_muted())
                                                        .child(format!(
                                                            "Started {}  Duration {}",
                                                            entry.started_display(),
                                                            entry.duration_display(),
                                                        )),
                                                ),
                                            )
                                            .when_some(
                                                entry.error_message.as_ref(),
                                                |this, msg| {
                                                    this.child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .text_color(theme::danger())
                                                            .child(msg.clone()),
                                                    )
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(entry.duration_display()),
                            )
                            .into_any_element()
                    }))
                    .when(logs.is_empty() && self.panes.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::BookOpen)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No session history yet",
                                "Connection history appears here after you open your first SSH workspace.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("logs-open-hosts")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                _cx,
                                            ))
                                            .label("Open Hosts")
                                            .on_click(_cx.listener(|this, _, _, cx| {
                                                this.nav_section = NavSection::Hosts;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn render_snippets_view(&self, _cx: &Context<Self>) -> Div {
        let snippets = self.saved.snippets.clone();

        v_flex()
            .flex_1()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Snippets"),
                    )
                    .when(!snippets.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "{} {}",
                                    snippets.len(),
                                    if snippets.len() == 1 {
                                        "snippet"
                                    } else {
                                        "snippets"
                                    }
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .max_w(px(820.))
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Save repeatable commands, pin the important ones, and send them to the active terminal in one click. Use {{HOST}}, {{USER}}, {{PORT}}, {{TITLE}}, or {{ADDRESS}} for auto-substitution; use {{?Name}} to ask for a value at run time — a small prompt panel opens in the workspace before the command is sent."),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field("Label", Input::new(&self.snippet_inputs.label)))
                    .child(self.render_vault_picker(
                        self.snippet_vault_id.as_deref(),
                        |vault_id, this, _, cx| {
                            this.snippet_vault_id = Some(vault_id.clone());
                            this.selected_vault_id = Some(vault_id.clone());
                            this.status_message = format!(
                                "Assigning this snippet to {}.",
                                this.effective_vault_name(Some(&vault_id))
                            );
                            this.error_message.clear();
                            cx.notify();
                        },
                        _cx,
                    ))
                    .child(self.form_field("Group", Input::new(&self.snippet_inputs.group)))
                    .child(self.form_field("Command", Input::new(&self.snippet_inputs.command)))
                    .child(
                        h_flex()
                            .p(px(3.))
                            .rounded(px(8.))
                            .bg(theme::hover())
                            .children([true, false].into_iter().enumerate().map(
                                |(index, pinned)| {
                                    let active = self.snippet_pinned == pinned;
                                    Button::new(("snippet-pin-toggle", index))
                                        .small()
                                        .custom(Self::segmented_button_style(active, _cx))
                                        .label(if pinned { "Pinned" } else { "Library" })
                                        .on_click(_cx.listener(move |this, _, _, cx| {
                                            this.toggle_snippet_pinned(pinned, cx);
                                        }))
                                        .into_any_element()
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("snippet-new")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        _cx,
                                    ))
                                    .label("New")
                                    .on_click(_cx.listener(|this, _, window, cx| {
                                        this.clear_snippet_form(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("snippet-save")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        _cx,
                                    ))
                                    .label("Save")
                                    .on_click(_cx.listener(|this, _, window, cx| {
                                        this.save_snippet(window, cx);
                                    })),
                            )
                            .when(self.selected_snippet_id.is_some(), |this| {
                                this.child(
                                    Button::new("snippet-delete")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Delete)
                                        .label("Delete")
                                        .on_click(_cx.listener(|this, _, window, cx| {
                                            this.remove_selected_snippet(window, cx);
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(snippets.iter().enumerate().map(|(index, snippet)| {
                        let snippet_id = snippet.id.clone();
                        let run_command = snippet.command.clone();
                        let group_label = snippet.group.trim().to_string();
                        let vault_label = self.effective_vault_name(snippet.vault_id.as_deref());
                        let toggle_snippet_id = snippet.id.clone();
                        let toggle_pinned = !snippet.pinned;

                        h_flex()
                            .id(("snippet-card", index))
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(if self.selected_snippet_id.as_deref()
                                == Some(snippet.id.as_str())
                            {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::card_hover_subtle()))
                            .on_click(_cx.listener(move |this, _, window, cx| {
                                this.load_snippet_into_inputs(&snippet_id, window, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(2.))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(snippet.display_name()),
                                            )
                                            .when(!group_label.is_empty(), |this| {
                                                this.child(self.status_badge(
                                                    group_label.clone(),
                                                    theme::library_bg(),
                                                    theme::slate(),
                                                ))
                                            })
                                            .when(snippet.pinned, |this| {
                                                this.child(self.status_badge(
                                                    "Pinned",
                                                    theme::library_bg(),
                                                    theme::warning(),
                                                ))
                                            })
                                            .child(self.status_badge(
                                                vault_label,
                                                theme::library_bg(),
                                                theme::accent(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(snippet.command.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("snippet-pin", index))
                                            .small()
                                            .custom(Self::action_button_style(
                                                if snippet.pinned {
                                                    theme::ActionTone::AccentSoft
                                                } else {
                                                    theme::ActionTone::Neutral
                                                },
                                                _cx,
                                            ))
                                            .label(if snippet.pinned { "Unpin" } else { "Pin" })
                                            .on_click(_cx.listener(move |this, _, window, cx| {
                                                this.set_saved_snippet_pinned(
                                                    &toggle_snippet_id,
                                                    toggle_pinned,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("snippet-run", index))
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Success,
                                                _cx,
                                            ))
                                            .label("Run")
                                            .on_click(_cx.listener(move |this, _, window, cx| {
                                                this.run_snippet_command(&run_command, window, cx);
                                            })),
                                    ),
                            )
                            .into_any_element()
                    }))
                    .when(snippets.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::BookOpen)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No snippets yet",
                                "Save repeatable commands here so they can be searched, pinned, and sent into active terminals.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("snippets-empty-new")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                _cx,
                                            ))
                                            .label("New Snippet")
                                            .on_click(_cx.listener(|this, _, window, cx| {
                                                this.clear_snippet_form(window, cx);
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn settings_section_card<E: IntoElement>(
        &self,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        body: E,
    ) -> Div {
        let title: SharedString = title.into();
        let description: SharedString = description.into();
        v_flex()
            .w_full()
            .gap(px(14.))
            .px(px(20.))
            .py(px(18.))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .shadow(vec![gpui::BoxShadow {
                color: theme::card_shadow_color(),
                offset: point(px(0.), px(1.)),
                blur_radius: px(2.),
                spread_radius: px(0.),
            }])
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.5))
                            .text_color(theme::text_muted())
                            .child(description),
                    ),
            )
            .child(body)
    }

    fn settings_subhead(
        &self,
        title: impl Into<SharedString>,
        hint: impl Into<SharedString>,
    ) -> Div {
        let title: SharedString = title.into();
        let hint: SharedString = hint.into();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
    }

    fn settings_divider(&self) -> Div {
        div()
            .h(px(1.))
            .w_full()
            .bg(theme::with_alpha(theme::border(), 0.6))
    }

    fn settings_shortcut_row(&self, keys: &'static str, description: &'static str) -> Div {
        h_flex()
            .justify_between()
            .items_center()
            .gap_3()
            .py(px(4.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_main())
                    .child(description),
            )
            .child(
                div()
                    .px_2()
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(theme::with_alpha(theme::hover(), 0.85))
                    .border_1()
                    .border_color(theme::border())
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(theme::text_muted())
                    .child(keys.replace("Cmd", primary_shortcut_label())),
            )
    }

    fn settings_shortcut_group<const N: usize>(
        &self,
        title: &'static str,
        rows: [(&'static str, &'static str); N],
    ) -> Div {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(13.))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .children(
                rows.into_iter()
                    .map(|(keys, desc)| self.settings_shortcut_row(keys, desc).into_any_element()),
            )
    }

    fn render_settings_view(&self, cx: &Context<Self>) -> Div {
        let theme_preset = self.saved.settings.theme_preset;
        let terminal_font_size = self.saved.settings.terminal_font_size;
        let restore_workspaces_on_launch = self.saved.settings.restore_workspaces_on_launch;
        let session_log_limit = self.saved.settings.session_log_limit;
        let onboarding_dismissed = self.saved.settings.onboarding_dismissed;
        let auto_reconnect_attempts = self.saved.settings.auto_reconnect_attempts;
        let auto_reconnect_delay_secs = self.saved.settings.auto_reconnect_delay_secs;
        let ssh_keepalive_secs = self.saved.settings.ssh_keepalive_secs;
        let copy_on_select = self.saved.settings.copy_on_select;
        let confirm_multiline_paste = self.saved.settings.confirm_multiline_paste;
        let session_log_count = self.saved.session_logs.len();
        let has_default_ssh_dir = self.saved.settings.default_ssh_startup_directory.is_some();

        let appearance_card = self.settings_section_card(
            "Appearance",
            "Switch the global UI palette across the whole desktop app.",
            v_flex()
                .gap_3()
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [ThemePreset::Ocean, ThemePreset::Daylight]
                            .into_iter()
                            .enumerate()
                            .map(|(index, preset)| {
                                let selected = preset == theme_preset;
                                div()
                                    .id(("settings-theme", index))
                                    .px_3()
                                    .py(px(8.))
                                    .rounded(px(999.))
                                    .bg(if selected {
                                        theme::accent_soft()
                                    } else {
                                        theme::with_alpha(theme::hover(), 0.72)
                                    })
                                    .border_1()
                                    .border_color(if selected {
                                        theme::with_alpha(theme::accent(), 0.42)
                                    } else {
                                        theme::border()
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_theme_preset(preset, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(if selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(preset.label()),
                                    )
                                    .into_any_element()
                            }),
                    ),
                )
                .child(
                    h_flex().gap_3().flex_wrap().children(
                        [
                            ("Library", theme::library_card(), theme::text_main()),
                            ("Chrome", theme::chrome_bg(), theme::text_on_dark()),
                            ("Terminal", theme::terminal_bg(), theme::text_on_dark()),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, bg, fg))| {
                            v_flex()
                                .id(("settings-preview", index))
                                .flex_1()
                                .min_w(px(140.))
                                .gap_1()
                                .p_3()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(bg)
                                .border_1()
                                .border_color(theme::with_alpha(fg, 0.18))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_semibold()
                                        .text_color(fg)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::with_alpha(fg, 0.78))
                                        .child(match label {
                                            "Library" => "Forms, host cards, and management views",
                                            "Chrome" => "Tabs, status bar, and workspace header",
                                            _ => "Terminal panels and focused work sessions",
                                        }),
                                )
                                .into_any_element()
                        }),
                    ),
                ),
        );

        let terminal_card = self.settings_section_card(
            "Terminal",
            "Tune what feels right inside every PTY: font size, selection behavior, and clipboard flow.",
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    "Font size",
                    "Apply a larger or tighter monospace size across every terminal pane.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([12u16, 13, 14, 15, 16, 18].into_iter().enumerate().map(
                            |(index, font_size)| {
                                let selected = font_size == terminal_font_size;
                                div()
                                    .id(("settings-font-size", index))
                                    .px_3()
                                    .py(px(8.))
                                    .rounded(px(999.))
                                    .bg(if selected {
                                        theme::accent_soft()
                                    } else {
                                        theme::with_alpha(theme::hover(), 0.72)
                                    })
                                    .border_1()
                                    .border_color(if selected {
                                        theme::with_alpha(theme::accent(), 0.42)
                                    } else {
                                        theme::border()
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.update_terminal_font_size(font_size, window, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(if selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(format!("{font_size} px")),
                                    )
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Copy on select",
                    "When enabled, releasing the mouse over a selection automatically copies it to the clipboard, like classic Unix terminals and Termius.",
                ))
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, enabled)| {
                                let active = enabled == copy_on_select;
                                Button::new(("settings-copy-on-select", index))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if enabled { "Auto Copy" } else { "Manual Only" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_copy_on_select(enabled, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Multi-line paste safety",
                    "Hold the paste in a confirmation banner when the clipboard contains newlines, so you don't accidentally execute a script you didn't mean to.",
                ))
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, enabled)| {
                                let active = enabled == confirm_multiline_paste;
                                Button::new(("settings-confirm-paste", index))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if enabled { "Confirm" } else { "Direct" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_confirm_multiline_paste(enabled, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Font family",
                    "Override the monospace font used in every terminal pane. Leave blank to inherit the app default.",
                ))
                .child(self.form_field(
                    "Font Family",
                    Input::new(&self.settings_inputs.terminal_font_family),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-terminal-font-family-save")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Font Family")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_terminal_font_family(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-terminal-font-family-reset")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Reset")
                                .disabled(self.saved.settings.terminal_font_family.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_terminal_font_family(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Font names are passed to the platform font system; install the family first."),
                        ),
                ),
        );

        let startup_card = self.settings_section_card(
            "Startup",
            "Pick how the app comes back when you launch it and whether the first-run guide reappears.",
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, restore)| {
                                let active = restore == restore_workspaces_on_launch;
                                Button::new(("settings-restore-workspaces", index))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if restore {
                                        "Restore Workspaces"
                                    } else {
                                        "Open Library"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_restore_workspaces_on_launch(restore, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-reset-onboarding")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label(if onboarding_dismissed {
                                    "Show Welcome Panel Again"
                                } else {
                                    "Welcome Panel Visible"
                                })
                                .disabled(!onboarding_dismissed)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reset_onboarding_panel(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(if onboarding_dismissed {
                                    "Bring the first-run Hosts guide back after you have dismissed it."
                                } else {
                                    "The first-run Hosts guide is already available in the library."
                                }),
                        ),
                ),
        );

        let sessions_card = self.settings_section_card(
            "Sessions",
            "Control how connection history is retained and where SSH sessions begin by default.",
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    "History retention",
                    "Keep this many connection history entries locally before older items roll off.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([100u16, 200, 500, 1000].into_iter().enumerate().map(
                            |(index, limit)| {
                                let selected = limit == session_log_limit;
                                Button::new(("settings-session-log-limit", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(format!("{limit} entries"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_session_log_limit(limit, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(format!(
                            "{session_log_count} history entries currently stored."
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Default SSH startup directory",
                    "When a host has no startup directory set, SSH sessions cd into this directory after connecting.",
                ))
                .child(self.form_field(
                    "Startup Directory",
                    Input::new(&self.settings_inputs.default_ssh_startup_directory),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-default-ssh-dir-save")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Default Directory")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_default_ssh_startup_directory(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-default-ssh-dir-clear")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Clear")
                                .disabled(!has_default_ssh_dir)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_default_ssh_startup_directory(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Per-host startup directories always take priority over this default."),
                        ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Auto-reconnect",
                    "When an SSH session drops with an error or unexpected disconnect, retry this many times before giving up.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([0u8, 1, 3, 5, 10].into_iter().enumerate().map(
                            |(index, attempts)| {
                                let selected = attempts == auto_reconnect_attempts;
                                Button::new(("settings-auto-reconnect-attempts", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(if attempts == 0 {
                                        "Off".to_string()
                                    } else {
                                        format!("{attempts} attempts")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_attempts(attempts, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "SSH keep-alive",
                    "Send a SSH-level keep-alive ping at this interval so idle sessions survive NAT timeouts and load balancer drops.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([0u16, 15, 30, 60, 120].into_iter().enumerate().map(
                            |(index, secs)| {
                                let selected = secs == ssh_keepalive_secs;
                                Button::new(("settings-ssh-keepalive", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(if secs == 0 {
                                        "Off".to_string()
                                    } else {
                                        format!("{secs}s")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_ssh_keepalive_secs(secs, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Reconnect delay",
                    "Wait this many seconds between automatic retry attempts.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([2u8, 5, 10, 30].into_iter().enumerate().map(
                            |(index, delay)| {
                                let selected = delay == auto_reconnect_delay_secs;
                                Button::new(("settings-auto-reconnect-delay", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(format!("{delay}s"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_delay(delay, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                ),
        );

        let local_shell_card = self.settings_section_card(
            "Local Shell",
            "Choose which shell binary and working directory new local terminals use.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Shell Program",
                    Input::new(&self.settings_inputs.local_shell_program),
                ))
                .child(self.form_field(
                    "Working Directory",
                    Input::new(&self.settings_inputs.local_shell_cwd),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-local-shell-save")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Shell Defaults")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_local_shell_settings(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Args stay empty for now; this sets the default executable and startup directory."),
                        ),
                ),
        );

        let portable_card = self.settings_section_card(
            "Portable Data Bundle",
            "Export or import hosts, vaults, identities, snippets, and known-host trust records as a local JSON bundle. Passwords and system credential-store secrets are intentionally excluded, so this is safe for portability but not a full account sync.",
            h_flex()
                .gap_2()
                .child(
                    Button::new("settings-export-data")
                        .small()
                        .custom(Self::action_button_style(
                            theme::ActionTone::Neutral,
                            cx,
                        ))
                        .label("Export Data")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.export_portable_data(cx);
                        })),
                )
                .child(
                    Button::new("settings-import-data")
                        .small()
                        .custom(Self::action_button_style(
                            theme::ActionTone::Accent,
                            cx,
                        ))
                        .label("Import Data")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.import_portable_data(window, cx);
                        })),
                ),
        );

        let encrypted_card = self.settings_section_card(
            "Encrypted Backup",
            "Wrap the same portable bundle in passphrase-based encryption for device backups, handoff, or manual sync. The file stays locally managed; no cloud account is involved yet.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Export Passphrase",
                    Input::new(&self.settings_inputs.export_backup_passphrase),
                ))
                .child(self.form_field(
                    "Confirm Passphrase",
                    Input::new(&self.settings_inputs.export_backup_confirm),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-export-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Export Encrypted Backup")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Use a strong passphrase you can recover later. The file cannot be opened without it."),
                        ),
                )
                .child(self.settings_divider())
                .child(self.form_field(
                    "Import Passphrase",
                    Input::new(&self.settings_inputs.import_backup_passphrase),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-import-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Import Encrypted Backup")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Import merges vaults, hosts, snippets, and trust records without exposing the plaintext bundle on disk."),
                        ),
                ),
        );

        let last_pushed = self
            .saved
            .settings
            .sync_last_pushed_at
            .map(|ts| format!("Last push: {}", format_relative_time(ts)))
            .unwrap_or_else(|| "Never pushed.".to_string());
        let last_pulled = self
            .saved
            .settings
            .sync_last_pulled_at
            .map(|ts| format!("Last pull: {}", format_relative_time(ts)))
            .unwrap_or_else(|| "Never pulled.".to_string());
        let sync_card = self.settings_section_card(
            "Shared-folder sync",
            "Cross-device sync without a server. Point at a Dropbox / iCloud Drive / Google Drive / Syncthing folder. Push writes the encrypted bundle; Pull merges the latest one. Your existing cloud drive carries the bundle between machines, so the encrypted file never lives on our servers.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Sync Folder",
                    Input::new(&self.settings_inputs.sync_folder_input),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-pick-folder")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Choose Folder…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-sync-save-folder")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Save Folder Path")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_sync_folder_input(window, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-push")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Push to Folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.push_to_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-sync-pull")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::AccentSoft,
                                    cx,
                                ))
                                .label("Pull from Folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pull_from_sync_folder(window, cx);
                                })),
                        )
                        .when(self.sync_pull_pending_warning, |this| {
                            this.child(
                                Button::new("settings-sync-pull-force")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label("Force Overwrite")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.force_pull_from_sync_folder(window, cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Push reuses the passphrase set in Encrypted Backup; Pull uses the import passphrase."),
                        ),
                )
                .child(
                    h_flex()
                        .gap_4()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(last_pushed),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(last_pulled),
                        ),
                ),
        );

        let shortcuts_card = self.settings_section_card(
            "Keyboard Shortcuts",
            "Every shortcut available right now. Anything that uses a modifier follows your platform convention (Cmd on macOS, Ctrl elsewhere).",
            v_flex()
                .gap_4()
                .child(self.settings_shortcut_group(
                    "Navigation",
                    [
                        ("Cmd+1", "Open Hosts"),
                        ("Cmd+2", "Open Vaults"),
                        ("Cmd+3", "Open Keychain"),
                        ("Cmd+4", "Open Snippets"),
                        ("Cmd+5", "Open Settings"),
                        ("Cmd+6", "Open Known Hosts"),
                        ("Cmd+7", "Open Logs"),
                        ("Cmd+,", "Jump to Settings"),
                        ("Cmd+L", "Focus host search / toggle Logs"),
                        ("Cmd+N", "Create a new host (in library)"),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    "Workspace",
                    [
                        ("Cmd+K", "Open the command palette"),
                        ("Cmd+F", "Search the active terminal"),
                        ("Cmd+T", "Open a new local terminal in a fresh tab"),
                        ("Cmd+W", "Close the active workspace tab"),
                        ("Cmd+D", "Duplicate the active pane"),
                        ("Cmd+Alt+Right", "Cycle to the next workspace tab"),
                        ("Cmd+Alt+Left", "Cycle to the previous workspace tab"),
                        ("Cmd+Shift+B", "Toggle broadcast input across panes"),
                        ("Cmd+Shift+L", "Clear the active pane screen and scrollback"),
                        ("Cmd+Shift+F", "Open the workspace files browser"),
                        ("Cmd+Shift+T", "Toggle Files / Terminal view"),
                        ("Esc", "Close dialogs or return from Files"),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    "Terminal",
                    [
                        ("Cmd+C", "Copy current selection"),
                        ("Cmd+V", "Paste from clipboard"),
                        ("Shift+PageUp", "Scroll back one screen"),
                        ("Shift+PageDown", "Scroll forward one screen"),
                        ("Up / Down", "Move autocomplete selection"),
                        ("Enter", "Accept the highlighted suggestion"),
                    ],
                )),
        );

        v_flex()
            .size_full()
            .flex_1()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .items_center()
                    .px_5()
                    .pt_5()
                    .pb_3()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child("Local desktop preferences"),
                    ),
            )
            .child(
                v_flex()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap_4()
                    .px_5()
                    .pb_5()
                    .overflow_y_scrollbar()
                    .child(appearance_card)
                    .child(terminal_card)
                    .child(startup_card)
                    .child(sessions_card)
                    .child(local_shell_card)
                    .child(shortcuts_card)
                    .child(portable_card)
                    .child(encrypted_card)
                    .child(sync_card),
            )
    }

    fn render_library_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.nav_section {
            NavSection::Hosts => self.render_hosts_view(window, cx).into_any_element(),
            NavSection::Vaults => self.render_vaults_view(cx).into_any_element(),
            NavSection::Keychain => self.render_keychain_view(cx).into_any_element(),
            NavSection::Snippets => self.render_snippets_view(cx).into_any_element(),
            NavSection::Settings => self.render_settings_view(cx).into_any_element(),
            NavSection::KnownHosts => self.render_known_hosts_view(cx).into_any_element(),
            NavSection::Logs => self.render_logs_view(cx).into_any_element(),
        }
    }

    fn render_library_shell(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .flex_1()
            .bg(theme::library_bg())
            .child(self.render_library_sidebar(cx))
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

    fn render_workspace_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let Some(pane) = self.active_pane() else {
            return v_flex();
        };
        let indicators = self.workspace_indicators(workspace);
        let files_mode = workspace.view_mode == WorkspaceViewMode::Files;
        let can_browse_files = !pane.request.is_local_shell();
        let selected_remote_entry = self.selected_workspace_sftp_entry(workspace.id);
        let _focused = pane.terminal_focus.is_focused(window);
        let workspace_id = workspace.id;
        let broadcast_active = workspace.broadcast_input;
        let broadcast_available = workspace.pane_ids.len() > 1;
        let renaming = self.tab_rename_workspace_id == Some(workspace_id);

        h_flex()
            .h(px(theme::WORKSPACE_HEADER_HEIGHT))
            .w_full()
            .px(px(18.))
            .gap_2()
            .items_center()
            .justify_between()
            .bg(theme::chrome_bg())
            .border_b_1()
            .border_color(theme::border_dark())
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(16.))
                            .text_color(theme::accent()),
                    )
                    .when(renaming, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .w(px(220.))
                                        .child(Input::new(&self.tab_rename_input).small()),
                                )
                                .child(
                                    Button::new("workspace-rename-save")
                                        .xsmall()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label("Save")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.commit_workspace_rename(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("workspace-rename-cancel")
                                        .xsmall()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_workspace_rename(window, cx);
                                        })),
                                ),
                        )
                    })
                    .when(!renaming, |this| {
                        this.child(
                            div()
                                .id(("workspace-title", workspace_id))
                                .text_size(px(15.))
                                .font_semibold()
                                .text_color(theme::text_on_dark())
                                .cursor_pointer()
                                .hover(|style| {
                                    style.text_color(theme::with_alpha(theme::accent(), 0.95))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_workspace_rename(window, cx);
                                }))
                                .child(workspace.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted_dark())
                                .child(pane.endpoint.clone()),
                        )
                    })
                    .when_some(
                        workspace_runtime_summary(indicators),
                        |this, (label, tone)| {
                            this.child(self.status_badge(
                                label,
                                theme::terminal_panel(),
                                match tone {
                                    WorkspaceRuntimeTone::Live => theme::success(),
                                    WorkspaceRuntimeTone::Connecting => theme::warning(),
                                    WorkspaceRuntimeTone::Error => theme::danger(),
                                    WorkspaceRuntimeTone::Closed => theme::text_muted_dark(),
                                },
                            ))
                        },
                    )
                    .when(indicators.split_count > 1, |this| {
                        this.child(self.status_badge(
                            format!("{} Panes", indicators.split_count),
                            theme::terminal_panel(),
                            theme::slate(),
                        ))
                    })
                    .when(files_mode, |this| {
                        this.child(self.status_badge(
                            "Files",
                            theme::terminal_panel(),
                            theme::accent(),
                        ))
                    })
                    .when(pane.request.is_local_shell(), |this| {
                        this.child(self.status_badge(
                            "Local",
                            theme::terminal_panel(),
                            theme::success(),
                        ))
                    })
                    .when_some(pane.request.jump_host.as_ref(), |this, jump_host| {
                        this.child(self.status_badge(
                            format!("Via {}", jump_host.title),
                            theme::terminal_panel(),
                            theme::accent(),
                        ))
                    })
                    .when(!pane.request.port_forward_rules.is_empty(), |this| {
                        let forward_label = if pane.request.port_forward_rules.len() == 1 {
                            pane.request.port_forward_rules[0].display_name()
                        } else {
                            format!("{} Forwards", pane.request.port_forward_rules.len())
                        };
                        this.child(self.status_badge(
                            forward_label,
                            theme::terminal_panel(),
                            theme::warning(),
                        ))
                    }),
            )
            .child(
                h_flex()
                    .gap(px(3.))
                    .items_center()
                    .child(self.status_badge(
                        pane.status.clone(),
                        theme::terminal_panel(),
                        if pane.connected {
                            theme::success()
                        } else if pane.closed {
                            theme::warning()
                        } else {
                            theme::accent()
                        },
                    ))
                    .child(
                        Button::new("workspace-scroll-top")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scroll_active_pane_top(cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-scroll-bottom")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowDown)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scroll_active_pane_bottom(cx);
                            })),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                    )
                    .child(
                        Button::new("workspace-search")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_workspace_search(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-commands")
                            .ghost()
                            .small()
                            .icon(IconName::SquareTerminal)
                            .label("Commands")
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_command_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files")
                            .ghost()
                            .small()
                            .icon(if files_mode {
                                IconName::SquareTerminal
                            } else {
                                IconName::FolderOpen
                            })
                            .label(if files_mode { "Terminal" } else { "Files" })
                            .disabled(!files_mode && !can_browse_files)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.active_workspace().is_some_and(|workspace| {
                                    workspace.view_mode == WorkspaceViewMode::Files
                                }) {
                                    this.show_active_workspace_terminal(cx);
                                } else {
                                    this.open_active_workspace_files(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("workspace-split-right")
                            .ghost()
                            .small()
                            .icon(IconName::PanelRight)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Horizontal, window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-split-down")
                            .ghost()
                            .small()
                            .icon(IconName::PanelBottom)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Vertical, window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-broadcast")
                            .small()
                            .custom(Self::segmented_button_style(broadcast_active, cx))
                            .label(if broadcast_active {
                                "Broadcast On"
                            } else {
                                "Broadcast"
                            })
                            .disabled(files_mode || !broadcast_available)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_workspace_broadcast(workspace_id, cx);
                            })),
                    )
                    .when(files_mode, |this| {
                        this.child(
                            Button::new("workspace-files-up")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowLeft)
                                .label("Up")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.navigate_workspace_files_up(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-refresh")
                                .ghost()
                                .small()
                                .icon(IconName::Redo)
                                .label("Refresh")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(workspace_id) = this.active_workspace_id {
                                        this.refresh_workspace_files(workspace_id);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new("workspace-files-upload")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowUp)
                                .label("Upload")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.upload_workspace_file(window, cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-open")
                                .ghost()
                                .small()
                                .icon(IconName::FolderOpen)
                                .label("Open")
                                .disabled(
                                    !selected_remote_entry
                                        .as_ref()
                                        .is_some_and(|entry| entry.is_dir),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_selected_workspace_file_entry(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-download")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowDown)
                                .label("Download")
                                .disabled(
                                    !selected_remote_entry
                                        .as_ref()
                                        .is_some_and(|entry| !entry.is_dir),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.download_workspace_file(window, cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-delete")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .icon(IconName::Delete)
                                .label("Delete")
                                .disabled(selected_remote_entry.is_none())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_workspace_file(cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                    )
                    .when(pane.closed, |this| {
                        this.child(
                            Button::new("workspace-reconnect")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Success, cx))
                                .icon(IconName::Redo)
                                .label("Reconnect")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(pane_id) = this.active_pane().map(|p| p.id) {
                                        this.reconnect_pane(pane_id, window, cx);
                                    }
                                })),
                        )
                    })
                    .when(indicators.closed_panes > 1, |this| {
                        this.child(
                            Button::new("workspace-reconnect-all")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Success, cx))
                                .icon(IconName::Redo)
                                .label(format!("Reconnect All ({})", indicators.closed_panes))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reconnect_all(window, cx);
                                })),
                        )
                    })
                    .when(pane.connected, |this| {
                        this.child(
                            Button::new("workspace-disconnect")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .label("Disconnect")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(workspace_id) = this.active_workspace_id {
                                        this.disconnect_workspace(workspace_id, cx);
                                    }
                                })),
                        )
                    }),
            )
    }

    fn render_quick_actions_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let snippets = self.pinned_snippet_quick_actions();
        let snippet_count = snippets.len();
        let pane_count = self
            .active_workspace()
            .map(|w| w.pane_ids.len())
            .unwrap_or(0);

        h_flex()
            .h(px(WORKSPACE_QUICK_ACTIONS_HEIGHT))
            .w_full()
            .px_4()
            .gap_2()
            .items_center()
            .bg(theme::terminal_panel())
            .border_b_1()
            .border_color(theme::with_alpha(theme::border_dark(), 0.4))
            .child(
                div()
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(theme::text_muted_dark())
                    .child("Actions"),
            )
            .child(
                Button::new("qa-new-tab")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Plus)
                    .label("New Tab")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_library(window, cx);
                        this.open_editor_for_new_host(window, cx);
                    })),
            )
            .child(
                Button::new("qa-split-h")
                    .xsmall()
                    .ghost()
                    .icon(IconName::PanelRight)
                    .label("Split H")
                    .disabled(pane_count >= 4)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.split_active_workspace(SplitAxis::Horizontal, window, cx);
                    })),
            )
            .child(
                Button::new("qa-split-v")
                    .xsmall()
                    .ghost()
                    .icon(IconName::PanelBottom)
                    .label("Split V")
                    .disabled(pane_count >= 4)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.split_active_workspace(SplitAxis::Vertical, window, cx);
                    })),
            )
            .child(
                Button::new("qa-toggle-files")
                    .xsmall()
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .label("Files")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.active_workspace().is_some_and(|workspace| {
                            workspace.view_mode == WorkspaceViewMode::Files
                        }) {
                            this.show_active_workspace_terminal(cx);
                        } else {
                            this.open_active_workspace_files(cx);
                        }
                    })),
            )
            .child(
                Button::new("qa-clear")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Delete)
                    .label("Clear")
                    .on_click(cx.listener(|this, _, _, _cx| {
                        if let Some(pane) = this.active_pane() {
                            let _ = pane
                                .runtime
                                .command_tx
                                .send(SessionCommand::Input(b"clear\n".to_vec()));
                        }
                    })),
            )
            .when(snippet_count > 0, |this| {
                this.child(
                    div()
                        .w(px(1.))
                        .h(px(14.))
                        .mx_1()
                        .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_medium()
                        .text_color(theme::text_muted_dark())
                        .child("Pinned"),
                )
                .child(
                    h_flex().flex_1().gap_1().children(
                        snippets
                            .into_iter()
                            .take(6)
                            .enumerate()
                            .map(|(index, snippet)| {
                                let command = snippet.command.clone();
                                Button::new(("qa-snippet", index))
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::AccentSoft,
                                        cx,
                                    ))
                                    .label(snippet.label.clone())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_snippet_command(&command, window, cx);
                                    }))
                            }),
                    ),
                )
                .when(snippet_count > 6, |this| {
                    let overflow = snippet_count - 6;
                    this.child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted_dark())
                            .child(format!("+{overflow}")),
                    )
                })
            })
    }

    fn render_workspace_search(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let workspace = self.active_workspace()?;
        if workspace.view_mode != WorkspaceViewMode::Terminal {
            return None;
        }
        if !workspace.search_visible {
            return None;
        }
        let matches = workspace.search_results.len();
        let current_match = workspace
            .active_search_index
            .map(|index| index + 1)
            .unwrap_or(0);

        Some(
            h_flex()
                .h(px(WORKSPACE_SEARCH_ROW_HEIGHT))
                .w_full()
                .px_4()
                .gap_3()
                .items_center()
                .bg(theme::terminal_bg())
                .border_b_1()
                .border_color(theme::border_dark())
                .child(Input::new(&self.shell_inputs.terminal_search).flex_1())
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted_dark())
                        .child(format!("{current_match}/{matches}")),
                )
                .child(
                    Button::new("workspace-search-prev")
                        .ghost()
                        .small()
                        .label("Prev")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(-1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-next")
                        .ghost()
                        .small()
                        .label("Next")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_workspace_search(window, cx);
                        })),
                ),
        )
    }

    fn render_workspace_autocomplete(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.show_command_palette {
            return None;
        }
        let workspace = self.active_workspace()?;
        if workspace.view_mode != WorkspaceViewMode::Terminal || workspace.search_visible {
            return None;
        }
        let pane = self.active_pane()?;

        let current_input = pane.current_input.trim().to_string();
        if current_input.is_empty() {
            let snippets = self.pinned_snippet_quick_actions();
            if snippets.is_empty() {
                return None;
            }

            return Some(
                h_flex()
                    .h(px(WORKSPACE_AUTOCOMPLETE_HEIGHT))
                    .w_full()
                    .px_4()
                    .gap_3()
                    .items_center()
                    .bg(theme::terminal_bg())
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted_dark())
                            .child("Pinned commands"),
                    )
                    .child(h_flex().flex_1().gap_2().overflow_x_scrollbar().children(
                        snippets.into_iter().enumerate().map(|(index, snippet)| {
                            let command = snippet.command.clone();
                            Button::new(("workspace-pinned-snippet", index))
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::AccentSoft,
                                    cx,
                                ))
                                .label(snippet.display_name())
                                .icon(IconName::BookOpen)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_snippet_command(&command, window, cx);
                                }))
                                .into_any_element()
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::with_alpha(theme::text_muted_dark(), 0.75))
                            .child("Pin snippets to keep your common commands one click away."),
                    ),
            );
        }

        let candidates = self.workspace_autocomplete_candidates();
        if candidates.is_empty() {
            return None;
        }
        let selected_index = self.selected_autocomplete_index(candidates.len());
        let selected_candidate = candidates.get(selected_index);

        Some(
            h_flex()
                .h(px(WORKSPACE_AUTOCOMPLETE_HEIGHT))
                .w_full()
                .px_4()
                .gap_3()
                .items_center()
                .bg(theme::terminal_bg())
                .border_b_1()
                .border_color(theme::border_dark())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted_dark())
                        .child(
                            match selected_candidate.and_then(|candidate| {
                                candidate.scope_label.as_ref().map(|scope| {
                                    format!(
                                        "Autocomplete for '{}' • {} • {}",
                                        current_input,
                                        candidate.source.label(),
                                        scope
                                    )
                                })
                            }) {
                                Some(label) => label,
                                None => format!(
                                    "Autocomplete for '{}' • {}",
                                    current_input,
                                    selected_candidate
                                        .map(|candidate| candidate.source.label())
                                        .unwrap_or("suggestion")
                                ),
                            },
                        ),
                )
                .child(h_flex().flex_1().gap_2().overflow_x_scrollbar().children(
                    candidates.iter().enumerate().map(|(index, candidate)| {
                        let command = candidate.command.clone();
                        let source = candidate.source;
                        let is_selected = index == selected_index;
                        Button::new(("workspace-autocomplete", index))
                            .small()
                            .custom(Self::action_button_style(
                                if is_selected {
                                    theme::ActionTone::AccentSoft
                                } else {
                                    theme::ActionTone::Neutral
                                },
                                cx,
                            ))
                            .label(command.clone())
                            .icon(match source {
                                AutocompleteSource::Path => IconName::Folder,
                                AutocompleteSource::Context => IconName::SquareTerminal,
                                AutocompleteSource::Argument => IconName::ChevronRight,
                                AutocompleteSource::History => IconName::Redo2,
                                AutocompleteSource::Snippet => IconName::BookOpen,
                                AutocompleteSource::Builtin => IconName::SquareTerminal,
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_autocomplete_candidate(&command, source, cx);
                            }))
                            .into_any_element()
                    }),
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::with_alpha(theme::text_muted_dark(), 0.75))
                        .child(format!(
                            "{}+↑/↓ select  {}+Enter apply",
                            primary_shortcut_label(),
                            primary_shortcut_label()
                        )),
                ),
        )
    }

    fn render_workspace_files_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let workspace_id = workspace.id;
        let Some(browser) = workspace.sftp.as_ref() else {
            let active_pane_is_local = self
                .active_pane()
                .is_some_and(|pane| pane.request.kind == ConnectionKind::LocalShell);
            let empty_state = if active_pane_is_local {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(24.))
                        .text_color(theme::accent()),
                    "Files view is unavailable for local shells",
                    "SFTP file browsing only applies to SSH hosts. Switch back to the terminal to keep working locally.",
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .child(
                            Button::new("workspace-files-local-back")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Back to Terminal")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_active_workspace_terminal(cx);
                                })),
                        ),
                )
            } else {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(24.))
                        .text_color(theme::accent()),
                    "Open Files for this host",
                    "Browse the active SSH host over SFTP, upload and download files, or switch back to the terminal.",
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .child(
                            Button::new("workspace-files-open")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Open Files")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_active_workspace_files(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-back")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Back to Terminal")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_active_workspace_terminal(cx);
                                })),
                        ),
                )
            };

            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(theme::terminal_bg())
                .p(px(WORKSPACE_PADDING))
                .child(empty_state);
        };
        let selected_entry = self.selected_workspace_sftp_entry(workspace.id);

        v_flex()
            .flex_1()
            .p(px(WORKSPACE_PADDING))
            .gap_3()
            .bg(theme::terminal_bg())
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::terminal_panel())
                    .border_1()
                    .border_color(theme::border_dark())
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(theme::text_muted_dark())
                                            .child("Remote Path"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_semibold()
                                            .text_color(theme::text_on_dark())
                                            .child(browser.current_path.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(self.status_badge(
                                        browser.request.address(),
                                        theme::terminal_bg(),
                                        theme::accent(),
                                    ))
                                    .when(browser.loading, |this| {
                                        this.child(self.status_badge(
                                            "Syncing",
                                            theme::terminal_bg(),
                                            theme::warning(),
                                        ))
                                    })
                                    .when_some(selected_entry.as_ref(), |this, entry| {
                                        this.child(self.status_badge(
                                            if entry.is_dir { "Folder" } else { "File" },
                                            theme::terminal_bg(),
                                            theme::success(),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(selected_entry.as_ref(), |this, entry| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted_dark())
                                .child(if entry.is_dir {
                                    format!("Selected folder: {}", entry.path)
                                } else {
                                    format!(
                                        "Selected file: {}  •  {}",
                                        entry.path,
                                        format_file_size(entry.size.unwrap_or(0))
                                    )
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .gap_2()
                    .when(browser.entries.is_empty() && browser.loading, |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::terminal_panel())
                                .border_1()
                                .border_color(theme::border_dark())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::LoaderCircle)
                                        .size(px(24.))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme::text_muted_dark())
                                        .child("Loading remote directory..."),
                                ),
                        )
                    })
                    .when(browser.entries.is_empty() && !browser.loading, |this| {
                        this.child(
                            self.render_workspace_empty_state(
                                Icon::new(IconName::Folder)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "This directory is empty",
                                "Try a different path, upload a file, or switch back to the terminal for shell work.",
                            )
                            .w_full(),
                        )
                    })
                    .children(browser.entries.iter().enumerate().map(|(index, entry)| {
                        let click_path = entry.path.clone();
                        let open_path = entry.path.clone();
                        let is_selected =
                            browser.selected_path.as_deref() == Some(entry.path.as_str());
                        let kind = if entry.is_dir {
                            "Folder".to_string()
                        } else if entry.is_symlink {
                            "Symlink".to_string()
                        } else {
                            format_file_size(entry.size.unwrap_or(0))
                        };

                        h_flex()
                            .id(("workspace-file-entry", index))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded(px(14.))
                            .bg(if is_selected {
                                theme::with_alpha(theme::accent(), 0.18)
                            } else {
                                theme::terminal_panel()
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::with_alpha(theme::accent(), 0.45)
                            } else {
                                theme::border_dark()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::accent(), 0.12)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_workspace_file_entry(
                                    workspace_id,
                                    click_path.clone(),
                                    cx,
                                );
                            }))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Icon::new(if entry.is_dir {
                                            IconName::FolderClosed
                                        } else {
                                            IconName::File
                                        })
                                        .size(px(16.))
                                        .text_color(
                                            if entry.is_dir {
                                                theme::warning()
                                            } else {
                                                theme::text_muted_dark()
                                            },
                                        ),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(1.))
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_medium()
                                                    .text_color(theme::text_on_dark())
                                                    .child(entry.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted_dark())
                                                    .child(entry.path.clone()),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted_dark())
                                            .child(kind),
                                    )
                                    .when(entry.is_dir, |this| {
                                        this.child(
                                            Button::new(("workspace-file-open", index))
                                                .ghost()
                                                .small()
                                                .icon(IconName::ChevronRight)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_workspace_file_entry(
                                                        workspace_id,
                                                        open_path.clone(),
                                                        cx,
                                                    );
                                                    this.open_selected_workspace_file_entry(cx);
                                                })),
                                        )
                                    }),
                            )
                            .into_any_element()
                    })),
            )
    }

    fn terminal_font_family(&self, cx: &Context<Self>) -> SharedString {
        self.saved
            .settings
            .terminal_font_family
            .as_deref()
            .filter(|family| !family.trim().is_empty())
            .map(|family| SharedString::from(family.to_string()))
            .unwrap_or_else(|| cx.theme().mono_font_family.clone())
    }

    fn render_terminal_cell_group(
        &self,
        text: String,
        style: TerminalStyle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut node = div()
            .whitespace_nowrap()
            .font_family(self.terminal_font_family(cx))
            .text_size(px(self.terminal_font_size()))
            .line_height(relative(TERMINAL_LINE_HEIGHT))
            .text_color(style.fg)
            .text_bg(style.bg)
            .child(display_terminal_text(&text));

        if style.bold {
            node = node.font_weight(FontWeight::BOLD);
        }
        if style.italic {
            node = node.italic();
        }
        if style.underline {
            node = node.underline().text_decoration_color(style.fg);
        }

        node.into_any_element()
    }

    fn render_terminal_row(
        &self,
        row_ix: usize,
        row: &TerminalRow,
        selection: Option<SelectionRange>,
        visible_matches: &[(usize, SearchMatch, bool)],
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut groups = Vec::new();
        let mut pending_text = String::new();
        let mut pending_style: Option<TerminalStyle> = None;

        for (col_ix, cell) in row.cells.iter().enumerate() {
            let selected = selection_contains(selection, row_ix, col_ix);
            let (matched, active_match) = visible_matches.iter().fold(
                (false, false),
                |acc, (visible_row, search_match, current)| {
                    if *visible_row != row_ix {
                        return acc;
                    }
                    if (search_match.start_col..search_match.end_col).contains(&col_ix) {
                        (true, acc.1 || *current)
                    } else {
                        acc
                    }
                },
            );
            let style = style_for_render(cell, selected, matched, active_match);

            match pending_style {
                Some(current) if current == style => pending_text.push_str(&cell.text),
                Some(current) => {
                    groups.push(self.render_terminal_cell_group(
                        std::mem::take(&mut pending_text),
                        current,
                        cx,
                    ));
                    pending_text.push_str(&cell.text);
                    pending_style = Some(style);
                }
                None => {
                    pending_text.push_str(&cell.text);
                    pending_style = Some(style);
                }
            }
        }

        if let Some(style) = pending_style {
            groups.push(self.render_terminal_cell_group(pending_text, style, cx));
        } else {
            groups.push(self.render_terminal_cell_group(
                " ".to_string(),
                default_terminal_style(),
                cx,
            ));
        }

        h_flex()
            .w_full()
            .gap_0()
            .whitespace_nowrap()
            .children(groups)
            .into_any_element()
    }

    fn render_terminal_pane(
        &self,
        pane: &SessionPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pane_id = pane.id;
        let snapshot = pane.terminal.snapshot();
        let selection = pane.selection;
        let visible_matches = self
            .active_workspace()
            .map(|workspace| self.workspace_visible_matches(workspace, pane))
            .unwrap_or_default();

        let is_active_pane = self.active_workspace().map(|w| w.active_pane_id) == Some(pane.id);
        let workspace_broadcasting = self
            .active_workspace()
            .map(|workspace| workspace.broadcast_input && workspace.pane_ids.len() > 1)
            .unwrap_or(false);
        let host_color_tag = self
            .saved
            .profiles
            .iter()
            .find(|profile| {
                profile.host == pane.request.host
                    && profile.port == pane.request.port
                    && profile.username == pane.request.username
            })
            .and_then(|profile| profile.color_tag);
        let status_color = if pane.connected {
            match host_color_tag {
                Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
                None => theme::success(),
            }
        } else if pane.closed && pane.status == "Error" {
            theme::danger()
        } else if pane.closed {
            theme::text_muted_dark()
        } else {
            theme::warning()
        };

        v_flex()
            .id(("terminal-pane", pane.id))
            .flex_1()
            .rounded(px(10.))
            .border_1()
            .border_color(if is_active_pane {
                theme::focus_ring()
            } else {
                theme::with_alpha(theme::border_dark(), 0.5)
            })
            .when(is_active_pane, |this| {
                this.shadow(vec![gpui::BoxShadow {
                    color: theme::pane_focus_glow(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(1.),
                }])
            })
            .bg(theme::terminal_panel())
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(PANE_HEADER_HEIGHT))
                    .px(px(12.))
                    .items_center()
                    .justify_between()
                    .bg(theme::chrome_bg())
                    .border_b_1()
                    .border_color(theme::with_alpha(theme::border_dark(), 0.6))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_center()
                            .child(div().size(px(9.)).rounded(px(999.)).bg(status_color))
                            .when(self.pane_rename_id == Some(pane.id), |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(180.))
                                                .child(Input::new(&self.pane_rename_input).small()),
                                        )
                                        .child(
                                            Button::new(("pane-rename-save", pane.id))
                                                .xsmall()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Accent,
                                                    cx,
                                                ))
                                                .label("Save")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.commit_pane_rename(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new(("pane-rename-cancel", pane.id))
                                                .xsmall()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.cancel_pane_rename(window, cx);
                                                })),
                                        ),
                                )
                            })
                            .when(self.pane_rename_id != Some(pane.id), |this| {
                                this.child(
                                    div()
                                        .id(("pane-title", pane.id))
                                        .text_size(px(14.))
                                        .font_medium()
                                        .text_color(theme::text_on_dark())
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.text_color(theme::with_alpha(
                                                theme::accent(),
                                                0.95,
                                            ))
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.start_pane_rename(pane_id, window, cx);
                                        }))
                                        .child(pane.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted_dark())
                                        .child(pane.endpoint.clone()),
                                )
                                .when(
                                    workspace_broadcasting,
                                    |this| {
                                        this.child(self.status_badge(
                                            "Broadcasting",
                                            theme::with_alpha(theme::warning(), 0.18),
                                            theme::warning(),
                                        ))
                                    },
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(pane.connected, |this| {
                                this.child(
                                    Button::new(("clear-pane", pane.id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Replace)
                                        .tooltip("Clear screen and scrollback")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.clear_pane_screen(pane_id, cx);
                                        })),
                                )
                                .when(
                                    self.active_workspace()
                                        .map(|workspace| workspace.pane_ids.len() < MAX_SPLIT_PANES)
                                        .unwrap_or(false),
                                    |this| {
                                        this.child(
                                            Button::new(("duplicate-pane", pane.id))
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::Copy)
                                                .tooltip("Duplicate pane")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.duplicate_pane(pane_id, window, cx);
                                                    },
                                                )),
                                        )
                                    },
                                )
                                .when(
                                    self.active_workspace()
                                        .map(|workspace| workspace.pane_ids.len() > 1)
                                        .unwrap_or(false),
                                    |this| {
                                        this.child(
                                            Button::new(("detach-pane", pane.id))
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::ExternalLink)
                                                .tooltip("Detach into new tab")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.move_pane_to_new_workspace(
                                                            pane_id, window, cx,
                                                        );
                                                    },
                                                )),
                                        )
                                    },
                                )
                            })
                            .when(pane.closed, |this| {
                                this.child(
                                    Button::new(("reconnect-pane", pane.id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Redo)
                                        .label("Reconnect")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.reconnect_pane(pane_id, window, cx);
                                        })),
                                )
                            })
                            .when(
                                self.active_workspace()
                                    .map(|workspace| workspace.pane_ids.len() > 1)
                                    .unwrap_or(false),
                                |this| {
                                    this.child(
                                        Button::new(("close-pane", pane.id))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Close)
                                            .tooltip("Close pane")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.close_pane(pane_id, cx);
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .id(("terminal-surface", pane.id))
                    .size_full()
                    .track_focus(&pane.terminal_focus)
                    .focusable()
                    .bg(theme::terminal_bg())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_pane(pane_id, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.handle_pane_mouse_down(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            this.handle_pane_mouse_move(pane_id, event, window, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, window, cx| {
                            this.handle_pane_scroll(pane_id, event, window, cx);
                        },
                    ))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if this.handle_terminal_key(pane_id, event, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        v_flex()
                            .size_full()
                            .overflow_hidden()
                            .px(px(TERMINAL_INNER_PADDING_X))
                            .pt(px(TERMINAL_INNER_PADDING_Y))
                            .pb(px(TERMINAL_INNER_PADDING_Y))
                            .children(snapshot.rows.iter().enumerate().map(|(row_ix, row)| {
                                self.render_terminal_row(
                                    row_ix,
                                    row,
                                    selection,
                                    &visible_matches,
                                    cx,
                                )
                            })),
                    ),
            )
    }

    fn render_workspace_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .items_center()
                .justify_center()
                .p(px(WORKSPACE_PADDING))
                .child(
                    self.render_workspace_empty_state(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "Open a host to start a workspace",
                        "Select a saved host from the library, use quick connect, or open a local terminal to start working.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("workspace-empty-local")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Local Terminal")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_local_terminal(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("workspace-empty-hosts")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("New Host")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    ),
                );
        };

        let panes = workspace
            .pane_ids
            .iter()
            .filter_map(|pane_id| self.pane(*pane_id))
            .map(|pane| {
                self.render_terminal_pane(pane, window, cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let gap = px(PANE_GAP);
        let content = match workspace.split_axis {
            SplitAxis::Horizontal => h_flex()
                .flex_1()
                .gap(gap)
                .children(panes)
                .into_any_element(),
            SplitAxis::Vertical => v_flex()
                .flex_1()
                .gap(gap)
                .children(panes)
                .into_any_element(),
        };

        v_flex()
            .flex_1()
            .p(px(WORKSPACE_PADDING))
            .bg(theme::terminal_bg())
            .child(content)
    }

    fn render_workspace_shell(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let content = if self
            .active_workspace()
            .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Files)
        {
            self.render_workspace_files_view(window, cx)
        } else {
            self.render_workspace_body(window, cx)
        };

        v_flex()
            .flex_1()
            .bg(theme::terminal_bg())
            .child(self.render_workspace_toolbar(window, cx))
            .when_some(self.render_snippet_prompts_panel(cx), |this, panel| {
                this.child(panel)
            })
            .when_some(self.render_paste_confirmation(cx), |this, banner| {
                this.child(banner)
            })
            .child(self.render_quick_actions_bar(window, cx))
            .when_some(self.render_workspace_search(window, cx), |this, search| {
                this.child(search)
            })
            .when_some(
                self.render_workspace_autocomplete(window, cx),
                |this, autocomplete| this.child(autocomplete),
            )
            .child(content)
    }

    fn render_snippet_prompts_panel(&self, cx: &Context<Self>) -> Option<Div> {
        let prompts = self.pending_snippet_prompts.as_ref()?;
        let preview: SharedString = prompts
            .command
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>()
            .into();
        Some(
            v_flex()
                .w_full()
                .px(px(18.))
                .py(px(10.))
                .gap_2()
                .bg(theme::with_alpha(theme::accent(), 0.16))
                .border_b_1()
                .border_color(theme::with_alpha(theme::accent(), 0.45))
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_semibold()
                                        .text_color(theme::text_on_dark())
                                        .child("Snippet prompts"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted_dark())
                                        .child(format!("Command: {preview}")),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("snippet-prompts-run")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label("Run")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_snippet_prompts(cx);
                                        })),
                                )
                                .child(
                                    Button::new("snippet-prompts-cancel")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_snippet_prompts(cx);
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .children(prompts.fields.iter().map(|field| {
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_on_dark())
                                        .child(field.name.clone()),
                                )
                                .child(Input::new(&field.input).small())
                                .into_any_element()
                        })),
                ),
        )
    }

    fn render_paste_confirmation(&self, cx: &Context<Self>) -> Option<Div> {
        let pending = self.pending_paste.as_ref()?;
        let line_count = pending.text.matches('\n').count() + 1;
        let preview = pending
            .text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        Some(
            h_flex()
                .w_full()
                .px(px(18.))
                .py(px(8.))
                .gap_2()
                .items_center()
                .justify_between()
                .bg(theme::with_alpha(theme::warning(), 0.16))
                .border_b_1()
                .border_color(theme::with_alpha(theme::warning(), 0.45))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_medium()
                                .text_color(theme::text_on_dark())
                                .child(format!("Paste {line_count} lines into the active pane?")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted_dark())
                                .child(format!("First line: {preview}…")),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("paste-confirm")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label("Paste")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_pending_paste(cx);
                                })),
                        )
                        .child(
                            Button::new("paste-cancel")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_pending_paste(cx);
                                })),
                        ),
                ),
        )
    }
}

impl Render for TermiRustApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.active_workspace_id.is_some() {
            self.render_workspace_shell(window, cx).into_any_element()
        } else {
            self.render_library_shell(window, cx).into_any_element()
        };

        div()
            .size_full()
            .bg(theme::app_bg())
            .font_family(cx.theme().font_family.clone())
            .text_color(theme::text_main())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_global_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                v_flex()
                    .size_full()
                    .child(self.render_top_chrome(window, cx))
                    .child(content)
                    .child({
                        let active_count = self.panes.iter().filter(|p| p.connected).count();
                        let is_workspace = self.active_workspace_id.is_some();
                        let muted_color = if is_workspace {
                            theme::text_muted_dark()
                        } else {
                            theme::text_muted()
                        };

                        h_flex()
                            .h(px(theme::STATUS_HEIGHT))
                            .px(px(14.))
                            .gap_2()
                            .items_center()
                            .justify_between()
                            .bg(if is_workspace {
                                theme::chrome_bg()
                            } else {
                                theme::library_sidebar()
                            })
                            .border_t_1()
                            .border_color(if is_workspace {
                                theme::border_dark()
                            } else {
                                theme::border()
                            })
                            .child(
                                h_flex()
                                    .gap(px(6.))
                                    .items_center()
                                    .when(!self.error_message.is_empty(), |this| {
                                        this.child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(12.))
                                                .text_color(theme::danger()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(if !self.error_message.is_empty() {
                                                theme::danger()
                                            } else {
                                                muted_color
                                            })
                                            .child(if !self.error_message.is_empty() {
                                                self.error_message.clone()
                                            } else {
                                                self.status_message.clone()
                                            }),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap(px(10.))
                                    .items_center()
                                    .child(
                                        h_flex()
                                            .gap(px(5.))
                                            .items_center()
                                            .when(active_count > 0, |this| {
                                                this.child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded(px(999.))
                                                        .bg(theme::success()),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(muted_color)
                                                    .child(format!(
                                                        "{} {}",
                                                        active_count,
                                                        if active_count == 1 {
                                                            "session"
                                                        } else {
                                                            "sessions"
                                                        }
                                                    )),
                                            ),
                                    )
                                    .when(is_workspace, |this| {
                                        this.child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme::with_alpha(muted_color, 0.6))
                                                .child(format!(
                                                    "{}+F Search  {}+K Commands  {}+W Close",
                                                    primary_shortcut_label(),
                                                    primary_shortcut_label(),
                                                    primary_shortcut_label()
                                                )),
                                        )
                                    }),
                            )
                    }),
            )
            .when(self.show_editor_panel, |this| {
                this.child(self.render_editor_dialog(window, cx))
            })
            .when(self.show_command_palette, |this| {
                this.child(self.render_command_palette(window, cx))
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

        if !event.keystroke.modifiers.shift && event.keystroke.key.as_str() == "d" {
            if let Some(pane_id) = self.active_pane().map(|pane| pane.id) {
                self.duplicate_pane(pane_id, window, cx);
                return true;
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

    fn render_command_palette(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let candidates = self.command_palette_candidates(cx);
        let selected_index = self.selected_command_palette_index(candidates.len());
        let query = self.command_palette_query(cx);

        div()
            .id("command-palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(88.))
            .bg(theme::modal_scrim())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_command_palette(window, cx);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_command_palette_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                v_flex()
                    .id("command-palette-card")
                    .w(px(720.))
                    .max_w(relative(0.9))
                    .max_h(px(560.))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .border_b_1()
                            .border_color(theme::border())
                            .bg(theme::with_alpha(theme::hover(), 0.5))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child("Command Palette"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(format!(
                                                "{}+K toggle  ↑/↓ move  Enter run",
                                                primary_shortcut_label()
                                            )),
                                    ),
                            )
                            .child(Input::new(&self.shell_inputs.command_palette).w_full()),
                    )
                    .child(
                        v_flex()
                            .max_h(px(400.))
                            .overflow_y_scrollbar()
                            .p_3()
                            .gap_2()
                            .when(!candidates.is_empty(), |this| {
                                this.children(candidates.iter().enumerate().map(|(index, candidate)| {
                                    let command = candidate.command.clone();
                                    let selected = index == selected_index;
                                    h_flex()
                                        .id(("command-palette-item", index))
                                        .justify_between()
                                        .items_start()
                                        .gap_3()
                                        .p_3()
                                        .rounded(px(12.))
                                        .bg(if selected {
                                            theme::with_alpha(theme::accent(), 0.1)
                                        } else {
                                            theme::with_alpha(theme::hover(), 0.72)
                                        })
                                        .border_1()
                                        .border_color(if selected {
                                            theme::with_alpha(theme::accent(), 0.42)
                                        } else {
                                            theme::border()
                                        })
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme::hover()))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.run_command_in_active_pane(
                                                &command,
                                                "Command sent to the active session.",
                                                cx,
                                            ) {
                                                this.close_command_palette(window, cx);
                                            }
                                        }))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .gap(px(4.))
                                                .child(
                                                    h_flex()
                                                        .justify_between()
                                                        .items_center()
                                                        .gap_3()
                                                        .child(
                                                            div()
                                                                .text_size(px(14.))
                                                                .font_semibold()
                                                                .text_color(theme::text_main())
                                                                .child(candidate.title.clone()),
                                                        )
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .when(candidate.pinned, |this| {
                                                                    this.child(self.status_badge(
                                                                        "Pinned",
                                                                        theme::library_bg(),
                                                                        theme::warning(),
                                                                    ))
                                                                })
                                                                .child(self.status_badge(
                                                                    candidate.source.label(),
                                                                    theme::library_bg(),
                                                                    match candidate.source {
                                                                        AutocompleteSource::Path => {
                                                                            theme::warning()
                                                                        }
                                                                        AutocompleteSource::Context => {
                                                                            theme::accent()
                                                                        }
                                                                        AutocompleteSource::Argument => {
                                                                            theme::warning()
                                                                        }
                                                                        AutocompleteSource::History => {
                                                                            theme::accent()
                                                                        }
                                                                        AutocompleteSource::Snippet => {
                                                                            theme::success()
                                                                        }
                                                                        AutocompleteSource::Builtin => {
                                                                            theme::slate()
                                                                        }
                                                                    },
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .text_color(theme::text_muted())
                                                        .child(candidate.detail.clone()),
                                                ),
                                        )
                                        .into_any_element()
                                }))
                            })
                            .when(candidates.is_empty(), |this| {
                                this.child(
                                    v_flex()
                                        .items_center()
                                        .justify_center()
                                        .p_8()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::Search)
                                                .size(px(24.))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.45)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .font_medium()
                                                .text_color(theme::text_muted())
                                                .child(if query.is_empty() {
                                                    "No commands yet"
                                                } else {
                                                    "No matching commands"
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                                .child(if query.is_empty() {
                                                    "Run a few commands or save snippets to build the palette."
                                                } else {
                                                    "Try a command prefix, snippet name, or recent task."
                                                }),
                                        ),
                                )
                            }),
                    ),
            )
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
                    .child(v_flex().px_5().pb_5().child(self.render_editor_panel(cx))),
            )
    }
}

#[derive(Clone, Copy)]
enum MouseEventKind {
    Press,
    Move { dragging: bool },
    Release,
    Wheel { delta: ScrollDelta },
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

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')
}

fn short_host_key(key: &str) -> String {
    if key.len() <= 40 {
        return key.to_string();
    }

    format!("{}…{}", &key[..18], &key[key.len() - 18..])
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
    }
}

fn selection_contains(selection: Option<SelectionRange>, row: usize, col: usize) -> bool {
    let Some(selection) = selection.and_then(normalized_selection) else {
        return false;
    };
    let current = TerminalCellPos {
        row: row as u16,
        col: col as u16,
    };

    compare_cell_pos(current, selection.anchor) != Ordering::Less
        && compare_cell_pos(current, selection.head) == Ordering::Less
}

fn normalized_selection(selection: SelectionRange) -> Option<SelectionRange> {
    let start = selection.anchor;
    let end_inclusive = selection.head;
    if start == end_inclusive {
        return None;
    }

    if compare_cell_pos(start, end_inclusive) == Ordering::Less {
        Some(SelectionRange {
            anchor: start,
            head: TerminalCellPos {
                row: end_inclusive.row,
                col: end_inclusive.col.saturating_add(1),
            },
        })
    } else {
        Some(SelectionRange {
            anchor: end_inclusive,
            head: TerminalCellPos {
                row: start.row,
                col: start.col.saturating_add(1),
            },
        })
    }
}

fn compare_cell_pos(left: TerminalCellPos, right: TerminalCellPos) -> Ordering {
    match left.row.cmp(&right.row) {
        Ordering::Equal => left.col.cmp(&right.col),
        ordering => ordering,
    }
}

fn style_for_render(
    cell: &TerminalCell,
    selected: bool,
    matched: bool,
    active_match: bool,
) -> TerminalStyle {
    let mut style = cell.style;
    if matched {
        style.bg = if active_match {
            theme::terminal_search_active_match_bg()
        } else {
            theme::terminal_search_match_bg()
        };
    }
    if selected {
        style.bg = theme::terminal_selection_bg();
        style.fg = theme::terminal_selection_fg();
    }
    style
}

fn default_terminal_style() -> TerminalStyle {
    TerminalStyle {
        fg: theme::text_on_dark(),
        bg: theme::terminal_bg(),
        bold: false,
        italic: false,
        underline: false,
    }
}

fn display_terminal_text(text: &str) -> SharedString {
    if text.is_empty() {
        return "\u{00a0}".into();
    }

    text.replace(' ', "\u{00a0}").into()
}

fn encode_terminal_input(keystroke: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    if keystroke.modifiers.function || keystroke.key.is_empty() {
        return None;
    }

    let mut bytes = if keystroke.modifiers.control {
        vec![encode_control_char(&keystroke.key)?]
    } else {
        match keystroke.key.as_str() {
            "enter" => b"\r".to_vec(),
            "backspace" => vec![0x7f],
            "tab" if keystroke.modifiers.shift => b"\x1b[Z".to_vec(),
            "tab" => b"\t".to_vec(),
            "escape" => vec![0x1b],
            "up" => {
                if application_cursor {
                    b"\x1bOA".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                }
            }
            "down" => {
                if application_cursor {
                    b"\x1bOB".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                }
            }
            "right" => {
                if application_cursor {
                    b"\x1bOC".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                }
            }
            "left" => {
                if application_cursor {
                    b"\x1bOD".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                }
            }
            "home" => {
                if application_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            }
            "end" => {
                if application_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            }
            "insert" => b"\x1b[2~".to_vec(),
            "delete" => b"\x1b[3~".to_vec(),
            "pageup" => b"\x1b[5~".to_vec(),
            "pagedown" => b"\x1b[6~".to_vec(),
            "space" => b" ".to_vec(),
            _ => keystroke.key_char.as_ref()?.as_bytes().to_vec(),
        }
    };

    if keystroke.modifiers.alt {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend(bytes);
        bytes = prefixed;
    }

    Some(bytes)
}

fn encode_control_char(key: &str) -> Option<u8> {
    if key.len() == 1 {
        let ch = key.as_bytes()[0];
        return match ch {
            b'a'..=b'z' => Some(ch & 0x1f),
            b'2' | b'@' => Some(0),
            b'3' | b'[' => Some(27),
            b'4' | b'\\' => Some(28),
            b'5' | b']' => Some(29),
            b'6' | b'^' => Some(30),
            b'7' | b'_' | b'/' => Some(31),
            _ => None,
        };
    }

    match key {
        "space" => Some(0),
        "enter" => Some(b'\r'),
        "backspace" => Some(0x7f),
        _ => None,
    }
}

fn encode_mouse_report(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    kind: MouseEventKind,
    pos: TerminalCellPos,
    modifiers: Modifiers,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }

    let mut button_code = modifier_bits(modifiers);
    let terminator = match kind {
        MouseEventKind::Press => {
            if mode == MouseProtocolMode::Press
                || mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 0;
                'M'
            } else {
                return None;
            }
        }
        MouseEventKind::Release => {
            if mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 3;
                'm'
            } else {
                return None;
            }
        }
        MouseEventKind::Move { dragging } => match mode {
            MouseProtocolMode::ButtonMotion if dragging => {
                button_code += 32;
                'M'
            }
            MouseProtocolMode::AnyMotion => {
                button_code += 35;
                'M'
            }
            _ => return None,
        },
        MouseEventKind::Wheel { delta } => {
            let direction = match delta {
                ScrollDelta::Lines(delta) => delta.y,
                ScrollDelta::Pixels(delta) => {
                    let value: f32 = delta.y.into();
                    value
                }
            };
            if direction < 0.0 {
                button_code += 64;
            } else if direction > 0.0 {
                button_code += 65;
            } else {
                return None;
            }
            'M'
        }
    };

    let x = pos.col as u32 + 1;
    let y = pos.row as u32 + 1;

    match encoding {
        MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{};{};{}{}", button_code, x, y, terminator).into_bytes())
        }
        MouseProtocolEncoding::Default => Some(vec![
            0x1b,
            b'[',
            b'M',
            (button_code + 32) as u8,
            (x + 32) as u8,
            (y + 32) as u8,
        ]),
        MouseProtocolEncoding::Utf8 => {
            let mut bytes = b"\x1b[M".to_vec();
            bytes.extend(char::from_u32(button_code + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(x + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(y + 32)?.to_string().into_bytes());
            Some(bytes)
        }
    }
}

fn modifier_bits(modifiers: Modifiers) -> u32 {
    let mut bits = 0;
    if modifiers.shift {
        bits += 4;
    }
    if modifiers.alt {
        bits += 8;
    }
    if modifiers.control {
        bits += 16;
    }
    bits
}

fn remote_parent_path(path: &str) -> Option<String> {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() || path == "." || path == "/" {
        return None;
    }

    if let Some((parent, _)) = path.rsplit_once('/') {
        if parent.is_empty() {
            Some("/".to_string())
        } else {
            Some(parent.to_string())
        }
    } else {
        Some(".".to_string())
    }
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn collect_autocomplete_candidates(
    input: &str,
    command_history: &[String],
    scoped_command_history: &[SavedCommandHistoryEntry],
    scope_key: &str,
    snippets: &[SavedSnippet],
    path_context: Option<&PathSuggestionContext>,
    output_context: Option<&OutputSuggestionContext>,
) -> Vec<AutocompleteCandidate> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    #[derive(Clone)]
    struct ScoredAutocompleteCandidate {
        candidate: AutocompleteCandidate,
        match_kind: AutocompleteMatchKind,
        snippet_priority: u8,
        ordinal: usize,
    }

    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path_suggestions) =
        collect_path_autocomplete_candidates(input, path_context, command_history, snippets)
    {
        for (ordinal, candidate) in path_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal,
                });
            }
        }
    }

    if let Some(context_suggestions) =
        collect_context_autocomplete_candidates(input, output_context)
    {
        for (ordinal, candidate) in context_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal: ordinal + 100,
                });
            }
        }
    }

    if let Some(argument_suggestions) = collect_argument_autocomplete_candidates(input) {
        for (ordinal, candidate) in argument_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal: ordinal + 200,
                });
            }
        }
    }

    for (ordinal, entry) in scoped_command_history
        .iter()
        .rev()
        .filter(|entry| entry.scope_key == scope_key)
        .enumerate()
    {
        let command = entry.command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::History,
                    scope_label: Some(if entry.scope_label.trim().is_empty() {
                        "This target".to_string()
                    } else {
                        entry.scope_label.clone()
                    }),
                },
                match_kind,
                snippet_priority: 1,
                ordinal,
            });
        }
    }

    for (ordinal, command) in command_history.iter().rev().enumerate() {
        let command = command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::History,
                    scope_label: None,
                },
                match_kind,
                snippet_priority: 1,
                ordinal: ordinal + scoped_command_history.len(),
            });
        }
    }

    for (ordinal, snippet) in snippets.iter().enumerate() {
        let command = snippet.command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::Snippet,
                    scope_label: None,
                },
                match_kind,
                snippet_priority: if snippet.pinned { 0 } else { 1 },
                ordinal: ordinal + scoped_command_history.len() + command_history.len(),
            });
        }
    }

    for (ordinal, template) in builtin_command_templates().iter().enumerate() {
        let key = template.command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: template.command.to_string(),
                    source: template.source,
                    scope_label: Some(template.detail.to_string()),
                },
                match_kind,
                snippet_priority: 1,
                ordinal: ordinal
                    + scoped_command_history.len()
                    + command_history.len()
                    + snippets.len(),
            });
        }
    }

    suggestions.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| {
                left.candidate
                    .source
                    .priority()
                    .cmp(&right.candidate.source.priority())
            })
            .then_with(|| left.snippet_priority.cmp(&right.snippet_priority))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
            .then_with(|| {
                left.candidate
                    .command
                    .to_ascii_lowercase()
                    .cmp(&right.candidate.command.to_ascii_lowercase())
            })
    });

    suggestions
        .into_iter()
        .take(6)
        .map(|candidate| candidate.candidate)
        .collect()
}

fn collect_command_palette_candidates(
    query: &str,
    command_history: &[String],
    scoped_command_history: &[SavedCommandHistoryEntry],
    scope_key: &str,
    snippets: &[SavedSnippet],
    output_context: Option<&OutputSuggestionContext>,
) -> Vec<CommandPaletteCandidate> {
    let query = query.trim().to_ascii_lowercase();

    #[derive(Clone)]
    struct ScoredPaletteCandidate {
        candidate: CommandPaletteCandidate,
        match_kind: AutocompleteMatchKind,
        ordinal: usize,
        source_priority: u8,
    }

    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    for (ordinal, entry) in scoped_command_history
        .iter()
        .rev()
        .filter(|entry| entry.scope_key == scope_key)
        .enumerate()
    {
        let command = entry.command.trim();
        let Some(match_kind) = palette_match_kind(&query, &[command, &entry.scope_label]) else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            let scope = if entry.scope_label.trim().is_empty() {
                "This target".to_string()
            } else {
                entry.scope_label.clone()
            };
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: command.to_string(),
                    title: command.to_string(),
                    detail: format!("History • {scope}"),
                    source: AutocompleteSource::History,
                    pinned: false,
                },
                match_kind,
                ordinal,
                source_priority: 0,
            });
        }
    }

    for (ordinal, command) in command_history.iter().rev().enumerate() {
        let command = command.trim();
        let Some(match_kind) = palette_match_kind(&query, &[command]) else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: command.to_string(),
                    title: command.to_string(),
                    detail: "Recent command".to_string(),
                    source: AutocompleteSource::History,
                    pinned: false,
                },
                match_kind,
                ordinal,
                source_priority: 1,
            });
        }
    }

    for (ordinal, snippet) in snippets.iter().enumerate() {
        let command = snippet.command.trim();
        let title = snippet.display_name();
        let Some(match_kind) = palette_match_kind(&query, &[command, &title, &snippet.group])
        else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            let mut detail = if snippet.pinned {
                format!("Pinned snippet • {}", command)
            } else {
                format!("Snippet • {}", command)
            };
            if !snippet.group.trim().is_empty() {
                detail = if snippet.pinned {
                    format!("Pinned snippet • {} • {}", snippet.group.trim(), command)
                } else {
                    format!("Snippet • {} • {}", snippet.group.trim(), command)
                };
            }
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: command.to_string(),
                    title,
                    detail,
                    source: AutocompleteSource::Snippet,
                    pinned: snippet.pinned,
                },
                match_kind,
                ordinal,
                source_priority: if snippet.pinned { 1 } else { 2 },
            });
        }
    }

    if let Some(context_suggestions) =
        collect_context_command_templates(query.as_str(), output_context)
    {
        for template in context_suggestions {
            let Some(match_kind) =
                palette_match_kind(&query, &[&template.command, &template.detail])
            else {
                continue;
            };
            let key = template.command.to_ascii_lowercase();
            if seen.insert(key) {
                suggestions.push(ScoredPaletteCandidate {
                    candidate: CommandPaletteCandidate {
                        command: template.command.clone(),
                        title: template.command,
                        detail: template.detail,
                        source: AutocompleteSource::Context,
                        pinned: false,
                    },
                    match_kind,
                    ordinal: template.ordinal,
                    source_priority: 2u8.saturating_add(template.rank),
                });
            }
        }
    }

    for (ordinal, template) in builtin_command_templates().iter().enumerate() {
        let Some(match_kind) = palette_match_kind(&query, &[template.command, template.detail])
        else {
            continue;
        };
        let key = template.command.to_ascii_lowercase();
        if seen.insert(key) {
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: template.command.to_string(),
                    title: template.command.to_string(),
                    detail: template.detail.to_string(),
                    source: template.source,
                    pinned: false,
                },
                match_kind,
                ordinal,
                source_priority: match template.source {
                    AutocompleteSource::Argument => 3,
                    _ => 4,
                },
            });
        }
    }

    suggestions.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| left.source_priority.cmp(&right.source_priority))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
            .then_with(|| {
                left.candidate
                    .title
                    .to_ascii_lowercase()
                    .cmp(&right.candidate.title.to_ascii_lowercase())
            })
    });

    suggestions
        .into_iter()
        .take(10)
        .map(|candidate| candidate.candidate)
        .collect()
}

fn collect_path_autocomplete_candidates(
    input: &str,
    path_context: Option<&PathSuggestionContext>,
    command_history: &[String],
    snippets: &[SavedSnippet],
) -> Option<Vec<AutocompleteCandidate>> {
    let query = path_query_context(input)?;
    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    let mut push_candidate =
        |path_value: String, scope_label: Option<String>, is_dir: bool, ordinal: usize| {
            let mut candidate_path = path_value;
            if is_dir && !candidate_path.ends_with('/') {
                candidate_path.push('/');
            }
            let Some(match_kind) = path_match_kind(&query.fragment, &candidate_path) else {
                return None;
            };
            let full_command = format!("{}{}", query.prefix, candidate_path);
            if seen.insert(full_command.to_ascii_lowercase()) {
                suggestions.push((
                    AutocompleteCandidate {
                        command: full_command,
                        source: AutocompleteSource::Path,
                        scope_label,
                    },
                    match_kind,
                    ordinal,
                ));
            }
            Some(())
        };

    let mut ordinal = 0usize;
    if let Some(context) = path_context {
        let current_path = context
            .current_path
            .clone()
            .unwrap_or_else(|| ".".to_string());
        let scope = Some(format!("Files • {}", current_path));

        for entry in &context.entries {
            let candidate_path = if query.fragment.starts_with('/') {
                entry.path.clone()
            } else if query.fragment.starts_with("./") {
                format!("./{}", entry.name)
            } else {
                entry.name.clone()
            };
            let _ = push_candidate(candidate_path, scope.clone(), entry.is_dir, ordinal);
            ordinal += 1;
        }

        if let Some(startup_directory) = context.startup_directory.clone() {
            let _ = push_candidate(
                startup_directory,
                Some("Startup path".to_string()),
                true,
                ordinal,
            );
            ordinal += 1;
        }
        if let Some(current_path) = context.current_path.clone() {
            let _ = push_candidate(
                current_path.clone(),
                Some("Current directory".to_string()),
                true,
                ordinal,
            );
            ordinal += 1;
            if let Some(parent) = remote_parent_path(&current_path) {
                let _ = push_candidate(parent, Some("Parent directory".to_string()), true, ordinal);
                ordinal += 1;
            }
        }
    }

    for path in command_history
        .iter()
        .flat_map(|command| extract_path_tokens(command))
        .chain(
            snippets
                .iter()
                .flat_map(|snippet| extract_path_tokens(&snippet.command)),
        )
    {
        let is_dir = path.ends_with('/');
        let _ = push_candidate(path, Some("Recent path".to_string()), is_dir, ordinal);
        ordinal += 1;
    }

    suggestions.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| {
                left.0
                    .command
                    .to_ascii_lowercase()
                    .cmp(&right.0.command.to_ascii_lowercase())
            })
    });

    if suggestions.is_empty() {
        None
    } else {
        Some(
            suggestions
                .into_iter()
                .take(6)
                .map(|(candidate, _, _)| candidate)
                .collect(),
        )
    }
}

fn collect_argument_autocomplete_candidates(input: &str) -> Option<Vec<AutocompleteCandidate>> {
    let query = input.trim();
    if query.is_empty() || query.contains('\n') {
        return None;
    }

    let first = query.split_whitespace().next()?;
    let has_family_templates = builtin_command_templates().iter().any(|template| {
        template.source == AutocompleteSource::Argument
            && template.command.starts_with(first)
            && (template.command == first || template.command.starts_with(&format!("{first} ")))
    });
    if !has_family_templates {
        return None;
    }

    let query_lower = query.to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut suggestions = builtin_command_templates()
        .iter()
        .filter(|template| template.source == AutocompleteSource::Argument)
        .filter_map(|template| {
            let command_lower = template.command.to_ascii_lowercase();
            let match_kind = autocomplete_match_kind(&query_lower, &command_lower)?;
            if !command_lower.starts_with(&first.to_ascii_lowercase())
                || command_lower == query_lower
            {
                return None;
            }
            if !seen.insert(command_lower.clone()) {
                return None;
            }
            Some((
                AutocompleteCandidate {
                    command: template.command.to_string(),
                    source: AutocompleteSource::Argument,
                    scope_label: Some(template.detail.to_string()),
                },
                match_kind,
            ))
        })
        .collect::<Vec<_>>();

    suggestions.sort_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| {
            left.0
                .command
                .to_ascii_lowercase()
                .cmp(&right.0.command.to_ascii_lowercase())
        })
    });

    if suggestions.is_empty() {
        None
    } else {
        Some(
            suggestions
                .into_iter()
                .take(6)
                .map(|(candidate, _)| candidate)
                .collect(),
        )
    }
}

fn collect_context_autocomplete_candidates(
    input: &str,
    output_context: Option<&OutputSuggestionContext>,
) -> Option<Vec<AutocompleteCandidate>> {
    let suggestions = collect_context_command_templates(input, output_context)?;
    let mut candidates = suggestions
        .into_iter()
        .filter_map(|template| {
            let command = template.command;
            let command_lower = command.to_ascii_lowercase();
            let match_kind =
                autocomplete_match_kind(&input.trim().to_ascii_lowercase(), &command_lower)?;
            Some((
                AutocompleteCandidate {
                    command,
                    source: AutocompleteSource::Context,
                    scope_label: Some(template.detail),
                },
                match_kind,
                template.rank,
                template.ordinal,
            ))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| {
            left.2
                .cmp(&right.2)
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| {
                    left.0
                        .command
                        .to_ascii_lowercase()
                        .cmp(&right.0.command.to_ascii_lowercase())
                })
        })
    });

    if candidates.is_empty() {
        None
    } else {
        Some(
            candidates
                .into_iter()
                .take(6)
                .map(|(candidate, _, _, _)| candidate)
                .collect(),
        )
    }
}

fn collect_context_command_templates(
    input: &str,
    output_context: Option<&OutputSuggestionContext>,
) -> Option<Vec<ContextCommandTemplate>> {
    let output_context = output_context?;
    let raw_query = input.trim_end_matches(['\r', '\n']);
    let query = raw_query.trim();
    if query.is_empty() || raw_query.contains('\n') || output_context.recent_lines.is_empty() {
        return None;
    }

    let query_lower = query.to_ascii_lowercase();
    let path_hint = current_path_hint(output_context.current_path.as_deref());
    let mut templates = Vec::new();
    let mut push_templates = |prefix: &str, targets: Vec<String>, kind: &str| {
        for (ordinal, target) in targets.into_iter().enumerate() {
            templates.push(ContextCommandTemplate {
                command: format!("{prefix}{target}"),
                detail: context_detail(kind, output_context.current_path.as_deref()),
                rank: context_target_rank(&target, path_hint.as_deref()),
                ordinal,
            });
        }
    };

    if matches_command_prefix(&query_lower, "git checkout")
        || matches_command_prefix(&query_lower, "git switch")
    {
        let prefix = if matches_command_prefix(&query_lower, "git switch") {
            "git switch "
        } else {
            "git checkout "
        };
        push_templates(
            prefix,
            extract_git_branch_targets(&output_context.recent_lines),
            "Git branch",
        );
    } else if matches_command_prefix(&query_lower, "git diff")
        || matches_command_prefix(&query_lower, "git log")
    {
        let prefix = if matches_command_prefix(&query_lower, "git log") {
            "git log "
        } else {
            "git diff "
        };
        push_templates(
            prefix,
            extract_git_branch_targets(&output_context.recent_lines),
            "Git branch",
        );
    } else if matches_command_prefix(&query_lower, "docker logs")
        || matches_command_prefix(&query_lower, "docker inspect")
        || matches_command_prefix(&query_lower, "docker stop")
        || matches_command_prefix(&query_lower, "docker restart")
        || matches_command_prefix(&query_lower, "docker rm")
        || matches_command_prefix(&query_lower, "docker exec -it")
    {
        let prefix = if matches_command_prefix(&query_lower, "docker exec -it") {
            "docker exec -it "
        } else if matches_command_prefix(&query_lower, "docker inspect") {
            "docker inspect "
        } else if matches_command_prefix(&query_lower, "docker stop") {
            "docker stop "
        } else if matches_command_prefix(&query_lower, "docker restart") {
            "docker restart "
        } else if matches_command_prefix(&query_lower, "docker rm") {
            "docker rm "
        } else {
            "docker logs "
        };
        push_templates(
            prefix,
            extract_docker_targets(&output_context.recent_lines),
            "Docker target",
        );
    } else if matches_command_prefix(&query_lower, "kubectl logs")
        || matches_command_prefix(&query_lower, "kubectl describe pod")
        || matches_command_prefix(&query_lower, "kubectl exec -it")
    {
        let prefix = if matches_command_prefix(&query_lower, "kubectl describe pod") {
            "kubectl describe pod "
        } else if matches_command_prefix(&query_lower, "kubectl exec -it") {
            "kubectl exec -it "
        } else {
            "kubectl logs "
        };
        push_templates(
            prefix,
            extract_kubernetes_pod_targets(&output_context.recent_lines),
            "Kubernetes pod",
        );
    } else if matches_command_prefix(&query_lower, "systemctl status")
        || matches_command_prefix(&query_lower, "systemctl restart")
        || matches_command_prefix(&query_lower, "systemctl reload")
        || matches_command_prefix(&query_lower, "journalctl -u")
        || matches_command_prefix(&query_lower, "journalctl -f -u")
    {
        let prefix = if matches_command_prefix(&query_lower, "systemctl restart") {
            "systemctl restart "
        } else if matches_command_prefix(&query_lower, "systemctl reload") {
            "systemctl reload "
        } else if matches_command_prefix(&query_lower, "journalctl -f -u") {
            "journalctl -f -u "
        } else if matches_command_prefix(&query_lower, "journalctl -u") {
            "journalctl -u "
        } else {
            "systemctl status "
        };
        push_templates(
            prefix,
            extract_systemd_unit_targets(&output_context.recent_lines),
            "Systemd unit",
        );
    }

    if templates.is_empty() {
        None
    } else {
        let mut seen = HashSet::new();
        let mut deduped = templates
            .into_iter()
            .filter(|template| seen.insert(template.command.to_ascii_lowercase()))
            .collect::<Vec<_>>();
        deduped.sort_by(|left, right| {
            left.rank
                .cmp(&right.rank)
                .then_with(|| left.ordinal.cmp(&right.ordinal))
                .then_with(|| {
                    left.command
                        .to_ascii_lowercase()
                        .cmp(&right.command.to_ascii_lowercase())
                })
        });
        Some(deduped)
    }
}

fn context_detail(kind: &str, current_path: Option<&str>) -> String {
    let Some(current_path) = current_path.map(str::trim).filter(|path| !path.is_empty()) else {
        return format!("Recent output • {kind}");
    };
    format!("Recent output • {kind} • {current_path}")
}

fn matches_command_prefix(query: &str, prefix: &str) -> bool {
    query == prefix || query.starts_with(&format!("{prefix} "))
}

fn current_path_hint(current_path: Option<&str>) -> Option<String> {
    let generic_segments = [
        "current", "releases", "release", "shared", "srv", "var", "www", "opt", "home", "users",
        "user", "app", "apps", "service", "services", "project", "projects",
    ];

    let mut segments = current_path_segments(current_path);
    segments.reverse();
    for segment in segments {
        if !generic_segments.contains(&segment.as_str()) {
            return Some(segment);
        }
    }

    current_path_segments(current_path).into_iter().last()
}

fn current_path_segments(current_path: Option<&str>) -> Vec<String> {
    current_path
        .unwrap_or_default()
        .split(['/', '\\'])
        .map(|segment| segment.trim().to_ascii_lowercase())
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn context_target_rank(target: &str, path_hint: Option<&str>) -> u8 {
    let Some(path_hint) = path_hint.filter(|hint| !hint.is_empty()) else {
        return 1;
    };
    let target = target.to_ascii_lowercase();
    if target == path_hint
        || target.starts_with(path_hint)
        || target.contains(&format!("-{path_hint}"))
        || target.contains(&format!("{path_hint}-"))
        || target.contains(&format!("/{path_hint}"))
        || target.contains(&format!("{path_hint}."))
    {
        0
    } else {
        1
    }
}

fn extract_git_branch_targets(lines: &[String]) -> Vec<String> {
    let mut branches = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if let Some(branch) = trimmed.strip_prefix("On branch ") {
            if let Some(branch) =
                clean_context_token(branch.split_whitespace().next().unwrap_or_default())
            {
                if seen.insert(branch.clone()) {
                    branches.push(branch);
                }
            }
            continue;
        }

        if let Some(rest) = trimmed
            .strip_prefix("* ")
            .or_else(|| trimmed.strip_prefix("+ "))
            .or_else(|| trimmed.strip_prefix("  "))
        {
            if let Some(branch) =
                clean_context_token(rest.split_whitespace().next().unwrap_or_default())
            {
                if branch != "HEAD" && seen.insert(branch.clone()) {
                    branches.push(branch);
                }
            }
        }
    }

    branches
}

fn extract_docker_targets(lines: &[String]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("CONTAINER ID") {
            continue;
        }

        let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
        if tokens.len() < 2 || !looks_like_hex_id(tokens[0]) {
            continue;
        }
        if let Some(target) = clean_context_token(tokens.last().copied().unwrap_or_default()) {
            if seen.insert(target.clone()) {
                targets.push(target);
            }
        }
    }

    targets
}

fn extract_kubernetes_pod_targets(lines: &[String]) -> Vec<String> {
    let mut pods = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("NAME ")
            || trimmed.starts_with("No resources found")
        {
            continue;
        }

        let first = trimmed.split_whitespace().next().unwrap_or_default();
        let Some(pod) = clean_context_token(first) else {
            continue;
        };
        if !(pod.contains('-')
            || trimmed.contains("Running")
            || trimmed.contains("Pending")
            || trimmed.contains("Completed")
            || trimmed.contains("CrashLoopBackOff"))
        {
            continue;
        }
        if seen.insert(pod.clone()) {
            pods.push(pod);
        }
    }

    pods
}

fn extract_systemd_unit_targets(lines: &[String]) -> Vec<String> {
    let mut units = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        for token in line.split_whitespace() {
            let Some(unit) = clean_context_token(token) else {
                continue;
            };
            if !unit.ends_with(".service") {
                continue;
            }
            if seen.insert(unit.clone()) {
                units.push(unit);
            }
        }
    }

    units
}

fn clean_context_token(token: &str) -> Option<String> {
    let token = token.trim_matches(|ch: char| {
        !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_' | '.' | '/' | ':' | '@')
    });
    if token.is_empty() || token.eq_ignore_ascii_case("name") {
        None
    } else {
        Some(token.to_string())
    }
}

fn looks_like_hex_id(token: &str) -> bool {
    token.len() >= 6 && token.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn autocomplete_match_kind(query: &str, command: &str) -> Option<AutocompleteMatchKind> {
    if command.starts_with(query) {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let token_prefix = command
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '/' | '\\' | ':' | '=' | '-' | '_' | '.' | '|' | '&' | ';'
                )
        })
        .filter(|token| !token.is_empty())
        .any(|token| token.starts_with(query));
    if token_prefix {
        return Some(AutocompleteMatchKind::TokenPrefix);
    }

    if query.len() >= 2 && command.contains(query) {
        return Some(AutocompleteMatchKind::Substring);
    }

    None
}

fn palette_match_kind(query: &str, fields: &[&str]) -> Option<AutocompleteMatchKind> {
    if query.is_empty() {
        return Some(AutocompleteMatchKind::Prefix);
    }

    fields
        .iter()
        .filter_map(|field| autocomplete_match_kind(query, &field.to_ascii_lowercase()))
        .min()
}

struct PathAutocompleteQuery {
    prefix: String,
    fragment: String,
}

fn path_query_context(input: &str) -> Option<PathAutocompleteQuery> {
    let input = input.trim_end_matches(['\r', '\n']);
    if input.trim().is_empty() {
        return None;
    }

    let tokens = input.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    if input.ends_with(' ') {
        let last = tokens.last().copied().unwrap_or_default();
        if is_path_command(last) {
            return Some(PathAutocompleteQuery {
                prefix: input.to_string(),
                fragment: String::new(),
            });
        }
        return None;
    }

    let last = tokens.last().copied().unwrap_or_default();
    let previous = tokens
        .get(tokens.len().saturating_sub(2))
        .copied()
        .unwrap_or_default();
    if !is_path_like_token(last) && !is_path_command(previous) {
        return None;
    }

    let start = input.rfind(last)?;
    Some(PathAutocompleteQuery {
        prefix: input[..start].to_string(),
        fragment: last.to_string(),
    })
}

fn is_path_command(command: &str) -> bool {
    matches!(
        command,
        "cd" | "ls"
            | "cat"
            | "tail"
            | "less"
            | "more"
            | "vim"
            | "nvim"
            | "nano"
            | "rm"
            | "cp"
            | "mv"
            | "mkdir"
            | "touch"
            | "chmod"
            | "chown"
            | "source"
    )
}

fn is_path_like_token(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.contains('/')
        || token == "."
        || token == ".."
}

fn path_match_kind(fragment: &str, candidate: &str) -> Option<AutocompleteMatchKind> {
    let fragment = fragment.trim().to_ascii_lowercase();
    if fragment.is_empty() {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let candidate_lower = candidate.to_ascii_lowercase();
    if candidate_lower.starts_with(&fragment) {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let basename = candidate
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(candidate)
        .to_ascii_lowercase();
    let stripped_fragment = fragment
        .trim_start_matches("./")
        .trim_start_matches("../")
        .trim_start_matches("~/");
    if !stripped_fragment.is_empty() && basename.starts_with(stripped_fragment) {
        return Some(AutocompleteMatchKind::TokenPrefix);
    }

    if fragment.len() >= 2 && candidate_lower.contains(&fragment) {
        return Some(AutocompleteMatchKind::Substring);
    }

    None
}

fn extract_path_tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                ch.is_ascii_punctuation() && ch != '/' && ch != '.' && ch != '_' && ch != '-'
            });
            if is_path_like_token(token) {
                Some(token.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn shell_command_requires_continuation(command: &str) -> bool {
    let trimmed = command.trim_end();
    if trimmed.is_empty() {
        return false;
    }

    let trailing_backslashes = trimmed.chars().rev().take_while(|ch| *ch == '\\').count();
    if trailing_backslashes % 2 == 1 {
        return true;
    }

    if trimmed.ends_with("&&") || trimmed.ends_with("||") {
        return true;
    }

    if trimmed.ends_with('|')
        || trimmed.ends_with('(')
        || trimmed.ends_with('{')
        || trimmed.ends_with('[')
    {
        return true;
    }

    let mut single_quote = false;
    let mut double_quote = false;
    let mut backtick = false;
    let mut escaped = false;
    let mut paren_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;

    for ch in trimmed.chars() {
        if escaped {
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !single_quote => {
                escaped = true;
            }
            '\'' if !double_quote && !backtick => {
                single_quote = !single_quote;
            }
            '"' if !single_quote && !backtick => {
                double_quote = !double_quote;
            }
            '`' if !single_quote && !double_quote => {
                backtick = !backtick;
            }
            '(' if !single_quote && !double_quote && !backtick => {
                paren_depth += 1;
            }
            ')' if !single_quote && !double_quote && !backtick => {
                paren_depth = (paren_depth - 1).max(0);
            }
            '{' if !single_quote && !double_quote && !backtick => {
                brace_depth += 1;
            }
            '}' if !single_quote && !double_quote && !backtick => {
                brace_depth = (brace_depth - 1).max(0);
            }
            '[' if !single_quote && !double_quote && !backtick => {
                bracket_depth += 1;
            }
            ']' if !single_quote && !double_quote && !backtick => {
                bracket_depth = (bracket_depth - 1).max(0);
            }
            _ => {}
        }
    }

    single_quote
        || double_quote
        || backtick
        || paren_depth > 0
        || brace_depth > 0
        || bracket_depth > 0
}

fn startup_bytes_for_request(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
) -> Option<Vec<u8>> {
    if request.is_local_shell() {
        return None;
    }

    let mut lines = Vec::new();
    for (key, value) in &request.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        lines.push(format!("export {key}={}", shell_single_quote(value)));
    }
    let effective_dir = request.startup_directory.as_deref().or(default_startup_dir);
    if let Some(directory) = effective_dir {
        let directory = directory.trim();
        if !directory.is_empty() {
            lines.push(format!("cd -- {}", shell_single_quote(directory)));
        }
    }
    if let Some(command) = request.startup_command.as_deref() {
        let command = command.trim();
        if !command.is_empty() {
            lines.push(command.to_string());
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("{}\n", lines.join("\n")).into_bytes())
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn pane_recent_output_lines(pane: &SessionPane, limit: usize) -> Vec<String> {
    let mut lines = pane
        .terminal
        .all_rows_text()
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let keep_from = lines.len().saturating_sub(limit);
    lines.drain(0..keep_from);
    lines
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

fn format_count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn extract_snippet_prompt_names(command: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = command;
    while let Some(start) = rest.find("{{?") {
        let after = &rest[start + 3..];
        let Some(end_rel) = after.find("}}") else {
            break;
        };
        let name = after[..end_rel].trim().to_string();
        if !name.is_empty() && !names.iter().any(|n| n == &name) {
            names.push(name);
        }
        rest = &after[end_rel + 2..];
    }
    names
}

fn substitute_snippet_prompts(command: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("{{?") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end_rel) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end_rel].trim();
        let replacement = values
            .iter()
            .find(|(prompt, _)| prompt == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        out.push_str(&replacement);
        rest = &after[end_rel + 2..];
    }
    out.push_str(rest);
    out
}

fn substitute_snippet_placeholders(command: &str, request: &ConnectRequest) -> String {
    let host = request.host.trim().to_string();
    let user = request.username.trim().to_string();
    let port = request.port.to_string();
    let title = request.title.trim().to_string();
    let address = request.address();

    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end_rel) = after_open.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after_open[..end_rel].trim().to_ascii_uppercase();
        let replacement = match name.as_str() {
            "HOST" => Some(host.as_str()),
            "USER" | "USERNAME" => Some(user.as_str()),
            "PORT" => Some(port.as_str()),
            "TITLE" => Some(title.as_str()),
            "ADDRESS" => Some(address.as_str()),
            _ => None,
        };
        match replacement {
            Some(value) => out.push_str(value),
            None => {
                out.push_str("{{");
                out.push_str(&after_open[..end_rel]);
                out.push_str("}}");
            }
        }
        rest = &after_open[end_rel + 2..];
    }
    out.push_str(rest);
    out
}

fn current_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_relative_time(timestamp_ms: u64) -> String {
    format_relative_time_for(timestamp_ms, current_unix_millis())
}

fn format_relative_time_for(timestamp_ms: u64, now_ms: u64) -> String {
    if timestamp_ms == 0 || timestamp_ms > now_ms {
        return "just now".to_string();
    }
    let delta_secs = (now_ms - timestamp_ms) / 1000;
    if delta_secs < 60 {
        return "just now".to_string();
    }
    let minutes = delta_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{days}d ago");
    }
    if days < 30 {
        let weeks = days / 7;
        return if weeks == 1 {
            "1w ago".to_string()
        } else {
            format!("{weeks}w ago")
        };
    }
    if days < 365 {
        let months = days / 30;
        return if months == 1 {
            "1mo ago".to_string()
        } else {
            format!("{months}mo ago")
        };
    }
    let years = days / 365;
    if years == 1 {
        "1y ago".to_string()
    } else {
        format!("{years}y ago")
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_tag_values(raw: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for raw in raw.split(',') {
        let tag = raw.trim().trim_start_matches('#');
        if tag.is_empty() {
            continue;
        }
        if !tags
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            tags.push(tag.to_string());
        }
    }
    tags
}

fn format_tag_values(tags: &[String]) -> String {
    tags.join(", ")
}

fn merge_tag_values(current: &[String], inherited: &[String]) -> Vec<String> {
    let mut merged = current.to_vec();
    for tag in inherited {
        if !merged
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            merged.push(tag.clone());
        }
    }
    merged
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
        AutocompleteSource, OutputSuggestionContext, PathSuggestionContext, WorkspaceIndicators,
        WorkspaceRuntimeTone, apply_group_defaults_to_draft, collect_autocomplete_candidates,
        collect_command_palette_candidates, extract_snippet_prompt_names, format_relative_time_for,
        shell_command_requires_continuation, shell_single_quote, startup_bytes_for_request,
        substitute_snippet_placeholders, substitute_snippet_prompts, workspace_runtime_summary,
    };
    use crate::models::{
        AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DraftProfile, LocalPortForward,
        PortForwardKind, PortForwardRule, SavedHostGroup, SavedIdentity,
    };
    use crate::sftp::RemoteFileEntry;

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
}

fn builtin_command_templates() -> &'static [BuiltinCommandTemplate] {
    &[
        BuiltinCommandTemplate {
            command: "pwd",
            detail: "Print the current working directory",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "ls -la",
            detail: "List files with hidden entries and details",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "cd /var/www",
            detail: "Jump to a common web root path",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "git status",
            detail: "Show working tree status",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git pull",
            detail: "Fetch and merge from the tracked remote branch",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git fetch --all",
            detail: "Fetch all remotes without changing the working tree",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git checkout main",
            detail: "Switch to a branch or restore a path",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git diff",
            detail: "Inspect uncommitted changes",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git log --oneline --decorate -20",
            detail: "Show recent commit history in a compact view",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker ps",
            detail: "List running containers",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker logs -f",
            detail: "Stream container logs",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker exec -it",
            detail: "Open an interactive shell inside a running container",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose ps",
            detail: "List compose services and state",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose logs -f",
            detail: "Stream logs for a compose project",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose up -d",
            detail: "Start compose services in the background",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl get pods",
            detail: "List pods in the current namespace",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl logs -f",
            detail: "Stream logs from a Kubernetes pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl describe pod",
            detail: "Inspect the full state of a pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl exec -it",
            detail: "Open an interactive shell inside a pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl status",
            detail: "Inspect a systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl restart",
            detail: "Restart a systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl reload",
            detail: "Reload a unit without a full restart when supported",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "journalctl -u",
            detail: "View logs for a specific systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "journalctl -f -u",
            detail: "Follow logs for a systemd unit in real time",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "tail -f /var/log/syslog",
            detail: "Follow a log file",
            source: AutocompleteSource::Builtin,
        },
    ]
}
