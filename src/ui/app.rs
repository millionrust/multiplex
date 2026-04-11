use std::cmp::Ordering;
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
use gpui_component::{ActiveTheme, Icon, Sizable, StyledExt as _, h_flex, v_flex};
use rfd::FileDialog;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::credentials;
use crate::models::{
    AuthConfig, AuthMode, ConnectRequest, DraftProfile, HostProfile, JumpHostConnection,
    ProfileSource, QuickConnect, SavedIdentity, SavedSnippet, SavedState, SavedWorkspace,
    SessionLogEntry, SessionLogStatus, SplitAxis,
};
use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent, spawn_session};
use crate::storage::{
    KnownHostStore, inspect_identity_file, load_local_ssh_identities, save_saved_state,
};
use crate::terminal::{
    DEFAULT_SCROLLBACK, TerminalCell, TerminalRow, TerminalSize, TerminalState, TerminalStyle,
};
use crate::ui::theme;

const TERMINAL_FONT_SIZE: f32 = 13.5;
const TERMINAL_LINE_HEIGHT: f32 = 1.3;
const LIBRARY_TOOLBAR_HEIGHT: f32 = 56.0;
const WORKSPACE_SEARCH_ROW_HEIGHT: f32 = 52.0;
const WORKSPACE_PADDING: f32 = 18.0;
const PANE_GAP: f32 = 12.0;
const PANE_HEADER_HEIGHT: f32 = 38.0;
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
    Keychain,
    Snippets,
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
            Self::Keychain => "Keys",
            Self::Snippets => "Snippets",
            Self::KnownHosts => "Known Hosts",
            Self::Logs => "Logs",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Hosts => IconName::SquareTerminal.into(),
            Self::Keychain => app_icon(ICON_KEY),
            Self::Snippets => IconName::BookOpen.into(),
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
}

struct SnippetInputs {
    label: Entity<InputState>,
    group: Entity<InputState>,
    command: Entity<InputState>,
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

impl ShellInputs {
    fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            host_search: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Find a host or ssh user@hostname...")
            }),
            quick_connect_password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Password")
            }),
            terminal_search: cx
                .new(|cx| InputState::new(window, cx).placeholder("Search terminal output")),
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
}

struct WorkspaceTab {
    id: u64,
    title: String,
    pane_ids: Vec<u64>,
    active_pane_id: u64,
    unread_events: u32,
    split_axis: SplitAxis,
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
    draft_auth_mode: AuthMode,
    nav_section: NavSection,
    show_editor_panel: bool,
    event_tx: Sender<SshEvent>,
    event_rx: Receiver<SshEvent>,
    panes: Vec<SessionPane>,
    workspaces: Vec<WorkspaceTab>,
    active_workspace_id: Option<u64>,
    selected_profile_id: Option<String>,
    selected_snippet_id: Option<String>,
    next_session_id: u64,
    next_workspace_id: u64,
    status_message: String,
    error_message: String,
    draft_identity_id: Option<String>,
    known_hosts: Arc<KnownHostStore>,
    keychain_tab: KeychainTab,
    _window_bounds_subscription: Option<Subscription>,
}

impl TermiRustApp {
    pub fn new(mut saved: SavedState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs = DraftInputs::new(window, cx);
        let shell_inputs = ShellInputs::new(window, cx);
        let snippet_inputs = SnippetInputs::new(window, cx);
        let (event_tx, event_rx) = mpsc::channel();
        let known_hosts =
            Arc::new(KnownHostStore::load().expect("unable to initialize known host storage"));
        saved.merge_imported_identities(load_local_ssh_identities().unwrap_or_default());

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
            draft_auth_mode,
            nav_section: NavSection::Hosts,
            show_editor_panel: false,
            event_tx,
            event_rx,
            panes: Vec::new(),
            workspaces: Vec::new(),
            active_workspace_id: None,
            selected_snippet_id: None,
            next_session_id: 1,
            next_workspace_id: 1,
            status_message: initial_status,
            error_message: String::new(),
            draft_identity_id: None,
            known_hosts,
            keychain_tab: KeychainTab::Keys,
            _window_bounds_subscription: None,
        };

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
            group: self.inputs.group.read(cx).value().to_string(),
            host: self.inputs.host.read(cx).value().to_string(),
            port: self.inputs.port.read(cx).value().to_string(),
            username: self.inputs.username.read(cx).value().to_string(),
            password: self.inputs.password.read(cx).value().to_string(),
            key_path,
            identity_id,
            jump_host_id,
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

