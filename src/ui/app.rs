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
    HostProfile, JumpHostConnection, LocalPortForward, ProfileSource, QuickConnect,
    SavedCommandHistoryEntry, SavedIdentity, SavedSnippet, SavedState, SavedVault,
    SavedVaultMember, SavedWorkspace, SessionLogEntry, SessionLogStatus, SplitAxis, ThemePreset,
    VaultKind, VaultMemberRole,
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
const TERMINAL_INNER_PADDING_X: f32 = 20.0;
const TERMINAL_INNER_PADDING_Y: f32 = 14.0;
const MAX_SPLIT_PANES: usize = 4;
const HOST_CARD_WIDTH: f32 = 300.0;
const ICON_KEY: &str = "icons/key.svg";
const ICON_SHIELD_CHECK: &str = "icons/shield-check.svg";

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
            Self::Vaults => IconName::Building2.into(),
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
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    key_path: Entity<InputState>,
    forward_local_port: Entity<InputState>,
    forward_remote_host: Entity<InputState>,
    forward_remote_port: Entity<InputState>,
    key_passphrase: Entity<InputState>,
}

impl DraftInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            label: cx.new(|cx| InputState::new(window, cx).placeholder("New host label")),
            group: cx.new(|cx| InputState::new(window, cx).placeholder("Production / Staging")),
            tags: cx.new(|cx| InputState::new(window, cx).placeholder("prod, blue, kubernetes")),
            jump_host: cx.new(|cx| InputState::new(window, cx).placeholder("Optional saved host")),
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
        }
    }
}

struct ShellInputs {
    host_search: Entity<InputState>,
    quick_connect_password: Entity<InputState>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteSource {
    History,
    Snippet,
    Builtin,
}

impl AutocompleteSource {
    fn priority(self) -> u8 {
        match self {
            Self::History => 0,
            Self::Snippet => 1,
            Self::Builtin => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Snippet => "snippet",
            Self::Builtin => "builtin",
        }
    }
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
                    .text_size(px(12.))
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
    draft_local_forwards: Vec<LocalPortForward>,
    snippet_vault_id: Option<String>,
    draft_vault_member_role: VaultMemberRole,
    known_hosts: Arc<KnownHostStore>,
    keychain_tab: KeychainTab,
    show_command_palette: bool,
    selected_command_palette_index: usize,
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
            selected_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            selected_vault_member_id: None,
            next_session_id: 1,
            next_sftp_operation_id: 1,
            next_workspace_id: 1,
            status_message: initial_status,
            error_message: String::new(),
            draft_identity_id: None,
            draft_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            draft_local_forwards: Vec::new(),
            snippet_vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            draft_vault_member_role: VaultMemberRole::Editor,
            known_hosts,
            keychain_tab: KeychainTab::Keys,
            show_command_palette: false,
            selected_command_palette_index: 0,
            _window_bounds_subscription: None,
        };

        app.load_settings_inputs(window, cx);