    fn preferred_identity(&self) -> Option<&SavedIdentity> {
        self.saved.identities.first()
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

        Ok(JumpHostConnection {
            title: profile.display_name(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth,
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
        Self::set_input_value(
            &self.inputs.forward_local_port,
            draft.forward_local_port,
            window,
            cx,
        );
        Self::set_input_value(
            &self.inputs.forward_remote_host,
            draft.forward_remote_host,
            window,
            cx,
        );
        Self::set_input_value(
            &self.inputs.forward_remote_port,
            draft.forward_remote_port,
            window,
            cx,
        );
        Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
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
        self.draft_identity_id = None;
        self.selected_profile_id = None;
        self.saved.selected_profile_id = None;
        self.draft_auth_mode = AuthMode::Password;
        self.show_editor_panel = true;
        self.status_message = "Draft cleared. Define a host to save or connect.".into();
        self.error_message.clear();
        cx.notify();
    }

    fn current_snippet_draft(&self, cx: &App) -> SavedSnippet {
        SavedSnippet {
            id: self
                .selected_snippet_id
                .clone()
                .unwrap_or_else(SavedSnippet::snippet_id),
            label: self.snippet_inputs.label.read(cx).value().to_string(),
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

    fn run_snippet_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let Some(pane_id) = self.active_pane().map(|pane| pane.id) else {
            self.error_message = "Open a terminal session to run a snippet.".to_string();
            cx.notify();
            return;
        };

        let mut bytes = command.as_bytes().to_vec();
        if !command.ends_with('\n') {
            bytes.push(b'\n');
        }

        if self.send_input_bytes(pane_id, bytes, cx) {
            self.status_message = "Snippet sent to the active session.".to_string();
            self.error_message.clear();
            cx.notify();
        }
    }

    fn pick_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = FileDialog::new().pick_file() {
            match inspect_identity_file(&path) {
                Ok(Some(imported)) => {
                    let identity = imported.into_saved();
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
                let haystacks = [
                    profile.display_name(),
                    profile.group.clone(),
                    profile.host.clone(),
                    profile.username.clone(),
                    profile.endpoint(),
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

    fn set_terminal_search_input(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(&self.shell_inputs.terminal_search, value, window, cx);
    }

    fn activate_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_workspace_id = None;
        self.nav_section = NavSection::Hosts;
        self.set_terminal_search_input("", window, cx);
        self.status_message = "Host library ready.".to_string();
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
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
        let endpoint = request.address();
        let title = request.title.clone();
        eprintln!("[app] spawn_pane: pane_id={pane_id} title='{title}' endpoint={endpoint}");
        let terminal_focus = cx.focus_handle().tab_stop(true);
        let runtime = spawn_session(
            request.clone(),
            self.known_hosts.clone(),
            self.event_tx.clone(),
        );
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
            terminal: TerminalState::new(TerminalSize::default(), DEFAULT_SCROLLBACK),
            terminal_focus,
            last_size: None,
            runtime,
            connected: false,
            closed: false,
            status: "Connecting".to_string(),
            selection: None,
            dragging_selection: false,
            log_id,
        });

        let _ = window;
        pane_id
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
                    match &request.auth {
                        AuthConfig::Password { .. } => "password",
                        AuthConfig::PasswordRef { .. } => "stored-password",
                        AuthConfig::PrivateKey { key_path, .. } => key_path.as_str(),
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
                    search_visible: false,
                    search_query: String::new(),
                    search_results: Vec::new(),
                    active_search_index: None,
                });

                self.active_workspace_id = Some(workspace_id);
                self.show_editor_panel = false;
                self.status_message = format!("Connecting to {}...", request.address());
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
            }
        }

        if let Some(old_pane) = self.pane(pane_id) {
            let _ = old_pane.runtime.command_tx.send(SessionCommand::Disconnect);
        }
        self.panes.retain(|p| p.id != pane_id);

        self.status_message = format!("Reconnecting to {}...", request.address());
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
            workspace.title = request.title.clone();
        }

        self.status_message = format!("Launching split pane for {}...", request.address());
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

                    self.status_message = if trusted_new_host {
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

                    self.status_message = "SSH session closed.".to_string();
                    if self.error_message.is_empty() {
                        self.error_message = message;
                    }
                }
            }
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
        let font_size = px(TERMINAL_FONT_SIZE);
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
        let line_height = (TERMINAL_FONT_SIZE * TERMINAL_LINE_HEIGHT).max(1.0);
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
            .send(SessionCommand::Input(data))
            .is_err()
        {
            self.error_message = "Unable to send input to the SSH runtime.".to_string();
            cx.notify();
            return false;
        }

        self.error_message.clear();
        if notify {
            cx.notify();
        }
        true
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

        let line_height = px(TERMINAL_FONT_SIZE * TERMINAL_LINE_HEIGHT);
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
                        NavSection::Keychain,
                        NavSection::Snippets,
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
        let forward_label = profile
            .local_forward
            .as_ref()
            .map(|forward| format!("Forward {}", forward.local_port));
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
                    ),
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
            .child(self.form_field("Group", Input::new(&self.inputs.group)))
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
                            .child("Local Forward"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::text_muted())
                            .child("Optional local tunnel bound on 127.0.0.1."),
                    )
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
                            ),
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
                                                        }),
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
                                            }),
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

    fn render_library_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.nav_section {
            NavSection::Hosts => self.render_hosts_view(window, cx).into_any_element(),
            NavSection::Keychain => self.render_keychain_view(cx).into_any_element(),
            NavSection::Snippets => self.render_snippets_view(cx).into_any_element(),
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
                    .when_some(pane.request.local_forward.as_ref(), |this, forward| {
                        this.child(self.status_badge(
                            format!("Local {}", forward.local_port),
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_workspace_search(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-split-right")
                            .ghost()
                            .small()
                            .icon(IconName::PanelRight)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Horizontal, window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-split-down")
                            .ghost()
                            .small()
                            .icon(IconName::PanelBottom)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Vertical, window, cx);
                            })),
                    )
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

    fn render_terminal_cell_group(
        &self,
        text: String,
        style: TerminalStyle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut node = div()
            .whitespace_nowrap()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(TERMINAL_FONT_SIZE))
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
        v_flex()
            .flex_1()
            .bg(theme::terminal_bg())
            .child(self.render_workspace_toolbar(window, cx))
            .when_some(self.render_workspace_search(window, cx), |this, search| {
                this.child(search)
            })
            .child(self.render_workspace_body(window, cx))
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
                                                    "{}+F Search  {}+W Close",
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
    }
}

impl TermiRustApp {
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
        NavSection::Keychain => 1,
        NavSection::Snippets => 2,
        NavSection::KnownHosts => 3,
        NavSection::Logs => 4,
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