        app.restore_saved_workspaces(window, cx);

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
                    .update(|_, cx| {
                        let _ = this.update(cx, |app, cx| app.process_events(cx));
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
        self.clear_backup_inputs(window, cx);
    }

    fn current_profile_draft(&self, cx: &App) -> anyhow::Result<DraftProfile> {
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

        Ok(DraftProfile {
            label: self.inputs.label.read(cx).value().to_string(),
            vault_id: self.draft_vault_id.clone(),
            group: self.inputs.group.read(cx).value().to_string(),
            tags: self.inputs.tags.read(cx).value().to_string(),
            host: self.inputs.host.read(cx).value().to_string(),
            port: self.inputs.port.read(cx).value().to_string(),
            username: self.inputs.username.read(cx).value().to_string(),
            password: self.inputs.password.read(cx).value().to_string(),
            key_path,
            identity_id,
            jump_host_id,
            saved_local_forwards: self.draft_local_forwards.clone(),
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
        })
    }

    fn set_auth_mode(&mut self, auth_mode: AuthMode, cx: &mut Context<Self>) {
        self.draft_auth_mode = auth_mode;
        self.status_message = format!("Using {} authentication.", auth_mode.label());
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

    fn set_quick_connect_password_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.quick_connect_password, value, window, cx);
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
        Self::set_input_value(&self.inputs.host, draft.host, window, cx);
        Self::set_input_value(&self.inputs.port, draft.port, window, cx);
        Self::set_input_value(&self.inputs.username, draft.username, window, cx);
        Self::set_input_value(&self.inputs.password, "", window, cx);
        Self::set_input_value(&self.inputs.key_path, draft.key_path, window, cx);
        Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
        Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
        self.draft_vault_id = Some(self.effective_vault_id(draft.vault_id.as_deref()));
        self.draft_local_forwards = draft.saved_local_forwards;
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
        Self::set_input_value(&self.inputs.host, "", window, cx);
        Self::set_input_value(&self.inputs.port, "22", window, cx);
        Self::set_input_value(&self.inputs.username, "", window, cx);
        Self::set_input_value(&self.inputs.password, "", window, cx);
        Self::set_input_value(&self.inputs.key_path, "", window, cx);
        Self::set_input_value(&self.inputs.forward_local_port, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_host, "", window, cx);
        Self::set_input_value(&self.inputs.forward_remote_port, "", window, cx);
        Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
        self.draft_vault_id = Some(self.effective_vault_id(self.selected_vault_id.as_deref()));
        self.draft_local_forwards.clear();
        self.draft_identity_id = None;
        self.selected_profile_id = None;
        self.saved.selected_profile_id = None;
        self.draft_auth_mode = AuthMode::Password;
        self.show_editor_panel = true;
        self.status_message = "Draft cleared. Define a host to save or connect.".into();
        self.error_message.clear();
        cx.notify();
    }

    fn add_draft_local_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = match self.current_profile_draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };

        match draft.parse_pending_local_forward() {
            Ok(Some(forward)) => {
                if self
                    .draft_local_forwards
                    .iter()
                    .any(|existing| existing.display_name() == forward.display_name())
                {
                    self.error_message =
                        format!("Forward rule '{}' already exists.", forward.display_name());
                    cx.notify();
                    return;
                }

                let label = forward.display_name();
                self.draft_local_forwards.push(forward);
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

    fn remove_draft_local_forward(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.draft_local_forwards.len() {
            return;
        }

        let removed = self.draft_local_forwards.remove(index);
        self.status_message = format!("Removed forward rule {}.", removed.display_name());
        self.error_message.clear();
        if self.draft_local_forwards.is_empty() {
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
        self.selected_snippet_id = None;
        self.nav_section = NavSection::Snippets;
        self.status_message = "Snippet draft cleared.".to_string();
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

        if self.send_input_bytes(pane_id, bytes, cx) {
            self.status_message = success_message.to_string();
            self.error_message.clear();
            cx.notify();
            return true;
        }
        false
    }

    fn run_snippet_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let _ = self.run_command_in_active_pane(command, "Snippet sent to the active session.", cx);
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
        profiles.sort_by_key(|profile| profile.display_name().to_ascii_lowercase());

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
                    profile.group.clone(),
                    profile.tags.join(" "),
                    profile.host.clone(),
                    profile.username.clone(),
                    profile.endpoint(),
                    profile
                        .effective_local_forwards()
                        .iter()
                        .map(LocalPortForward::display_name)
                        .collect::<Vec<_>>()
                        .join(" "),
                    vault_label,
                    jump_host_label,
                ];
                haystacks
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&query))
            })
            .collect()
    }

    fn profile_group_name(profile: &HostProfile) -> String {
        let group = profile.group.trim();
        if !group.is_empty() {
            group.to_string()
        } else if profile.source == ProfileSource::SshConfig {
            "Imported".to_string()
        } else {
            "Ungrouped".to_string()
        }
    }

    fn group_sort_key(group: &str) -> (u8, String) {
        match group {
            "Imported" => (1, group.to_ascii_lowercase()),
            "Ungrouped" => (2, group.to_ascii_lowercase()),
            _ => (0, group.to_ascii_lowercase()),
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

    fn save_settings(&mut self) {
        self.saved.ensure_settings();
        let _ = save_saved_state(&self.saved);
    }

    fn update_theme_preset(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.saved.settings.theme_preset = preset;
        theme::set_theme_preset(preset);
        self.save_settings();
        self.status_message = format!("Theme set to {}.", preset.label());
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

    fn open_active_workspace_files(&mut self, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(pane) = self.active_pane() else {
            return;
        };
        if pane.request.is_local_shell() {
            self.error_message = "Remote files are only available for SSH sessions.".to_string();
            cx.notify();
            return;
        }

        let pane_id = pane.id;
        let endpoint = pane.endpoint.clone();
        let request = pane.request.clone();
        let path = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.sftp.as_ref())
            .filter(|browser| browser.pane_id == pane_id)
            .map(|browser| browser.current_path.clone())
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
        eprintln!("[app] spawn_pane: pane_id={pane_id} title='{title}' endpoint={endpoint}");
        let terminal_focus = cx.focus_handle().tab_stop(true);
        let runtime = if request.kind == ConnectionKind::LocalShell {
            spawn_local_session(request.clone(), self.event_tx.clone())
        } else {
            spawn_session(
                request.clone(),
                self.known_hosts.clone(),
                self.event_tx.clone(),
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
            terminal: TerminalState::new(TerminalSize::default(), 10_000),
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
        });

        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
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
                });

                self.active_workspace_id = Some(workspace_id);
                self.show_editor_panel = false;
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
        });

        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
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
        if let Some(pane) = self.pane(pane_id) {
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
                    self.status_message = if local_shell {
                        "Local terminal ready.".to_string()
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

                    self.error_message = message;
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
                    if let Some(pane) = self.pane_mut(session_id) {
                        pane.connected = false;
                        pane.closed = true;
                        pane.status = "Closed".to_string();
                        let log_id = pane.log_id.clone();
                        self.saved
                            .update_session_log(&log_id, |e| e.mark_disconnected());
                        let _ = save_saved_state(&self.saved);
                    }

                    self.status_message = if self
                        .pane(session_id)
                        .is_some_and(|pane| pane.request.is_local_shell())
                    {
                        "Local terminal closed.".to_string()
                    } else {
                        "SSH session closed.".to_string()
                    };
                    if self.error_message.is_empty() {
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
            .resolve_font(&font(cx.theme().mono_font_family.clone()));
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
        let available_x = WORKSPACE_PADDING;
        let available_y = theme::CHROME_HEIGHT
            + theme::WORKSPACE_HEADER_HEIGHT
            + search_height
            + WORKSPACE_PADDING;
        let available_width = (viewport_width - WORKSPACE_PADDING * 2.0).max(320.0);
        let available_height = (viewport_height
            - theme::CHROME_HEIGHT
            - theme::WORKSPACE_HEADER_HEIGHT
            - search_height
            - if self.workspace_autocomplete_candidates().is_empty() {
                0.0
            } else {
                WORKSPACE_AUTOCOMPLETE_HEIGHT
            }
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
        let Some((scope_key, scope_label)) = self.pane(pane_id).map(|pane| {
            (
                pane.request.history_scope_key(),
                pane.request.history_scope_label(),
            )
        }) else {
            return;
        };
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };

        if data.starts_with(b"\x1b") {
            return;
        }

        for &byte in data {
            match byte {
                b'\r' | b'\n' => {
                    let command = pane.current_input.trim().to_string();
                    if !command.is_empty() {
                        completed_commands.push(command);
                    }
                    pane.current_input.clear();
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
        if !pane.connected || pane.closed || pane.current_input.trim().is_empty() {
            return Vec::new();
        }

        collect_autocomplete_candidates(
            pane.current_input.trim(),
            &self.saved.command_history,
            &self.saved.scoped_command_history,
            &pane.request.history_scope_key(),
            &self.saved.snippets,
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

        self.send_input_bytes(pane_id, bytes, cx)
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

        self.send_input_bytes(pane_id, bytes, cx)
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

        if let Some(pane) = self.pane_mut(pane_id) {
            pane.dragging_selection = false;
            if let Some(selection) = pane.selection {
                if selection.anchor == selection.head {
                    pane.selection = None;
                }
            }
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
                    .text_size(px(9.))
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
                    .text_size(px(9.))
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
                    .text_size(px(12.))
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
                    .text_size(px(12.5))
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
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let profile_id = profile.id.clone();
        let connect_profile_id = profile.id.clone();
        let accent = theme::host_chip_color(&profile.display_name());
        let group_label = profile.group.trim().to_string();
        let tags = profile.tags.iter().take(3).cloned().collect::<Vec<_>>();
        let vault_label = self.effective_vault_name(profile.vault_id.as_deref());
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
        let forward_count = profile.effective_local_forwards().len();
        let forward_label = (forward_count > 0).then(|| {
            if forward_count == 1 {
                "1 Forward".to_string()
            } else {
                format!("{forward_count} Forwards")
            }
        });
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
            .gap_3()
            .items_center()
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
            .hover(|style| style.bg(theme::card_hover_subtle()).shadow_sm())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.load_profile_into_inputs(&profile_id, window, cx);
            }))
            .child(
                div()
                    .size(px(44.))
                    .rounded(px(14.))
                    .bg(accent)
                    .shadow(vec![gpui::BoxShadow {
                        color: theme::avatar_glow(accent),
                        offset: point(px(0.), px(2.)),
                        blur_radius: px(6.),
                        spread_radius: px(0.),
                    }])
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(20.))
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
                                    .text_size(px(13.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(profile.display_name()),
                            )
                            .when(profile.source == ProfileSource::SshConfig, |this| {
                                this.child(self.status_badge(
                                    "SSH Config",
                                    theme::library_bg(),
                                    theme::accent(),
                                ))
                            })
                            .when(!group_label.is_empty(), |this| {
                                this.child(self.status_badge(
                                    group_label.clone(),
                                    theme::library_bg(),
                                    theme::slate(),
                                ))
                            })
                            .child(self.status_badge(
                                vault_label,
                                theme::library_bg(),
                                theme::accent(),
                            ))
                            .when_some(identity_label.clone(), |this, identity_label| {
                                this.child(self.status_badge(
                                    identity_label,
                                    theme::library_bg(),
                                    theme::success(),
                                ))
                            })
                            .when_some(jump_host_label.clone(), |this, jump_host_label| {
                                this.child(self.status_badge(
                                    jump_host_label,
                                    theme::library_bg(),
                                    theme::accent(),
                                ))
                            })
                            .when_some(forward_label.clone(), |this, forward_label| {
                                this.child(self.status_badge(
                                    forward_label,
                                    theme::library_bg(),
                                    theme::warning(),
                                ))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(protocol_icon.size(px(11.)).text_color(theme::text_muted()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(protocols),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::text_muted())
                            .child(format!("{}  •  {}", profile.endpoint(), profile.username)),
                    )
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
                Button::new(("connect-host-card", card_ix))
                    .small()
                    .custom(Self::action_button_style(theme::ActionTone::AccentSoft, cx))
                    .label("Connect")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_editor_panel = false;
                        this.load_profile_into_inputs(&connect_profile_id, window, cx);
                        this.connect_current(window, cx);
                    })),
            )
    }

    fn render_host_grid(&self, cx: &Context<Self>) -> Div {
        let groups = self.grouped_profiles(cx);

        let mut sections = Vec::new();
        let mut card_ix = 0usize;
        for (group_name, profiles) in &groups {
            let header = h_flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(group_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::text_muted())
                        .child(format!(
                            "{} {}",
                            profiles.len(),
                            if profiles.len() == 1 { "host" } else { "hosts" }
                        )),
                );

            let cards = div().w_full().flex().flex_wrap().gap_3().children(
                profiles.iter().enumerate().map(|(group_ix, profile)| {
                    self.host_card(
                        card_ix + group_ix,
                        profile,
                        self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
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
                this.child(
                    v_flex()
                        .items_center()
                        .justify_center()
                        .p_8()
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::with_alpha(theme::library_card(), 0.6))
                        .border_1()
                        .border_color(theme::with_alpha(theme::border(), 0.5))
                        .gap_2()
                        .child(
                            Icon::new(IconName::Search)
                                .size(px(28.))
                                .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_medium()
                                .text_color(theme::text_muted())
                                .child("No hosts match the current filter"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                .child("Try a different search or add a new host"),
                        ),
                )
            })
    }

    fn render_identity_picker(&self, cx: &Context<Self>) -> Div {
        let selected_path = self.current_key_path(cx);
        let identities = self.saved.identities.clone();

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.))
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
                        .text_size(px(10.))
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
                                                            .text_size(px(11.))
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
                                                    .text_size(px(10.))
                                                    .text_color(theme::text_muted())
                                                    .child(display_identity.kind.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(9.))
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
                    .text_size(px(11.))
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
                                            .text_size(px(11.))
                                            .font_medium()
                                            .text_color(if is_selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
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
                            .into_any_element()
                    })),
            )
    }

    fn render_editor_panel(&self, cx: &Context<Self>) -> Div {
        let auth_mode = self.draft_auth_mode;
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
                    .text_size(px(11.))
                    .text_color(theme::text_muted())
                    .child(
                        "Passwords are stored in the system credential store when you save or reconnect with them. Key paths are stored only for reconnects.",
                    ),
            )
            .child(self.form_field("Label", Input::new(&self.inputs.label)))
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
            .child(self.form_field("Tags", Input::new(&self.inputs.tags)))
            .child(self.form_field("Jump Host", Input::new(&self.inputs.jump_host)))
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
                            .text_size(px(12.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Port Forwarding Rules"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::text_muted())
                            .child("Save one or more local tunnels bound on 127.0.0.1 and launch them automatically with the host."),
                    )
                    .when(!self.draft_local_forwards.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_2()
                                .children(
                                    self.draft_local_forwards
                                        .iter()
                                        .cloned()
                                        .enumerate()
                                        .map(|(index, forward)| {
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
                                                    div()
                                                        .text_size(px(10.5))
                                                        .font_medium()
                                                        .text_color(theme::text_main())
                                                        .child(forward.display_name()),
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
                                                                this.remove_draft_local_forward(
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
                                self.form_field("Local Port", Input::new(&self.inputs.forward_local_port))
                                    .flex_1(),
                            )
                            .child(
                                self.form_field(
                                    "Remote Host",
                                    Input::new(&self.inputs.forward_remote_host),
                                )
                                .flex_1(),
                            )
                            .child(
                                self.form_field(
                                    "Remote Port",
                                    Input::new(&self.inputs.forward_remote_port),
                                )
                                .flex_1(),
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
                                        this.add_draft_local_forward(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme::text_muted())
                                    .child("Leave the row empty if you do not need another tunnel. Saving the host also includes any valid unsaved row."),
                            ),
                    )
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
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
                                    .text_size(px(12.))
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
                                    .text_size(px(12.))
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
                            .text_size(px(10.))
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
                                .text_size(px(11.))
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
                                    .text_size(px(10.))
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
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(label),
            )
            .child(input)
    }

    fn render_hosts_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let quick_connect = self.try_quick_connect_from_search(cx);
        let has_quick_connect = quick_connect.is_some();
        let quick_connect_password = self.current_quick_connect_password(cx);

        v_flex()
            .flex_1()
            .gap_3()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .h(px(LIBRARY_TOOLBAR_HEIGHT))
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
            .when(
                !quick_connect_password.trim().is_empty() && !has_quick_connect,
                |this| {
                    this.child(
                        div()
                            .px_4()
                            .text_size(px(10.5))
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
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Hosts"),
                    )
                    .child(self.render_host_grid(cx)),
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
                    .text_size(px(12.))
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
                    .text_size(px(12.))
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
                            .text_size(px(12.))
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
                                        .text_size(px(11.))
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
                                                                .text_size(px(12.))
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
                                                        .text_size(px(10.))
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
                            v_flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .gap_2()
                                .child(
                                    app_icon(ICON_KEY)
                                        .size(px(28.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child("No identities available"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                        .child("Use \"Add Key File\" above to add a reusable identity"),
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
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child("Saved host identities with password authentication."),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
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
                                                            .text_size(px(12.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(profile.display_name()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
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
                            v_flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::User)
                                        .size(px(28.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child("No password identities saved"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                        .child("Save a host with password auth to see it here"),
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
                        .text_size(px(18.))
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
                            .text_size(px(18.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Vaults"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
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
                    .text_size(px(12.))
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
                                                .text_size(px(12.))
                                                .font_medium()
                                                .text_color(theme::text_main())
                                                .child("Members"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.5))
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
                                            .text_size(px(10.5))
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
                                                    .text_size(px(11.))
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
                                                                    .text_size(px(11.))
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
                                                                        .text_size(px(11.5))
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
                                                                .text_size(px(10.))
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
                                                    .text_size(px(12.5))
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
                                            .text_size(px(10.5))
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
                            .text_size(px(18.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Known Hosts"),
                    )
                    .when(!entries.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
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
                    .text_size(px(12.))
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
                                                    .text_size(px(12.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(endpoint.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
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
                            v_flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .gap_2()
                                .child(
                                    app_icon(ICON_SHIELD_CHECK)
                                        .size(px(28.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child("No hosts pinned yet"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                        .child("Connect to a server to trust its key"),
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
                            .text_size(px(18.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Session History"),
                    )
                    .when(!logs.is_empty(), |this| {
                        this.child(
                            h_flex().gap_2().items_center().child(
                                div()
                                    .text_size(px(11.))
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
                                                    .text_size(px(12.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(pane.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
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
                                                            .text_size(px(12.))
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
                                                    .text_size(px(10.))
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
                                                        .text_size(px(10.))
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
                                                            .text_size(px(10.))
                                                            .text_color(theme::danger())
                                                            .child(msg.clone()),
                                                    )
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme::text_muted())
                                    .child(entry.duration_display()),
                            )
                            .into_any_element()
                    }))
                    .when(logs.is_empty() && self.panes.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::BookOpen)
                                        .size(px(28.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child("No session history yet"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                        .child("Connect to a host to see logs here"),
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
                            .text_size(px(18.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Snippets"),
                    )
                    .when(!snippets.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
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
                    .text_size(px(12.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Save repeatable commands and send them to the active terminal in one click."),
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
                                                    .text_size(px(12.))
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
                                            .child(self.status_badge(
                                                vault_label,
                                                theme::library_bg(),
                                                theme::accent(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(theme::text_muted())
                                            .child(snippet.command.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("snippet-run", index))
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Success,
                                                _cx,
                                            ))
                                            .label("Run")
                                            .on_click(_cx.listener(move |this, _, _, cx| {
                                                this.run_snippet_command(&run_command, cx);
                                            })),
                                    ),
                            )
                            .into_any_element()
                    }))
                    .when(snippets.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(theme::border())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::BookOpen)
                                        .size(px(28.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.4)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child("No snippets yet"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                        .child("Save a reusable command above to build a snippets library"),
                                ),
                        )
                    }),
            )
    }

    fn render_settings_view(&self, cx: &Context<Self>) -> Div {
        let theme_preset = self.saved.settings.theme_preset;
        let terminal_font_size = self.saved.settings.terminal_font_size;

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
                            .text_size(px(18.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("Local desktop preferences"),
                    ),
            )
            .child(
                v_flex()
                    .gap_4()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child("Appearance Theme"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_muted())
                                    .child("Switch the global UI palette across the whole desktop app."),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .children([ThemePreset::Ocean, ThemePreset::Daylight].into_iter().enumerate().map(
                                        |(index, preset)| {
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
                                                        .text_size(px(11.))
                                                        .font_medium()
                                                        .text_color(if selected {
                                                            theme::text_main()
                                                        } else {
                                                            theme::text_muted()
                                                        })
                                                        .child(preset.label()),
                                                )
                                                .into_any_element()
                                        },
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child("Terminal Font Size"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_muted())
                                    .child("Apply a larger or tighter monospace size across every terminal pane."),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
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
                                                        .text_size(px(11.))
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
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child("Default Local Terminal"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_muted())
                                    .child("Choose which shell binary and working directory new local terminals use."),
                            )
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
                                            .text_size(px(10.))
                                            .text_color(theme::text_muted())
                                            .child("Args stay empty for now; this sets the default executable and startup directory."),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .children([
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
                                    .gap_2()
                                    .p_3()
                                    .rounded(px(theme::CARD_RADIUS))
                                    .bg(bg)
                                    .border_1()
                                    .border_color(theme::with_alpha(fg, 0.18))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_semibold()
                                            .text_color(fg)
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(theme::with_alpha(fg, 0.78))
                                            .child(match label {
                                                "Library" => "Forms, host cards, and management views",
                                                "Chrome" => "Tabs, status bar, and workspace header",
                                                _ => "Terminal panels and focused work sessions",
                                            }),
                                    )
                                    .into_any_element()
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Portable Data Bundle"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(relative(1.5))
                            .text_color(theme::text_muted())
                            .child("Export or import hosts, vaults, identities, snippets, and known-host trust records as a local JSON bundle. Passwords and system credential-store secrets are intentionally excluded, so this is safe for portability but not a full account sync."),
                    )
                    .child(
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
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child("Encrypted Backup"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(relative(1.5))
                            .text_color(theme::text_muted())
                            .child("Wrap the same portable bundle in passphrase-based encryption for device backups, handoff, or manual sync. The file stays locally managed; no cloud account is involved yet."),
                    )
                    .child(
                        v_flex()
                            .gap_2()
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
                                            .text_size(px(10.))
                                            .text_color(theme::text_muted())
                                            .child("Use a strong passphrase you can recover later. The file cannot be opened without it."),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme::border())
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
                                            .text_size(px(10.))
                                            .text_color(theme::text_muted())
                                            .child("Import merges vaults, hosts, snippets, and trust records without exposing the plaintext bundle on disk."),
                                    ),
                            ),
                    ),
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
        h_flex()
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
            .text_size(px(10.))
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

    fn render_workspace_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let Some(pane) = self.active_pane() else {
            return v_flex();
        };
        let files_mode = workspace.view_mode == WorkspaceViewMode::Files;
        let can_browse_files = !pane.request.is_local_shell();
        let selected_remote_entry = self.selected_workspace_sftp_entry(workspace.id);
        let _focused = pane.terminal_focus.is_focused(window);

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
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_semibold()
                            .text_color(theme::text_on_dark())
                            .child(workspace.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted_dark())
                            .child(pane.endpoint.clone()),
                    )
                    .when_some(pane.request.jump_host.as_ref(), |this, jump_host| {
                        this.child(self.status_badge(
                            format!("Via {}", jump_host.title),
                            theme::terminal_panel(),
                            theme::accent(),
                        ))
                    })
                    .when(!pane.request.local_forwards.is_empty(), |this| {
                        let forward_label = if pane.request.local_forwards.len() == 1 {
                            let forward = &pane.request.local_forwards[0];
                            format!("Local {}", forward.local_port)
                        } else {
                            format!("{} Forwards", pane.request.local_forwards.len())
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
                        .text_size(px(11.))
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
        let candidates = self.workspace_autocomplete_candidates();
        if current_input.is_empty() || candidates.is_empty() {
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
                        .text_size(px(10.5))
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
                        .text_size(px(10.))
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
            return v_flex().flex_1().items_center().justify_center().child(
                v_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::FolderOpen)
                            .size(px(28.))
                            .text_color(theme::with_alpha(theme::text_muted_dark(), 0.45)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(theme::text_on_dark())
                            .child("Open Files to browse this host over SFTP"),
                    ),
            );
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
                                            .text_size(px(11.))
                                            .font_medium()
                                            .text_color(theme::text_muted_dark())
                                            .child("Remote Path"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.))
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
                                .text_size(px(10.5))
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
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted_dark())
                                        .child("Loading remote directory..."),
                                ),
                        )
                    })
                    .when(browser.entries.is_empty() && !browser.loading, |this| {
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
                                    Icon::new(IconName::Folder).size(px(28.)).text_color(
                                        theme::with_alpha(theme::text_muted_dark(), 0.4),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_on_dark())
                                        .child("This directory is empty"),
                                ),
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
                                                    .text_size(px(12.5))
                                                    .font_medium()
                                                    .text_color(theme::text_on_dark())
                                                    .child(entry.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
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
                                            .text_size(px(10.5))
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

    fn render_terminal_cell_group(
        &self,
        text: String,
        style: TerminalStyle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut node = div()
            .whitespace_nowrap()
            .font_family(cx.theme().mono_font_family.clone())
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
        let status_color = if pane.connected {
            theme::success()
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
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_medium()
                                    .text_color(theme::text_on_dark())
                                    .child(pane.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_muted_dark())
                                    .child(pane.endpoint.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(pane.closed, |this| {
                                this.child(
                                    Button::new(("reconnect-pane", pane.id))
                                        .ghost()
                                        .xsmall()
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
                .gap_3()
                .child(
                    Icon::new(IconName::SquareTerminal)
                        .size(px(36.))
                        .text_color(theme::with_alpha(theme::text_muted_dark(), 0.3)),
                )
                .child(
                    div()
                        .text_size(px(15.))
                        .font_medium()
                        .text_color(theme::text_on_dark())
                        .child("Open a host to start a workspace"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted_dark())
                        .child("Select a host from the library or use quick connect"),
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
            .when_some(self.render_workspace_search(window, cx), |this, search| {
                this.child(search)
            })
            .when_some(
                self.render_workspace_autocomplete(window, cx),
                |this, autocomplete| this.child(autocomplete),
            )
            .child(content)
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
                                            .text_size(px(11.))
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
                                                    .text_size(px(10.5))
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
                                                .text_size(px(10.))
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
                                            .text_size(px(15.))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child("Command Palette"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
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
                                                                .text_size(px(12.5))
                                                                .font_semibold()
                                                                .text_color(theme::text_main())
                                                                .child(candidate.title.clone()),
                                                        )
                                                        .child(self.status_badge(
                                                            candidate.source.label(),
                                                            theme::library_bg(),
                                                            match candidate.source {
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
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
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
                                                .text_size(px(13.))
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
                                                .text_size(px(11.))
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
                            .size(px(30.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .hover(|style| style.bg(theme::hover()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor_dialog(window, cx);
                            }))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(14.))
                                    .text_color(theme::text_muted()),
                            ),
                    )
                    .child(
                        v_flex().px_5().pt_5().pb_2().child(
                            div()
                                .text_size(px(16.))
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
    query: &str,
    command_history: &[String],
    scoped_command_history: &[SavedCommandHistoryEntry],
    scope_key: &str,
    snippets: &[SavedSnippet],
) -> Vec<AutocompleteCandidate> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    #[derive(Clone)]
    struct ScoredAutocompleteCandidate {
        candidate: AutocompleteCandidate,
        match_kind: AutocompleteMatchKind,
        ordinal: usize,
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
                ordinal: ordinal + scoped_command_history.len() + command_history.len(),
            });
        }
    }

    for (ordinal, command) in builtin_commands().iter().enumerate() {
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: (*command).to_string(),
                    source: AutocompleteSource::Builtin,
                    scope_label: None,
                },
                match_kind,
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
            let mut detail = format!("Snippet • {}", command);
            if !snippet.group.trim().is_empty() {
                detail = format!("Snippet • {} • {}", snippet.group.trim(), command);
            }
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: command.to_string(),
                    title,
                    detail,
                    source: AutocompleteSource::Snippet,
                },
                match_kind,
                ordinal,
                source_priority: 2,
            });
        }
    }

    for (ordinal, command) in builtin_commands().iter().enumerate() {
        let Some(match_kind) = palette_match_kind(&query, &[*command]) else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    command: (*command).to_string(),
                    title: (*command).to_string(),
                    detail: "Built-in suggestion".to_string(),
                    source: AutocompleteSource::Builtin,
                },
                match_kind,
                ordinal,
                source_priority: 3,
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

fn builtin_commands() -> &'static [&'static str] {
    &[
        "ls -la",
        "pwd",
        "cd /var/www",
        "git status",
        "git pull",
        "docker ps",
        "docker logs -f",
        "systemctl status",
        "journalctl -u",
        "tail -f /var/log/syslog",
    ]
}
