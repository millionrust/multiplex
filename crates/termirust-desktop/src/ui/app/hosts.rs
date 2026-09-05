//! Hosts library page: host tile/list cards, top toolbar (search + NEW HOST
//! split menu + Grid/Tag/Sort/Avatar dropdowns), the absolute overlay layer
//! and the page wrapper. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Div, ElementId, Focusable as _,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Disableable, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::connection_diagnostics::MAX_DIAGNOSTIC_BATCH;
use crate::models::{AuthMode, HostProfile, ProfileSource};
use crate::ui::app::{
    ConnectionDiagnosticStatus, EditorMenu, HostsSort, HostsViewMode, ICON_CALENDAR, ICON_GRID,
    ICON_KEY, ICON_PENCIL, ICON_TAG, ICON_VAULT, TermiRustApp, ToolbarMenu, app_icon,
};
use crate::ui::localization;
use crate::ui::theme;
use crate::ui::util::format_relative_time;
use termirust_ui_contract::{
    HostAuthChoice, HostConnectionAccessibilityCommand, HostConnectionAction,
    HostConnectionControl, HostConnectionControlRole, HostConnectionRow, HostConnectionRowId,
    HostConnectionScreen, HostConnectionSemanticSnapshot, HostConnectionSurfaceState, MessageId,
    SemanticActionValue, stable_host_row_value,
};

const PROVIDER_AWS: &str = "AWS";
const PROVIDER_DIGITAL_OCEAN: &str = "DigitalOcean";
const PROVIDER_AZURE: &str = "Azure";

impl TermiRustApp {
    pub(super) fn host_connection_semantic_snapshot(
        &self,
        cx: &App,
    ) -> Option<HostConnectionSemanticSnapshot> {
        if let Some(workspace) = self.active_workspace() {
            if let Some(failure) = workspace.connect_failure.as_ref() {
                return Some(self.connect_failure_semantic_snapshot(failure));
            }
            if let Some(profile) = workspace.pending_connect.as_ref() {
                return Some(match workspace.pending_connect_mode {
                    crate::ui::app::ConnectDialogMode::Username => {
                        self.connect_username_semantic_snapshot(profile, cx)
                    }
                    crate::ui::app::ConnectDialogMode::ChooseProtocol => {
                        self.connect_protocol_semantic_snapshot(profile, cx)
                    }
                });
            }
        }
        if self.nav_section != crate::ui::app::NavSection::Hosts {
            return None;
        }
        if self.show_editor_panel {
            Some(self.host_editor_semantic_snapshot(cx))
        } else {
            Some(self.hosts_library_semantic_snapshot(cx))
        }
    }

    fn hosts_library_semantic_snapshot(&self, cx: &App) -> HostConnectionSemanticSnapshot {
        const MAX_VISIBLE_HOST_ROWS: usize = 512;
        let profiles = self
            .filtered_profiles(cx)
            .into_iter()
            .take(MAX_VISIBLE_HOST_ROWS)
            .collect::<Vec<_>>();
        let profile_count = profiles.len();
        let mut rows = Vec::with_capacity(profile_count + self.connection_diagnostics.len());
        let mut controls = vec![
            host_control(
                HostConnectionAction::SetSearch,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostsSearchField,
                Some(self.host_search_query(cx)),
                false,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::AddHost,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsAddAction,
                None,
                false,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::QuickConnect,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsQuickConnectAction,
                None,
                false,
                self.try_quick_connect_from_search(cx).is_none(),
                false,
            ),
            host_control(
                HostConnectionAction::SetQuickConnectPassword,
                None,
                HostConnectionControlRole::PasswordField,
                MessageId::HostsQuickConnectPasswordField,
                (!self.current_quick_connect_password(cx).is_empty()).then(|| "set".to_string()),
                false,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::SelectVisible,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsSelectVisible,
                None,
                false,
                profiles.is_empty(),
                false,
            ),
            host_control(
                HostConnectionAction::ClearSelection,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsClearSelection,
                None,
                false,
                self.selected_host_ids.is_empty(),
                false,
            ),
            host_control(
                HostConnectionAction::DiagnoseSelected,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsDiagnoseAction,
                None,
                false,
                self.selected_host_ids.is_empty(),
                false,
            ),
            host_control(
                HostConnectionAction::SetBulkGroup,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostsBulkGroupField,
                Some(self.shell_inputs.bulk_group.read(cx).value().to_string()),
                false,
                self.selected_host_ids.is_empty(),
                false,
            ),
            host_control(
                HostConnectionAction::ApplyBulkGroup,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostsBulkGroupApply,
                None,
                false,
                self.selected_host_ids.is_empty(),
                false,
            ),
        ];
        if self
            .connection_diagnostics
            .iter()
            .any(|row| !row.status.is_active())
        {
            controls.push(host_control(
                HostConnectionAction::ClearFinishedDiagnostics,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostDiagnosticsClear,
                None,
                false,
                false,
                false,
            ));
        }

        for (index, profile) in profiles.iter().enumerate() {
            let row_id = host_row_id(&profile.id);
            rows.push(HostConnectionRow {
                id: row_id,
                parent: None,
                name: profile.display_name(),
                status: auth_mode_message(profile.auth_mode),
                detail: Some(format!("{}@{}", profile.username, profile.endpoint())),
                selected: self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                disabled: false,
                checked: Some(self.selected_host_ids.contains(&profile.id)),
                invalid: profile.host.trim().is_empty() || profile.username.trim().is_empty(),
                stale: false,
                position: index + 1,
                set_size: profile_count.max(1),
            });
            for (action, name, role, selected) in [
                (
                    HostConnectionAction::ConnectHost(row_id),
                    MessageId::CommonConnect,
                    HostConnectionControlRole::Button,
                    false,
                ),
                (
                    HostConnectionAction::EditHost(row_id),
                    MessageId::HostEditAction,
                    HostConnectionControlRole::Button,
                    false,
                ),
                (
                    HostConnectionAction::ToggleFavorite(row_id),
                    if profile.favorite {
                        MessageId::HostUnstarAction
                    } else {
                        MessageId::HostStarAction
                    },
                    HostConnectionControlRole::Checkbox,
                    profile.favorite,
                ),
                (
                    HostConnectionAction::ToggleBatchSelection(row_id),
                    if self.selected_host_ids.contains(&profile.id) {
                        MessageId::HostDeselectAction
                    } else {
                        MessageId::HostSelectAction
                    },
                    HostConnectionControlRole::Checkbox,
                    self.selected_host_ids.contains(&profile.id),
                ),
            ] {
                controls.push(host_control(
                    action,
                    Some(row_id),
                    role,
                    name,
                    None,
                    selected,
                    false,
                    false,
                ));
            }
        }

        for diagnostic in self
            .connection_diagnostics
            .iter()
            .take(MAX_DIAGNOSTIC_BATCH)
        {
            let Some(parent) = profiles
                .iter()
                .find(|profile| profile.id == diagnostic.profile_id)
                .map(|profile| host_row_id(&profile.id))
            else {
                continue;
            };
            let row_id = HostConnectionRowId::diagnostic(parent.value, diagnostic.operation_id);
            rows.push(HostConnectionRow {
                id: row_id,
                parent: Some(parent),
                name: diagnostic.title.clone(),
                status: diagnostic_status_message(diagnostic.status),
                detail: Some(format!(
                    "{} | {} | {} | {}",
                    diagnostic.address,
                    diagnostic.route,
                    localization::message_id(diagnostic_stage_message(diagnostic.stage))
                        .unwrap_or_default(),
                    diagnostic.message
                )),
                selected: false,
                disabled: false,
                checked: None,
                invalid: diagnostic.status == ConnectionDiagnosticStatus::Failed,
                stale: false,
                position: 1,
                set_size: 1,
            });
            controls.push(host_control(
                if diagnostic.status.is_active() {
                    HostConnectionAction::CancelDiagnostic(row_id)
                } else {
                    HostConnectionAction::RetryDiagnostic(row_id)
                },
                Some(row_id),
                HostConnectionControlRole::Button,
                if diagnostic.status.is_active() {
                    MessageId::HostDiagnosticCancel
                } else {
                    MessageId::HostDiagnosticRetry
                },
                None,
                false,
                false,
                false,
            ));
        }

        let state = host_library_state(
            self.saved.profiles.is_empty(),
            profiles.is_empty(),
            &self.connection_diagnostics,
        );
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::Hosts,
            state,
            rows,
            controls,
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    fn host_editor_semantic_snapshot(&self, cx: &App) -> HostConnectionSemanticSnapshot {
        let value = |input: &gpui::Entity<gpui_component::input::InputState>| {
            Some(input.read(cx).value().to_string())
        };
        let secret_set = |input: &gpui::Entity<gpui_component::input::InputState>| {
            (!input.read(cx).value().is_empty()).then(|| "set".to_string())
        };
        let host_invalid = self.inputs.host.read(cx).value().trim().is_empty();
        let username_invalid = self.inputs.username.read(cx).value().trim().is_empty();
        let port_invalid = self
            .inputs
            .port
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .is_err();
        let mut controls = vec![
            host_control(
                HostConnectionAction::SetHostLabel,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostLabelField,
                value(&self.inputs.label),
                false,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::SetHostAddress,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostAddressField,
                value(&self.inputs.host),
                false,
                false,
                host_invalid,
            ),
            host_control(
                HostConnectionAction::SetHostPort,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostPortField,
                value(&self.inputs.port),
                false,
                false,
                port_invalid,
            ),
            host_control(
                HostConnectionAction::SetHostUsername,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostUsernameField,
                value(&self.inputs.username),
                false,
                false,
                username_invalid,
            ),
        ];
        for (choice, name, selected) in [
            (
                HostAuthChoice::Password,
                MessageId::HostAuthPassword,
                self.draft_auth_mode == AuthMode::Password,
            ),
            (
                HostAuthChoice::PrivateKey,
                MessageId::HostAuthPrivateKey,
                self.draft_auth_mode == AuthMode::PrivateKey,
            ),
            (
                HostAuthChoice::LocalAgent,
                MessageId::HostAuthLocalAgent,
                self.draft_auth_mode == AuthMode::LocalAgent,
            ),
        ] {
            controls.push(host_control(
                HostConnectionAction::SelectAuth(choice),
                None,
                HostConnectionControlRole::RadioButton,
                name,
                None,
                selected,
                false,
                false,
            ));
        }
        match self.draft_auth_mode {
            AuthMode::Password => controls.push(host_control(
                HostConnectionAction::SetHostPassword,
                None,
                HostConnectionControlRole::PasswordField,
                MessageId::HostPasswordField,
                secret_set(&self.inputs.password),
                false,
                false,
                false,
            )),
            AuthMode::PrivateKey => {
                controls.push(host_control(
                    HostConnectionAction::SetHostKeyPath,
                    None,
                    HostConnectionControlRole::TextField,
                    MessageId::HostKeyPathField,
                    value(&self.inputs.key_path),
                    false,
                    false,
                    self.inputs.key_path.read(cx).value().trim().is_empty(),
                ));
                controls.push(host_control(
                    HostConnectionAction::SetHostKeyPassphrase,
                    None,
                    HostConnectionControlRole::PasswordField,
                    MessageId::HostKeyPassphraseField,
                    secret_set(&self.inputs.key_passphrase),
                    false,
                    false,
                    false,
                ));
            }
            AuthMode::LocalAgent => controls.push(host_control(
                HostConnectionAction::SetHostAgentSocket,
                None,
                HostConnectionControlRole::TextField,
                MessageId::HostAgentSocketField,
                value(&self.inputs.identity_agent),
                false,
                false,
                false,
            )),
        }
        controls.extend([
            host_control(
                HostConnectionAction::SaveHost,
                None,
                HostConnectionControlRole::Button,
                MessageId::CommonSave,
                None,
                false,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::CloseHostEditor,
                None,
                HostConnectionControlRole::Button,
                MessageId::CommonClose,
                None,
                false,
                false,
                false,
            ),
        ]);
        if let Some(profile_id) = self.selected_profile_id.as_deref() {
            let imported = self
                .saved
                .profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .is_some_and(|profile| profile.source == ProfileSource::SshConfig);
            controls.push(host_control(
                HostConnectionAction::DeleteHost,
                None,
                HostConnectionControlRole::Button,
                MessageId::HostDeleteAction,
                None,
                false,
                imported,
                false,
            ));
        }
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::HostEditor,
            state: classify_host_connection_error(&self.error_message)
                .unwrap_or(HostConnectionSurfaceState::Editing),
            rows: Vec::new(),
            controls,
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    fn connect_username_semantic_snapshot(
        &self,
        _profile: &HostProfile,
        cx: &App,
    ) -> HostConnectionSemanticSnapshot {
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::ConnectUsername,
            state: HostConnectionSurfaceState::Ready,
            rows: Vec::new(),
            controls: vec![
                host_control(
                    HostConnectionAction::SetConnectUsername,
                    None,
                    HostConnectionControlRole::TextField,
                    MessageId::HostUsernameField,
                    Some(
                        self.shell_inputs
                            .connect_username
                            .read(cx)
                            .value()
                            .to_string(),
                    ),
                    false,
                    false,
                    self.shell_inputs
                        .connect_username
                        .read(cx)
                        .value()
                        .trim()
                        .is_empty(),
                ),
                host_control(
                    HostConnectionAction::ContinueAndSave,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::ConnectContinueSave,
                    None,
                    false,
                    false,
                    false,
                ),
                host_control(
                    HostConnectionAction::CloseConnectionDialog,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::CommonClose,
                    None,
                    false,
                    false,
                    false,
                ),
            ],
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    fn connect_protocol_semantic_snapshot(
        &self,
        profile: &HostProfile,
        cx: &App,
    ) -> HostConnectionSemanticSnapshot {
        let protocol = HostConnectionRowId::protocol(1);
        let port_invalid = self
            .shell_inputs
            .protocol_ssh_port
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .is_err();
        let mut controls = vec![
            host_control(
                HostConnectionAction::SelectProtocol(protocol),
                Some(protocol),
                HostConnectionControlRole::RadioButton,
                MessageId::ConnectProtocolSsh,
                None,
                true,
                false,
                false,
            ),
            host_control(
                HostConnectionAction::SetProtocolPort,
                Some(protocol),
                HostConnectionControlRole::TextField,
                MessageId::ConnectProtocolPort,
                Some(
                    self.shell_inputs
                        .protocol_ssh_port
                        .read(cx)
                        .value()
                        .to_string(),
                ),
                false,
                false,
                port_invalid,
            ),
            host_control(
                HostConnectionAction::ContinueProtocol,
                None,
                HostConnectionControlRole::Button,
                MessageId::ConnectContinueAction,
                None,
                false,
                port_invalid,
                false,
            ),
            host_control(
                HostConnectionAction::CloseConnectionDialog,
                None,
                HostConnectionControlRole::Button,
                MessageId::CommonClose,
                None,
                false,
                false,
                false,
            ),
        ];
        if profile.auth_mode == AuthMode::LocalAgent {
            controls.push(host_control(
                HostConnectionAction::ForwardAgentOnce,
                None,
                HostConnectionControlRole::Button,
                MessageId::ConnectForwardAgentAction,
                None,
                false,
                port_invalid,
                false,
            ));
        }
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::ChooseProtocol,
            state: HostConnectionSurfaceState::Ready,
            rows: vec![HostConnectionRow {
                id: protocol,
                parent: None,
                name: host_message(MessageId::ConnectProtocolSsh),
                status: MessageId::ConnectProtocolSsh,
                detail: Some(format!("{}:{}", profile.host, profile.port)),
                selected: true,
                disabled: false,
                checked: Some(true),
                invalid: port_invalid,
                stale: false,
                position: 1,
                set_size: 1,
            }],
            controls,
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    fn connect_failure_semantic_snapshot(
        &self,
        failure: &crate::ui::app::ConnectFailure,
    ) -> HostConnectionSemanticSnapshot {
        HostConnectionSemanticSnapshot {
            screen: HostConnectionScreen::ConnectionFailure,
            state: classify_connect_failure(failure),
            rows: Vec::new(),
            controls: vec![
                host_control(
                    HostConnectionAction::CopyFailureLog,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::ConnectCopyLogAction,
                    None,
                    false,
                    false,
                    false,
                ),
                host_control(
                    HostConnectionAction::EditFailedHost,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::ConnectEditHostAction,
                    None,
                    false,
                    false,
                    false,
                ),
                host_control(
                    HostConnectionAction::RetryConnection,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::ConnectRetryAction,
                    None,
                    false,
                    false,
                    false,
                ),
                host_control(
                    HostConnectionAction::CloseConnectionDialog,
                    None,
                    HostConnectionControlRole::Button,
                    MessageId::CommonClose,
                    None,
                    false,
                    false,
                    false,
                ),
            ],
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    pub(super) fn handle_host_connection_accessibility_command(
        &mut self,
        command: HostConnectionAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.host_connection_semantic_snapshot(cx) else {
            return;
        };
        match command {
            HostConnectionAccessibilityCommand::FocusRow(_) => {
                self.project_list_focus.focus(window);
            }
            HostConnectionAccessibilityCommand::ActivateRow(row_id) => {
                if snapshot
                    .rows
                    .iter()
                    .any(|row| row.id == row_id && !row.disabled)
                    && let Some(profile_id) = self.profile_id_for_host_row(row_id)
                {
                    self.select_profile_from_library(&profile_id, window, cx);
                }
            }
            HostConnectionAccessibilityCommand::FocusControl(action) => {
                if snapshot
                    .controls
                    .iter()
                    .any(|control| control.action == action)
                {
                    self.focus_host_connection_control(action, window, cx);
                }
            }
            HostConnectionAccessibilityCommand::SetControlValue(action) => {
                if !snapshot.controls.iter().any(|control| {
                    control.action == action
                        && !control.disabled
                        && matches!(
                            control.role,
                            HostConnectionControlRole::TextField
                                | HostConnectionControlRole::PasswordField
                        )
                }) {
                    return;
                }
                let Some(SemanticActionValue::Text(value)) = value else {
                    return;
                };
                self.set_host_connection_control_value(action, value, window, cx);
            }
            HostConnectionAccessibilityCommand::ActivateControl(action) => {
                if !snapshot.controls.iter().any(|control| {
                    control.action == action
                        && !control.disabled
                        && !matches!(
                            control.role,
                            HostConnectionControlRole::TextField
                                | HostConnectionControlRole::PasswordField
                        )
                }) {
                    return;
                }
                self.activate_host_connection_control(action, window, cx);
            }
        }
    }

    fn profile_id_for_host_row(&self, row_id: HostConnectionRowId) -> Option<String> {
        (row_id.kind == termirust_ui_contract::HostConnectionRowKind::Host)
            .then(|| {
                self.saved
                    .profiles
                    .iter()
                    .find(|profile| host_row_id(&profile.id) == row_id)
            })
            .flatten()
            .map(|profile| profile.id.clone())
    }

    fn profile_id_for_diagnostic_row(&self, row_id: HostConnectionRowId) -> Option<String> {
        self.connection_diagnostics
            .iter()
            .find(|row| {
                row.operation_id as u128 == row_id.value
                    && stable_host_row_value(&row.profile_id) == row_id.owner
            })
            .map(|row| row.profile_id.clone())
    }

    fn focus_host_connection_control(
        &self,
        action: HostConnectionAction,
        window: &mut Window,
        cx: &App,
    ) {
        let input = match action {
            HostConnectionAction::SetSearch => Some(&self.shell_inputs.host_search),
            HostConnectionAction::SetQuickConnectPassword => {
                Some(&self.shell_inputs.quick_connect_password)
            }
            HostConnectionAction::SetBulkGroup => Some(&self.shell_inputs.bulk_group),
            HostConnectionAction::SetHostLabel => Some(&self.inputs.label),
            HostConnectionAction::SetHostAddress => Some(&self.inputs.host),
            HostConnectionAction::SetHostPort => Some(&self.inputs.port),
            HostConnectionAction::SetHostUsername => Some(&self.inputs.username),
            HostConnectionAction::SetHostPassword => Some(&self.inputs.password),
            HostConnectionAction::SetHostKeyPath => Some(&self.inputs.key_path),
            HostConnectionAction::SetHostKeyPassphrase => Some(&self.inputs.key_passphrase),
            HostConnectionAction::SetHostAgentSocket => Some(&self.inputs.identity_agent),
            HostConnectionAction::SetConnectUsername => Some(&self.shell_inputs.connect_username),
            HostConnectionAction::SetProtocolPort => Some(&self.shell_inputs.protocol_ssh_port),
            _ => None,
        };
        if let Some(input) = input {
            input.read(cx).focus_handle(cx).focus(window);
        } else {
            self.project_list_focus.focus(window);
        }
    }

    fn set_host_connection_control_value(
        &mut self,
        action: HostConnectionAction,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match action {
            HostConnectionAction::SetSearch => Some(&self.shell_inputs.host_search),
            HostConnectionAction::SetQuickConnectPassword => {
                Some(&self.shell_inputs.quick_connect_password)
            }
            HostConnectionAction::SetBulkGroup => Some(&self.shell_inputs.bulk_group),
            HostConnectionAction::SetHostLabel => Some(&self.inputs.label),
            HostConnectionAction::SetHostAddress => Some(&self.inputs.host),
            HostConnectionAction::SetHostPort => Some(&self.inputs.port),
            HostConnectionAction::SetHostUsername => Some(&self.inputs.username),
            HostConnectionAction::SetHostPassword => Some(&self.inputs.password),
            HostConnectionAction::SetHostKeyPath => Some(&self.inputs.key_path),
            HostConnectionAction::SetHostKeyPassphrase => Some(&self.inputs.key_passphrase),
            HostConnectionAction::SetHostAgentSocket => Some(&self.inputs.identity_agent),
            HostConnectionAction::SetConnectUsername => Some(&self.shell_inputs.connect_username),
            HostConnectionAction::SetProtocolPort => Some(&self.shell_inputs.protocol_ssh_port),
            _ => None,
        };
        if let Some(input) = input {
            Self::set_input_value(input, value, window, cx);
        }
    }

    fn activate_host_connection_control(
        &mut self,
        action: HostConnectionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HostConnectionAction::AddHost => self.open_editor_for_new_host(window, cx),
            HostConnectionAction::QuickConnect => {
                if let Some(target) = self.try_quick_connect_from_search(cx) {
                    let password = self.current_quick_connect_password(cx);
                    self.quick_connect(
                        target,
                        (!password.trim().is_empty()).then_some(password),
                        window,
                        cx,
                    );
                }
            }
            HostConnectionAction::SelectHost(row) | HostConnectionAction::EditHost(row) => {
                if let Some(profile_id) = self.profile_id_for_host_row(row) {
                    self.select_profile_from_library(&profile_id, window, cx);
                }
            }
            HostConnectionAction::ConnectHost(row) => {
                if let Some(profile_id) = self.profile_id_for_host_row(row) {
                    self.open_connect_dialog_tab(&profile_id, window, cx);
                }
            }
            HostConnectionAction::ToggleFavorite(row) => {
                if let Some(profile_id) = self.profile_id_for_host_row(row)
                    && let Some(favorite) = self
                        .saved
                        .profiles
                        .iter()
                        .find(|profile| profile.id == profile_id)
                        .map(|profile| !profile.favorite)
                {
                    self.set_profile_favorite(&profile_id, favorite, window, cx);
                }
            }
            HostConnectionAction::ToggleBatchSelection(row) => {
                if let Some(profile_id) = self.profile_id_for_host_row(row) {
                    self.toggle_host_batch_selection(&profile_id, cx);
                }
            }
            HostConnectionAction::SelectVisible => self.select_all_filtered_hosts(cx),
            HostConnectionAction::ClearSelection => self.clear_host_batch_selection(cx),
            HostConnectionAction::DiagnoseSelected => self.diagnose_selected_hosts(cx),
            HostConnectionAction::ClearFinishedDiagnostics => {
                self.clear_finished_connection_diagnostics(cx)
            }
            HostConnectionAction::CancelDiagnostic(row) => {
                if self.profile_id_for_diagnostic_row(row).is_some() {
                    self.cancel_connection_diagnostic(row.value as u64, cx);
                }
            }
            HostConnectionAction::RetryDiagnostic(row) => {
                if let Some(profile_id) = self.profile_id_for_diagnostic_row(row) {
                    self.retry_connection_diagnostic(&profile_id, cx);
                }
            }
            HostConnectionAction::ApplyBulkGroup => {
                self.bulk_assign_selected_hosts_group(window, cx)
            }
            HostConnectionAction::SelectAuth(choice) => self.set_auth_mode(
                match choice {
                    HostAuthChoice::Password => AuthMode::Password,
                    HostAuthChoice::PrivateKey => AuthMode::PrivateKey,
                    HostAuthChoice::LocalAgent => AuthMode::LocalAgent,
                },
                cx,
            ),
            HostConnectionAction::SaveHost => self.save_profile(window, cx),
            HostConnectionAction::DeleteHost => self.remove_selected_profile(window, cx),
            HostConnectionAction::CloseHostEditor => self.close_editor_dialog(window, cx),
            HostConnectionAction::ContinueAndSave => self.confirm_connect_dialog(true, window, cx),
            HostConnectionAction::SelectProtocol(_) => {
                if let Some(workspace) = self.active_workspace_id.and_then(|id| {
                    self.workspaces
                        .iter_mut()
                        .find(|workspace| workspace.id == id)
                }) {
                    workspace.pending_connect_protocol = crate::ui::app::ConnectProtocol::Ssh;
                    cx.notify();
                }
            }
            HostConnectionAction::ContinueProtocol => {
                if let Some(workspace_id) = self.active_workspace_id {
                    self.confirm_choose_protocol(workspace_id, window, cx);
                }
            }
            HostConnectionAction::ForwardAgentOnce => {
                if let Some(workspace_id) = self.active_workspace_id {
                    self.confirm_choose_protocol_with_agent_forwarding(workspace_id, window, cx);
                }
            }
            HostConnectionAction::CopyFailureLog => self.copy_active_connect_failure_log(cx),
            HostConnectionAction::EditFailedHost => self.edit_active_connect_failure(window, cx),
            HostConnectionAction::RetryConnection => {
                if let Some(workspace_id) = self.active_workspace_id {
                    self.restart_choose_protocol(workspace_id, cx);
                }
            }
            HostConnectionAction::CloseConnectionDialog => {
                if let Some(workspace_id) = self.active_workspace_id {
                    self.close_connect_dialog_tab(workspace_id, window, cx);
                }
            }
            HostConnectionAction::SetSearch
            | HostConnectionAction::SetQuickConnectPassword
            | HostConnectionAction::SetBulkGroup
            | HostConnectionAction::SetHostLabel
            | HostConnectionAction::SetHostAddress
            | HostConnectionAction::SetHostPort
            | HostConnectionAction::SetHostUsername
            | HostConnectionAction::SetHostPassword
            | HostConnectionAction::SetHostKeyPath
            | HostConnectionAction::SetHostKeyPassphrase
            | HostConnectionAction::SetHostAgentSocket
            | HostConnectionAction::SetConnectUsername
            | HostConnectionAction::SetProtocolPort => {}
        }
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
        let protocols = match profile.auth_mode {
            AuthMode::Password => "password",
            AuthMode::PrivateKey => "key auth",
            AuthMode::LocalAgent => "SSH agent",
        };
        let protocol_icon = match profile.auth_mode {
            AuthMode::Password => Icon::new(IconName::User),
            AuthMode::PrivateKey | AuthMode::LocalAgent => app_icon(ICON_KEY),
        };

        let visible_tags: Vec<String> = profile.tags.iter().take(4).cloned().collect();
        let _ = (
            tags,
            identity_label,
            jump_host_label,
            startup_label,
            connect_view_label,
            scrollback_label,
            forward_label,
            last_connected_label,
            protocols,
            protocol_icon,
            selected,
        );

        let _ = (group_label, visible_tags, connect_profile_id);
        let sublabel = if profile.username.trim().is_empty() {
            "ssh".to_string()
        } else {
            format!("{}@{}", profile.username, profile.endpoint())
        };
        let icon_bg = theme::with_alpha(accent, 0.18);
        h_flex()
            .id(("host-row", card_ix))
            .debug_selector(move || format!("host-row-{card_ix}"))
            .group(format!("host-row-group-{card_ix}"))
            .w(px(theme::HOST_CARD_WIDTH))
            .h(px(theme::HOST_CARD_HEIGHT))
            .gap(px(theme::SPACE_4))
            .items_center()
            .px(px(theme::SPACE_COMPACT))
            .rounded(px(theme::HOST_CARD_RADIUS))
            .border_2()
            .border_color(if batch_selected {
                theme::with_alpha(theme::accent(), 0.5)
            } else {
                gpui::transparent_black()
            })
            .bg(theme::library_card())
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(theme::with_alpha(theme::hover(), 0.6))
                    .border_color(theme::accent())
            })
            .on_click(cx.listener({
                let profile_id = profile_id.clone();
                move |this, event: &ClickEvent, window, cx| {
                    let click_count = match event {
                        ClickEvent::Mouse(e) => e.up.click_count,
                        ClickEvent::Keyboard(_) => 1,
                    };
                    if click_count >= 2 {
                        this.open_connect_dialog_tab(&profile_id, window, cx);
                    } else {
                        this.select_profile_from_library(&profile_id, window, cx);
                    }
                }
            }))
            .child(
                div()
                    .size(px(theme::CHROME_HEIGHT))
                    .rounded(px(theme::CARD_RADIUS))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(icon_bg)
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(theme::ICON_SIZE_MEDIUM))
                            .text_color(accent),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(theme::SPACE_1))
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_DENSE))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(profile.display_name()),
                            )
                            .when(profile.favorite, |this| {
                                this.child(
                                    Icon::new(IconName::Star)
                                        .size(px(theme::HOST_ICON_SIZE_TINY))
                                        .text_color(theme::warning()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .child(sublabel),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(theme::SPACE_1))
                    .child(
                        div()
                            .debug_selector(move || format!("host-row-select-{card_ix}"))
                            .child(
                                Button::new(("host-row-select", card_ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(if batch_selected {
                                        IconName::Check
                                    } else {
                                        IconName::Plus
                                    })
                                    .tooltip(if batch_selected {
                                        host_message(MessageId::HostDeselectAction)
                                    } else {
                                        host_message(MessageId::HostSelectAction)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_host_batch_selection(&batch_profile_id, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("host-row-favorite-{card_ix}"))
                            .child(
                                Button::new(("host-row-favorite", card_ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Star)
                                    .tooltip(if favorite_selected {
                                        host_message(MessageId::HostUnstarAction)
                                    } else {
                                        host_message(MessageId::HostStarAction)
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.set_profile_favorite(
                                            &favorite_profile_id,
                                            !favorite_selected,
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .child({
                        let edit_profile_id = profile_id.clone();
                        div()
                            .id(("host-row-edit", card_ix))
                            .debug_selector(move || format!("host-row-edit-{card_ix}"))
                            .size(px(theme::STATUS_HEIGHT))
                            .rounded(px(theme::CONTROL_RADIUS))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(theme::text_muted())
                            .hover(|style| {
                                style
                                    .bg(theme::with_alpha(theme::hover(), 0.85))
                                    .text_color(theme::text_main())
                            })
                            .child(app_icon(ICON_PENCIL).size(px(theme::HOST_ICON_SIZE_BODY)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.load_profile_into_inputs(&edit_profile_id, window, cx);
                                this.show_editor_panel = true;
                                cx.notify();
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    }),
            )
    }

    fn host_list_row(
        &self,
        card_ix: usize,
        profile: &HostProfile,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let profile_id = profile.id.clone();
        let select_id = profile.id.clone();
        let favorite_id = profile.id.clone();
        let edit_id = profile.id.clone();
        let display = profile.display_name();
        let endpoint = format!("{}@{}", profile.username, profile.endpoint());
        let is_batch_selected = self.selected_host_ids.contains(profile.id.as_str());
        let favorite_selected = profile.favorite;
        let accent = match profile.color_tag {
            Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
            None => theme::host_chip_color(&display),
        };
        h_flex()
            .id(("host-row-list", card_ix))
            .debug_selector(move || format!("host-row-list-{card_ix}"))
            .w_full()
            .h(px(theme::HOST_CONTROL_HEIGHT))
            .gap(px(theme::SPACE_COMPACT))
            .px(px(theme::SPACE_COMPACT))
            .items_center()
            .rounded(px(theme::CONTROL_RADIUS))
            .bg(if selected {
                theme::accent_soft()
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
            .on_click(cx.listener({
                let pid = profile_id.clone();
                move |this, event: &ClickEvent, window, cx| {
                    let click_count = match event {
                        ClickEvent::Mouse(e) => e.up.click_count,
                        ClickEvent::Keyboard(_) => 1,
                    };
                    if click_count >= 2 {
                        this.open_connect_dialog_tab(&pid, window, cx);
                    } else {
                        this.select_profile_from_library(&pid, window, cx);
                    }
                }
            }))
            .child(
                div()
                    .size(px(theme::CARD_RADIUS))
                    .rounded(px(theme::PILL_RADIUS))
                    .bg(accent),
            )
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(theme::HOST_ICON_SIZE_DENSE))
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .w(px(theme::SHELL_TAB_LABEL_MAXIMUM))
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(display),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(endpoint),
            )
            .child(
                div()
                    .debug_selector(move || format!("host-row-list-select-{card_ix}"))
                    .child(
                        Button::new(("host-row-list-select", card_ix))
                            .xsmall()
                            .ghost()
                            .icon(if is_batch_selected {
                                IconName::Check
                            } else {
                                IconName::Plus
                            })
                            .tooltip(if is_batch_selected {
                                host_message(MessageId::HostDeselectAction)
                            } else {
                                host_message(MessageId::HostSelectAction)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_host_batch_selection(&select_id, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(move || format!("host-row-list-favorite-{card_ix}"))
                    .child(
                        Button::new(("host-row-list-favorite", card_ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Star)
                            .tooltip(if favorite_selected {
                                host_message(MessageId::HostUnstarAction)
                            } else {
                                host_message(MessageId::HostStarAction)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.set_profile_favorite(
                                    &favorite_id,
                                    !favorite_selected,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child({
                div()
                    .id(("host-row-list-edit", card_ix))
                    .debug_selector(move || format!("host-row-list-edit-{card_ix}"))
                    .size(px(theme::SPACE_6))
                    .rounded(px(theme::SPACE_2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(theme::text_muted())
                    .hover(|s| {
                        s.bg(theme::with_alpha(theme::hover(), 0.85))
                            .text_color(theme::text_main())
                    })
                    .child(app_icon(ICON_PENCIL).size(px(theme::HOST_ICON_SIZE_DENSE)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.load_profile_into_inputs(&edit_id, window, cx);
                        this.show_editor_panel = true;
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
    }

    fn render_host_grid(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let groups = self.grouped_profiles(cx);

        let mut sections = Vec::new();
        let mut card_ix = 0usize;
        for (group_index, (group_name, profiles)) in groups.iter().enumerate() {
            let visible_count = profiles.len();
            let fleet_group_name = group_name.clone();
            let fleet_profile_ids = profiles
                .iter()
                .map(|profile| profile.id.clone())
                .collect::<Vec<_>>();
            let total_count = self
                .saved
                .profiles
                .iter()
                .filter(|profile| Self::profile_group_name(profile) == *group_name)
                .count();
            let _ = total_count;
            let is_only_ungrouped = groups.len() == 1 && group_name == "Ungrouped";
            let is_list = self.hosts_view_mode == HostsViewMode::List;
            let cards: Div = if is_list {
                v_flex()
                    .w_full()
                    .gap(px(theme::SPACE_1))
                    .children(profiles.iter().enumerate().map(|(group_ix, profile)| {
                        self.host_list_row(
                            card_ix + group_ix,
                            profile,
                            self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                            cx,
                        )
                        .into_any_element()
                    }))
            } else {
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(theme::SPACE_COMPACT))
                    .children(profiles.iter().enumerate().map(|(group_ix, profile)| {
                        self.host_card(
                            card_ix + group_ix,
                            profile,
                            self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                            self.selected_host_ids.contains(profile.id.as_str()),
                            cx,
                        )
                        .into_any_element()
                    }))
            };

            let mut section = v_flex().w_full().gap(px(theme::SPACE_COMPACT));
            if !is_only_ungrouped {
                section = section.child(
                    h_flex()
                        .min_h(px(theme::STATUS_HEIGHT))
                        .items_center()
                        .justify_between()
                        .pl(px(theme::SPACE_1))
                        .child(
                            h_flex()
                                .items_end()
                                .gap(px(theme::SPACE_3))
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_semibold()
                                        .text_color(theme::text_main())
                                        .child(group_name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_MICRO_SIZE))
                                        .text_color(theme::text_muted())
                                        .pb(px(theme::BORDER_HAIRLINE))
                                        .child(format!(
                                            "{} {}",
                                            visible_count,
                                            if visible_count == 1 { "host" } else { "hosts" }
                                        )),
                                ),
                        )
                        .when(visible_count > 1, |header| {
                            header.child(
                                Button::new(("hosts-open-fleet", group_index))
                                    .debug_selector(move || {
                                        format!("hosts-open-fleet-{group_index}")
                                    })
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::AccentSoft,
                                        cx,
                                    ))
                                    .icon(IconName::Globe)
                                    .label(host_message(MessageId::HostsOpenFleetAction))
                                    .tooltip(localization::hosts_open_fleet_tooltip(visible_count))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_saved_host_fleet_canvas(
                                            &fleet_group_name,
                                            fleet_profile_ids.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        }),
                );
            }
            section = section.child(cards);
            sections.push(section.into_any_element());
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
                            .size(px(theme::SPACE_6))
                            .text_color(theme::accent()),
                        "Create host",
                        "Save your connection details as hosts to connect in one click.",
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(theme::HOST_CONTROL_HEIGHT))
                            .px(px(theme::SPACE_4))
                            .items_center()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .border_1()
                            .border_color(theme::soft_border())
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_main())
                            .child(
                                Input::new(&self.shell_inputs.create_host_address)
                                    .appearance(false)
                                    .flex_1(),
                            ),
                    )
                    .child(
                        Button::new("hosts-empty-new")
                            .w_full()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label(host_message(MessageId::ConnectContinueAction))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !this.submit_create_host_from_empty_state(window, cx) {
                                    this.open_editor_for_new_host(window, cx);
                                }
                            })),
                    )
                } else {
                    self.render_library_empty_state(
                        Icon::new(IconName::Search)
                            .size(px(theme::SPACE_6))
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
                                    .label(host_message(MessageId::HostsClearSearch))
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
                                    .label(host_message(MessageId::HostsAddAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    )
                };
                this.child(empty_state)
            })
    }

    fn render_new_host_split_button(&self, cx: &mut Context<Self>) -> Div {
        let menu_open = self.show_new_host_menu;
        div()
            .relative()
            .child(
                h_flex()
                    .gap(px(theme::SPACE_0))
                    .items_center()
                    .child(
                        Button::new("library-new-host")
                            .debug_selector(|| "library-new-host".to_string())
                            .xsmall()
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .icon(IconName::Plus)
                            .label(host_message(MessageId::HostsAddAction))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor_for_new_host(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("library-new-host-chevron")
                            .debug_selector(|| "library-new-host-chevron".to_string())
                            .h(px(theme::STATUS_HEIGHT))
                            .px(px(theme::SPACE_DENSE))
                            .ml(px(theme::SPACE_1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::CONTROL_RADIUS))
                            .bg(if menu_open {
                                theme::with_alpha(theme::hover(), 0.85)
                            } else {
                                gpui::transparent_black()
                            })
                            .border_1()
                            .border_color(theme::soft_border())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                Icon::new(if menu_open {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(theme::SPACE_4))
                                .text_color(theme::text_main()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_new_host_menu(cx);
                            })),
                    ),
            )
            .when(false, |this| {
                this.child(
                    v_flex()
                        .absolute()
                        .top(px(theme::SHELL_NAVIGATION_ROW_HEIGHT))
                        .left(px(theme::SPACE_0))
                        .w(px(theme::HOST_SIDEBAR_WIDTH))
                        .py(px(theme::SPACE_DENSE))
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::border())
                        .shadow(theme::popover_shadow())
                        .child(self.new_host_menu_item(
                            "menu-new-group-x",
                            IconName::Folder,
                            "New Group",
                            false,
                            cx,
                            |this, window, cx| {
                                this.show_new_host_menu = false;
                                this.open_editor_for_new_host(window, cx);
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-import-x",
                            IconName::PanelLeft,
                            "Import",
                            false,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    host_message(MessageId::HostsImportDescription);
                                cx.notify();
                            },
                        ))
                        .child(
                            div()
                                .h(px(theme::BORDER_HAIRLINE))
                                .w_full()
                                .my(px(theme::SPACE_2))
                                .bg(theme::soft_border()),
                        )
                        .child(self.new_host_menu_item(
                            "menu-aws-x",
                            IconName::Globe,
                            "AWS Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    localization::hosts_provider_unavailable(PROVIDER_AWS);
                                cx.notify();
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-do-x",
                            IconName::Globe,
                            "DigitalOcean Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message = localization::hosts_provider_unavailable(
                                    PROVIDER_DIGITAL_OCEAN,
                                );
                                cx.notify();
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-azure-x",
                            IconName::Globe,
                            "Azure Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    localization::hosts_provider_unavailable(PROVIDER_AZURE);
                                cx.notify();
                            },
                        )),
                )
            })
    }

    fn new_host_menu_item(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        cloud: bool,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        h_flex()
            .id(id)
            .debug_selector(|| id.to_string())
            .w_full()
            .h(px(theme::SHELL_COMPACT_CONTROL_HEIGHT))
            .px(px(theme::SPACE_4))
            .gap(px(theme::SPACE_COMPACT))
            .items_center()
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .child(
                Icon::new(icon)
                    .size(px(theme::HOST_ICON_SIZE_BODY))
                    .text_color(theme::text_muted_dark()),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(label),
            )
            .when(cloud, |this| {
                this.child(
                    Icon::new(IconName::ExternalLink)
                        .size(px(theme::HOST_ICON_SIZE_TINY))
                        .text_color(theme::text_muted_dark()),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                handler(this, window, cx);
            }))
    }

    fn toolbar_chevron_svg_button(
        &self,
        id: &'static str,
        svg_path: &'static str,
        _tooltip: &'static str,
        menu: ToolbarMenu,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.open_toolbar_menu == Some(menu);
        let chevron = if open {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };
        div()
            .id(id)
            .debug_selector(|| id.to_string())
            .h(px(theme::STATUS_HEIGHT))
            .px(px(theme::SPACE_3))
            .gap(px(theme::SPACE_2))
            .flex()
            .items_center()
            .rounded(px(theme::CONTROL_RADIUS))
            .cursor_pointer()
            .when(open, |this| this.bg(theme::with_alpha(theme::hover(), 0.7)))
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .child(
                app_icon(svg_path)
                    .size(px(theme::HOST_ICON_SIZE_BODY))
                    .text_color(theme::text_main()),
            )
            .child(
                Icon::new(chevron)
                    .size(px(theme::HOST_ICON_SIZE_TINY))
                    .text_color(theme::text_muted_dark()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_toolbar_menu = if this.open_toolbar_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            }))
    }

    pub(super) fn dropdown_item(
        &self,
        id: impl Into<ElementId>,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        h_flex()
            .id(id)
            .h(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
            .px(px(theme::SPACE_COMPACT))
            .gap(px(theme::SPACE_COMPACT))
            .items_center()
            .rounded(px(theme::CONTROL_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .when_some(icon, |this, icon| {
                this.child(
                    icon.size(px(theme::HOST_ICON_SIZE_DENSE))
                        .text_color(theme::text_muted_dark()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_main())
                    .child(label),
            )
            .when(selected, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .size(px(theme::SPACE_4))
                        .text_color(theme::accent()),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
                this.open_toolbar_menu = None;
                this.open_editor_menu = None;
                cx.notify();
            }))
    }

    fn render_view_mode_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-view-mode",
            ICON_GRID,
            "View mode",
            ToolbarMenu::ViewMode,
            cx,
        )
    }

    fn render_tag_filter_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-tag-filter",
            ICON_TAG,
            "Filter by tag",
            ToolbarMenu::TagFilter,
            cx,
        )
    }

    fn render_sort_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-sort",
            ICON_CALENDAR,
            "Sort hosts",
            ToolbarMenu::Sort,
            cx,
        )
    }

    fn render_avatar_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("library-avatar-trigger")
            .debug_selector(|| "library-avatar-trigger".to_string())
            .cursor_pointer()
            .child(self.toolbar_avatar_pill(cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_toolbar_menu = if this.open_toolbar_menu == Some(ToolbarMenu::Avatar) {
                    None
                } else {
                    Some(ToolbarMenu::Avatar)
                };
                cx.notify();
            }))
    }

    fn render_hosts_overlays(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.show_editor_panel {
            if self.open_editor_menu == Some(EditorMenu::Vault) {
                let mut panel = v_flex()
                    .min_w(px(theme::HOST_SIDEBAR_WIDTH))
                    .p(px(theme::SPACE_DENSE))
                    .gap(px(theme::SPACE_1))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg();
                let active = self.draft_vault_id.clone();
                for (idx, vault) in self.saved.vaults.iter().enumerate() {
                    let id = vault.id.clone();
                    let display = vault.display_name();
                    let is_active = active.as_deref() == Some(id.as_str());
                    panel = panel.child(self.dropdown_item(
                        ("vault-pick", idx),
                        Some(app_icon(ICON_VAULT)),
                        display,
                        is_active,
                        move |this, _, _| {
                            this.draft_vault_id = Some(id.clone());
                        },
                        cx,
                    ));
                }
                return div()
                    .id("editor-vault-overlay")
                    .absolute()
                    .top(px(theme::HOST_OVERLAY_LOW_TOP))
                    .right(px(theme::HOST_OVERLAY_RIGHT_WIDE))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_editor_menu = None;
                        cx.notify();
                    }))
                    .child(panel)
                    .into_any_element();
            }
            if self.open_editor_menu == Some(EditorMenu::Overflow) {
                let has_profile = self.selected_profile_id.is_some();
                let mut menu = v_flex()
                    .min_w(px(theme::HOST_MENU_WIDTH))
                    .p(px(theme::SPACE_DENSE))
                    .gap(px(theme::SPACE_1))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg()
                    .child(self.dropdown_item(
                        "overflow-connect",
                        Some(Icon::new(IconName::SquareTerminal)),
                        "Connect",
                        false,
                        |this, window, cx| {
                            eprintln!(
                                "[connect] overflow Connect clicked, selected_profile_id={:?}",
                                this.selected_profile_id
                            );
                            if let Some(id) = this.selected_profile_id.clone() {
                                this.open_choose_protocol_tab(&id, window, cx);
                            } else {
                                this.open_choose_protocol_tab_from_draft(window, cx);
                            }
                        },
                        cx,
                    ))
                    .child(self.dropdown_item(
                        "overflow-duplicate",
                        Some(Icon::new(IconName::Copy)),
                        "Duplicate",
                        false,
                        |this, _, cx| {
                            if let Some(id) = this.selected_profile_id.clone() {
                                if let Some(orig) =
                                    this.saved.profiles.iter().find(|p| p.id == id).cloned()
                                {
                                    let mut copy = orig.clone();
                                    copy.id =
                                        format!("{}-copy-{}", orig.id, this.next_session_id());
                                    copy.label = format!("{} (copy)", orig.label);
                                    this.saved.upsert_profile(copy.clone());
                                    this.selected_profile_id = Some(copy.id);
                                    this.persist_runtime_state();
                                    cx.notify();
                                }
                            }
                        },
                        cx,
                    ));
                if has_profile {
                    menu = menu.child(self.dropdown_item(
                        "overflow-remove",
                        Some(Icon::new(IconName::Delete)),
                        "Remove",
                        false,
                        |this, _, cx| {
                            if let Some(id) = this.selected_profile_id.clone() {
                                this.saved.remove_profile(&id);
                                this.show_editor_panel = false;
                                this.persist_runtime_state();
                                cx.notify();
                            }
                        },
                        cx,
                    ));
                }
                return div()
                    .id("editor-overflow-overlay")
                    .absolute()
                    .top(px(theme::HOST_OVERLAY_LOW_TOP))
                    .right(px(theme::HOST_OVERLAY_RIGHT_NARROW))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_editor_menu = None;
                        cx.notify();
                    }))
                    .child(menu)
                    .into_any_element();
            }
        }
        if self.show_new_host_menu {
            return div()
                .id("new-host-overlay")
                .absolute()
                .top(px(theme::HOST_OVERLAY_TOP))
                .left(px(theme::SPACE_4))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.show_new_host_menu = false;
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .w(px(theme::HOST_SIDEBAR_WIDTH))
                        .py(px(theme::SPACE_DENSE))
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::border())
                        .shadow_lg()
                        .child(self.new_host_menu_item(
                            "menu-new-group",
                            IconName::Folder,
                            "New Group",
                            false,
                            cx,
                            |this, window, cx| {
                                this.show_new_host_menu = false;
                                this.open_editor_for_new_host(window, cx);
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-import",
                            IconName::PanelLeft,
                            "Import from ~/.ssh/config",
                            false,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                match crate::storage::load_local_ssh_hosts() {
                                    Ok(hosts) => {
                                        let count = hosts.len();
                                        this.saved.merge_imported_profiles(hosts);
                                        this.persist_runtime_state();
                                        this.status_message =
                                            localization::hosts_imported_count(count);
                                    }
                                    Err(e) => {
                                        this.error_message =
                                            localization::hosts_import_error(e.to_string());
                                    }
                                }
                                cx.notify();
                            },
                        )),
                )
                .into_any_element();
        }
        let menu = self.open_toolbar_menu;
        let email = std::env::var("USER")
            .ok()
            .map(|u| format!("{u}@local"))
            .unwrap_or_else(|| "user@local".to_string());
        let mut tags: Vec<String> = self
            .saved
            .profiles
            .iter()
            .flat_map(|p| p.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        let inner: Div = match menu {
            Some(ToolbarMenu::ViewMode) => v_flex()
                .min_w(px(theme::HOST_MENU_NARROW_WIDTH))
                .p(px(theme::SPACE_DENSE))
                .gap(px(theme::SPACE_1))
                .rounded(px(theme::CARD_RADIUS))
                .bg(theme::library_card())
                .border_1()
                .border_color(theme::soft_border())
                .shadow_lg()
                .child(
                    self.dropdown_item(
                        "view-mode-grid",
                        Some(app_icon(ICON_GRID)),
                        "Grid",
                        self.hosts_view_mode == HostsViewMode::Grid,
                        |this, _, _| this.hosts_view_mode = HostsViewMode::Grid,
                        cx,
                    )
                    .debug_selector(|| "view-mode-grid".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "view-mode-list",
                        Some(Icon::new(IconName::Menu)),
                        "List",
                        self.hosts_view_mode == HostsViewMode::List,
                        |this, _, _| this.hosts_view_mode = HostsViewMode::List,
                        cx,
                    )
                    .debug_selector(|| "view-mode-list".to_string()),
                ),
            Some(ToolbarMenu::TagFilter) => {
                if tags.is_empty() {
                    v_flex()
                        .w(px(theme::HOST_SIDEBAR_WIDTH))
                        .p(px(theme::ICON_SIZE_MEDIUM))
                        .gap(px(theme::SPACE_COMPACT))
                        .items_center()
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::soft_border())
                        .shadow_lg()
                        .child(
                            div()
                                .size(px(theme::SHELL_NAVIGATION_ROW_HEIGHT))
                                .rounded(px(theme::CARD_RADIUS))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(theme::with_alpha(theme::hover(), 0.7))
                                .child(
                                    app_icon(ICON_TAG)
                                        .size(px(theme::ICON_SIZE_DEFAULT))
                                        .text_color(theme::text_main()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child(host_message(MessageId::HostsAddTagsAction)),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_MICRO_SIZE))
                                .text_color(theme::text_muted())
                                .child(host_message(MessageId::HostsTagsHelp)),
                        )
                } else {
                    let mut panel = v_flex()
                        .min_w(px(theme::HOST_MENU_WIDTH))
                        .p(px(theme::SPACE_DENSE))
                        .gap(px(theme::SPACE_1))
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::soft_border())
                        .shadow_lg();
                    let active_filter = self.hosts_tag_filter.clone();
                    panel = panel.child(
                        self.dropdown_item(
                            "tag-filter-all",
                            Some(app_icon(ICON_TAG)),
                            "All hosts",
                            active_filter.is_none(),
                            |this, _, _| this.hosts_tag_filter = None,
                            cx,
                        )
                        .debug_selector(|| "tag-filter-all".to_string()),
                    );
                    for (idx, tag) in tags.iter().enumerate() {
                        let tag_owned = tag.clone();
                        let is_active = active_filter.as_deref() == Some(tag.as_str());
                        panel = panel.child(
                            self.dropdown_item(
                                ("tag-filter", idx),
                                Some(app_icon(ICON_TAG)),
                                tag.clone(),
                                is_active,
                                move |this, _, _| {
                                    this.hosts_tag_filter = Some(tag_owned.clone());
                                },
                                cx,
                            )
                            .debug_selector(move || format!("tag-filter-{idx}")),
                        );
                    }
                    panel
                }
            }
            Some(ToolbarMenu::Sort) => v_flex()
                .min_w(px(theme::HOST_MENU_WIDTH))
                .p(px(theme::SPACE_DENSE))
                .gap(px(theme::SPACE_1))
                .rounded(px(theme::CARD_RADIUS))
                .bg(theme::library_card())
                .border_1()
                .border_color(theme::soft_border())
                .shadow_lg()
                .child(
                    self.dropdown_item(
                        "sort-az",
                        Some(Icon::new(IconName::SortAscending)),
                        "A-z",
                        self.hosts_sort == HostsSort::AZ,
                        |this, _, _| this.hosts_sort = HostsSort::AZ,
                        cx,
                    )
                    .debug_selector(|| "sort-az".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "sort-za",
                        Some(Icon::new(IconName::SortDescending)),
                        "Z-a",
                        self.hosts_sort == HostsSort::ZA,
                        |this, _, _| this.hosts_sort = HostsSort::ZA,
                        cx,
                    )
                    .debug_selector(|| "sort-za".to_string()),
                )
                .child(
                    div()
                        .h(px(theme::BORDER_HAIRLINE))
                        .my(px(theme::SPACE_2))
                        .bg(theme::soft_border()),
                )
                .child(
                    self.dropdown_item(
                        "sort-newest",
                        Some(app_icon(ICON_CALENDAR)),
                        "Newest to oldest",
                        self.hosts_sort == HostsSort::NewestFirst,
                        |this, _, _| this.hosts_sort = HostsSort::NewestFirst,
                        cx,
                    )
                    .debug_selector(|| "sort-newest".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "sort-oldest",
                        Some(app_icon(ICON_CALENDAR)),
                        "Oldest to newest",
                        self.hosts_sort == HostsSort::OldestFirst,
                        |this, _, _| this.hosts_sort = HostsSort::OldestFirst,
                        cx,
                    )
                    .debug_selector(|| "sort-oldest".to_string()),
                ),
            Some(ToolbarMenu::Avatar) => {
                let invite_email = email.clone();
                let copy_email = email.clone();
                v_flex()
                    .min_w(px(theme::HOST_MENU_WIDE_WIDTH))
                    .p(px(theme::SPACE_DENSE))
                    .gap(px(theme::SPACE_1))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg()
                    .child(
                        self.dropdown_item(
                            "avatar-invite",
                            Some(Icon::new(IconName::User)),
                            "Invite team members",
                            false,
                            move |this, _, cx| {
                                #[cfg(not(test))]
                                let _ = std::process::Command::new("open")
                                    .arg(format!(
                                        "mailto:?subject=Join%20me%20on%20TermiRust&body=I%27m%20using%20TermiRust%20at%20{invite_email}"
                                    ))
                                    .spawn();
                                #[cfg(test)]
                                let _ = &invite_email;
                                this.status_message = host_message(MessageId::HostsEmailOpened);
                                cx.notify();
                            },
                            cx,
                        )
                        .debug_selector(|| "avatar-invite".to_string()),
                    )
                    .child(div().h(px(theme::BORDER_HAIRLINE)).my(px(theme::SPACE_2)).bg(theme::soft_border()))
                    .child(
                        self.dropdown_item(
                            "avatar-email",
                            None,
                            email,
                            false,
                            move |this, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_email.clone()));
                                this.status_message = host_message(MessageId::HostsEmailCopied);
                                cx.notify();
                            },
                            cx,
                        )
                        .debug_selector(|| "avatar-email".to_string()),
                    )
            }
            None => return div().into_any_element(),
        };
        // Toolbar row layout (right-aligned): chrome pr=12, then AvatarPill (52px,
        // ml=4), Sort chevron (45px), Tag (45px), View (45px), all separated by
        // gap=4 in the parent h_flex.
        let right_offset = match menu {
            Some(ToolbarMenu::ViewMode) => px(theme::HOST_TOOLBAR_OFFSET_VIEW),
            Some(ToolbarMenu::TagFilter) => px(theme::HOST_TOOLBAR_OFFSET_TAG),
            Some(ToolbarMenu::Sort) => px(theme::HOST_TOOLBAR_OFFSET_SORT),
            Some(ToolbarMenu::Avatar) => px(theme::SPACE_4),
            None => return div().into_any_element(),
        };
        div()
            .id("hosts-overlay")
            .absolute()
            .top(px(theme::PALETTE_OFFSET_TOP))
            .right(right_offset)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_toolbar_menu = None;
                cx.notify();
            }))
            .child(inner)
            .into_any_element()
    }

    fn toolbar_avatar_pill(&self, _cx: &mut Context<Self>) -> Div {
        h_flex()
            .ml(px(theme::SPACE_2))
            .h(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
            .pl(px(theme::SPACE_1))
            .pr(px(theme::SPACE_DENSE))
            .gap(px(theme::SPACE_2))
            .items_center()
            .rounded(px(theme::PILL_RADIUS))
            .border_2()
            .border_color(theme::accent())
            .bg(theme::library_card())
            .child(self.toolbar_avatar_button(_cx))
            .child(
                Icon::new(IconName::Plus)
                    .size(px(theme::SPACE_4))
                    .text_color(theme::text_main()),
            )
    }

    fn toolbar_avatar_button(&self, _cx: &mut Context<Self>) -> Stateful<Div> {
        let initials = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("USERNAME").ok())
            .and_then(|name| {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let mut chars = trimmed.chars();
                    let first = chars.next().map(|c| c.to_ascii_uppercase());
                    let second = trimmed
                        .split(|c: char| !c.is_alphanumeric())
                        .nth(1)
                        .and_then(|word| word.chars().next())
                        .map(|c| c.to_ascii_uppercase());
                    match (first, second) {
                        (Some(a), Some(b)) => Some(format!("{a}{b}")),
                        (Some(a), None) => Some(a.to_string()),
                        _ => None,
                    }
                }
            })
            .unwrap_or_else(|| "ME".to_string());

        div()
            .id("library-avatar")
            .size(px(theme::SPACE_6))
            .rounded(px(theme::PILL_RADIUS))
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::warning())
            .text_size(px(theme::TYPE_NANO_SIZE))
            .font_semibold()
            .text_color(gpui::white())
            .cursor_pointer()
            .child(initials)
    }

    pub(super) fn render_hosts_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let quick_connect = self.try_quick_connect_from_search(cx);
        let has_quick_connect = quick_connect.is_some();
        let _ = self.current_quick_connect_password(cx);
        let _ = self.filtered_profile_ids(cx).len();
        let selected_host_count = self.selected_host_ids.len();

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .h(px(theme::CHROME_HEIGHT))
                    .flex_none()
                    .w_full()
                    .px(px(theme::SPACE_4))
                    .pt(px(theme::SPACE_3))
                    .child(
                        h_flex()
                            .id("hosts-search-bar")
                            .w_full()
                            .h(px(theme::SHELL_NAVIGATION_ROW_HEIGHT))
                            .px(px(theme::SPACE_4))
                            .gap(px(theme::SPACE_3))
                            .items_center()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .border_1()
                            .border_color(theme::soft_border())
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_main())
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(theme::HOST_ICON_SIZE_BODY))
                                    .text_color(theme::text_muted()),
                            )
                            .child(
                                div().id("hosts-search-input-wrap").flex_1().child(
                                    Input::new(&self.shell_inputs.host_search)
                                        .appearance(false)
                                        .flex_1(),
                                ),
                            )
                            .child(
                                Button::new("library-quick-connect")
                                    .debug_selector(|| "library-quick-connect".to_string())
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        if has_quick_connect {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .disabled(!has_quick_connect)
                                    .label(localization::common_connect())
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
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h(px(theme::CHROME_HEIGHT))
                    .px(px(theme::SPACE_4))
                    .py(px(theme::SPACE_DENSE))
                    .gap(px(theme::SPACE_DENSE))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_DENSE))
                            .items_center()
                            .child(self.render_new_host_split_button(cx))
                            .child(
                                Button::new("library-agent-canvas")
                                    .debug_selector(|| "library-agent-canvas".to_string())
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::AccentSoft,
                                        cx,
                                    ))
                                    .icon(IconName::Map)
                                    .label(host_message(MessageId::HostsAgentCanvasAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_agent_canvas(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("library-new-terminal")
                                    .debug_selector(|| "library-new-terminal".to_string())
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .icon(IconName::SquareTerminal)
                                    .label(host_message(MessageId::HostsTerminalAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_local_terminal(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .items_center()
                            .child(
                                Button::new("hosts-select-visible")
                                    .debug_selector(|| "hosts-select-visible".to_string())
                                    .xsmall()
                                    .ghost()
                                    .label(host_message(MessageId::HostsSelectVisible))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_all_filtered_hosts(cx);
                                    })),
                            )
                            .when(selected_host_count > 0, |this| {
                                this.child(self.status_badge(
                                    localization::hosts_selected_count(selected_host_count),
                                    theme::with_alpha(theme::accent(), 0.16),
                                    theme::accent(),
                                ))
                                .child(
                                    Button::new("hosts-clear-selection")
                                        .debug_selector(|| "hosts-clear-selection".to_string())
                                        .xsmall()
                                        .ghost()
                                        .label(host_message(MessageId::HostsClearSelection))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.clear_host_batch_selection(cx);
                                        })),
                                )
                                .child(
                                    Button::new("hosts-bulk-diagnose")
                                        .debug_selector(|| "hosts-bulk-diagnose".to_string())
                                        .xsmall()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label(host_message(MessageId::HostsDiagnoseAction))
                                        .tooltip(host_message(MessageId::HostsDiagnoseTooltip))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.diagnose_selected_hosts(cx);
                                        })),
                                )
                                .child(
                                    Button::new("hosts-bulk-star")
                                        .debug_selector(|| "hosts-bulk-star".to_string())
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Star)
                                        .tooltip(host_message(MessageId::HostsStarSelected))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_set_selected_hosts_favorite(true, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("hosts-bulk-unstar")
                                        .debug_selector(|| "hosts-bulk-unstar".to_string())
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Star)
                                        .tooltip(host_message(MessageId::HostsUnstarSelected))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_set_selected_hosts_favorite(
                                                false, window, cx,
                                            );
                                        })),
                                )
                                .child(
                                    div().id("hosts-bulk-group-input-wrap").child(
                                        Input::new(&self.shell_inputs.bulk_group)
                                            .w(px(theme::HOST_BULK_GROUP_WIDTH)),
                                    ),
                                )
                                .child(
                                    Button::new("hosts-bulk-apply-group")
                                        .debug_selector(|| "hosts-bulk-apply-group".to_string())
                                        .xsmall()
                                        .ghost()
                                        .label(host_message(MessageId::HostsBulkGroupApply))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_assign_selected_hosts_group(window, cx);
                                        })),
                                )
                            })
                            .child(self.render_view_mode_dropdown(cx))
                            .child(self.render_tag_filter_dropdown(cx))
                            .child(self.render_sort_dropdown(cx))
                            .child(self.render_avatar_dropdown(cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .gap_0()
                    .child(
                        v_flex()
                            .id("hosts-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .gap(px(theme::SPACE_COMPACT))
                            .px(px(theme::SPACE_4))
                            .pt(px(theme::SPACE_3))
                            .pb(px(theme::ICON_SIZE_DEFAULT))
                            .track_scroll(&self.hosts_list_scroll)
                            .overflow_y_scroll()
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
                            .when_some(self.render_connection_diagnostics(cx), |this, panel| {
                                this.child(panel)
                            })
                            .when(!self.saved.profiles.is_empty(), |this| {
                                this.child(
                                    div()
                                        .pl(px(theme::SPACE_1))
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_semibold()
                                        .text_color(theme::text_main())
                                        .child(host_message(MessageId::HostsConnectionsHeading)),
                                )
                            })
                            .child(self.render_host_grid(window, cx)),
                    )
                    .when(self.show_editor_panel, |this| {
                        this.child(self.render_editor_side_panel(window, cx))
                    }),
            )
            .child(self.render_hosts_overlays(cx))
    }

    fn render_connection_diagnostics(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.connection_diagnostics.is_empty() {
            return None;
        }

        let active = self
            .connection_diagnostics
            .iter()
            .filter(|row| row.status.is_active())
            .count();
        let passed = self
            .connection_diagnostics
            .iter()
            .filter(|row| row.status == ConnectionDiagnosticStatus::Passed)
            .count();
        let attention = self
            .connection_diagnostics
            .iter()
            .filter(|row| row.status == ConnectionDiagnosticStatus::Failed)
            .count();
        let has_finished = self
            .connection_diagnostics
            .iter()
            .any(|row| !row.status.is_active());

        let mut panel = v_flex()
            .id("connection-diagnostics-panel")
            .w_full()
            .gap(px(theme::SPACE_3))
            .py(px(theme::SPACE_COMPACT))
            .border_b_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_1))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(host_message(MessageId::HostDiagnosticsTitle)),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::hosts_diagnostic_summary(
                                        active, passed, attention,
                                    )),
                            ),
                    )
                    .when(has_finished, |this| {
                        this.child(
                            Button::new("connection-diagnostics-clear")
                                .xsmall()
                                .ghost()
                                .label(host_message(MessageId::HostDiagnosticsClear))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_finished_connection_diagnostics(cx);
                                })),
                        )
                    }),
            );

        for row in self
            .connection_diagnostics
            .iter()
            .take(MAX_DIAGNOSTIC_BATCH)
        {
            let operation_id = row.operation_id;
            let profile_id = row.profile_id.clone();
            let status_color = match row.status {
                ConnectionDiagnosticStatus::Queued => theme::text_muted(),
                ConnectionDiagnosticStatus::Running => theme::accent(),
                ConnectionDiagnosticStatus::Passed => theme::success(),
                ConnectionDiagnosticStatus::Failed => theme::danger(),
                ConnectionDiagnosticStatus::Cancelled => theme::warning(),
            };
            let detail = if row.recovery.is_empty() {
                row.message.clone()
            } else {
                format!("{} - {}", row.message, row.recovery)
            };
            let elapsed = if row.elapsed.is_zero() {
                String::new()
            } else {
                format!("{:.1}s", row.elapsed.as_secs_f32())
            };
            panel = panel.child(
                h_flex()
                    .id(("connection-diagnostic-row", operation_id))
                    .w_full()
                    .min_h(px(theme::current_design_tokens()
                        .layout_host_compact_row_height()
                        .0))
                    .gap(px(theme::SPACE_COMPACT))
                    .px(px(theme::SPACE_COMPACT))
                    .py(px(theme::SPACE_3))
                    .rounded(px(theme::CONTROL_RADIUS))
                    .bg(theme::with_alpha(theme::hover(), 0.45))
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        div()
                            .size(px(theme::ICON_SIZE_INDICATOR))
                            .rounded(px(theme::PILL_RADIUS))
                            .bg(status_color)
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap(px(theme::SPACE_MICRO))
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_3))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .font_semibold()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_main())
                                            .child(row.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_NANO_SIZE))
                                            .font_semibold()
                                            .text_color(status_color)
                                            .child(row.status.label()),
                                    )
                                    .when(!elapsed.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .text_size(px(theme::TYPE_NANO_SIZE))
                                                .text_color(theme::text_muted())
                                                .child(elapsed),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_NANO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(format!(
                                        "{}  |  {}  |  {}",
                                        row.address,
                                        row.route,
                                        row.stage.label()
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(
                                        if row.status == ConnectionDiagnosticStatus::Failed {
                                            theme::danger()
                                        } else {
                                            theme::text_muted()
                                        },
                                    )
                                    .child(detail),
                            ),
                    )
                    .when(row.status.is_active(), |this| {
                        this.child(
                            Button::new(("connection-diagnostic-cancel", operation_id))
                                .xsmall()
                                .ghost()
                                .label(host_message(MessageId::HostDiagnosticCancel))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_connection_diagnostic(operation_id, cx);
                                })),
                        )
                    })
                    .when(!row.status.is_active(), |this| {
                        this.child(
                            Button::new(("connection-diagnostic-retry", operation_id))
                                .xsmall()
                                .ghost()
                                .label(host_message(MessageId::HostDiagnosticRetry))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.retry_connection_diagnostic(&profile_id, cx);
                                })),
                        )
                    }),
            );
        }

        Some(panel)
    }
}

fn host_message(id: MessageId) -> String {
    localization::message_id(id).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn host_control(
    action: HostConnectionAction,
    parent: Option<HostConnectionRowId>,
    role: HostConnectionControlRole,
    name: MessageId,
    value: Option<String>,
    selected: bool,
    disabled: bool,
    invalid: bool,
) -> HostConnectionControl {
    HostConnectionControl {
        action,
        parent,
        role,
        name,
        value,
        selected,
        disabled,
        invalid,
    }
}

fn host_row_id(profile_id: &str) -> HostConnectionRowId {
    HostConnectionRowId::host(stable_host_row_value(profile_id))
}

fn auth_mode_message(auth_mode: AuthMode) -> MessageId {
    match auth_mode {
        AuthMode::Password => MessageId::HostAuthPassword,
        AuthMode::PrivateKey => MessageId::HostAuthPrivateKey,
        AuthMode::LocalAgent => MessageId::HostAuthLocalAgent,
    }
}

fn diagnostic_status_message(status: ConnectionDiagnosticStatus) -> MessageId {
    match status {
        ConnectionDiagnosticStatus::Queued => MessageId::HostDiagnosticQueued,
        ConnectionDiagnosticStatus::Running => MessageId::HostDiagnosticRunning,
        ConnectionDiagnosticStatus::Passed => MessageId::HostDiagnosticPassed,
        ConnectionDiagnosticStatus::Failed => MessageId::HostDiagnosticFailed,
        ConnectionDiagnosticStatus::Cancelled => MessageId::HostDiagnosticCancelled,
    }
}

fn diagnostic_stage_message(stage: crate::connection_diagnostics::DiagnosticStage) -> MessageId {
    match stage {
        crate::connection_diagnostics::DiagnosticStage::Configuration => {
            MessageId::HostDiagnosticStageConfiguration
        }
        crate::connection_diagnostics::DiagnosticStage::RouteAndAuthentication => {
            MessageId::HostDiagnosticStageAuthentication
        }
        crate::connection_diagnostics::DiagnosticStage::SessionChannel => {
            MessageId::HostDiagnosticStageChannel
        }
        crate::connection_diagnostics::DiagnosticStage::Sftp => MessageId::HostDiagnosticStageSftp,
    }
}

fn host_library_state(
    all_empty: bool,
    filtered_empty: bool,
    diagnostics: &[crate::ui::app::ConnectionDiagnosticRow],
) -> HostConnectionSurfaceState {
    if diagnostics
        .iter()
        .any(|row| row.status == ConnectionDiagnosticStatus::Running)
    {
        return HostConnectionSurfaceState::DiagnosticRunning;
    }
    if diagnostics
        .iter()
        .any(|row| row.status == ConnectionDiagnosticStatus::Queued)
    {
        return HostConnectionSurfaceState::DiagnosticQueued;
    }
    if let Some(kind) = diagnostics
        .iter()
        .find(|row| row.status == ConnectionDiagnosticStatus::Failed)
        .and_then(|row| row.failure_kind)
    {
        return match kind {
            crate::connection_diagnostics::DiagnosticFailureKind::UnknownHostKey => {
                HostConnectionSurfaceState::HostKeyUnknown
            }
            crate::connection_diagnostics::DiagnosticFailureKind::HostKeyMismatch => {
                HostConnectionSurfaceState::HostKeyMismatch
            }
            crate::connection_diagnostics::DiagnosticFailureKind::CredentialDenied => {
                HostConnectionSurfaceState::AuthenticationDenied
            }
            crate::connection_diagnostics::DiagnosticFailureKind::Timeout => {
                HostConnectionSurfaceState::Timeout
            }
            crate::connection_diagnostics::DiagnosticFailureKind::RouteUnavailable => {
                HostConnectionSurfaceState::Offline
            }
            crate::connection_diagnostics::DiagnosticFailureKind::Cancelled => {
                HostConnectionSurfaceState::Cancelled
            }
            crate::connection_diagnostics::DiagnosticFailureKind::SessionChannelUnavailable
            | crate::connection_diagnostics::DiagnosticFailureKind::SftpUnavailable => {
                HostConnectionSurfaceState::Partial
            }
            crate::connection_diagnostics::DiagnosticFailureKind::Internal => {
                HostConnectionSurfaceState::Error
            }
        };
    }
    if all_empty {
        HostConnectionSurfaceState::Empty
    } else if filtered_empty {
        HostConnectionSurfaceState::FilterEmpty
    } else {
        HostConnectionSurfaceState::Ready
    }
}

fn classify_connect_failure(
    failure: &crate::ui::app::ConnectFailure,
) -> HostConnectionSurfaceState {
    classify_host_connection_error(&failure.log.join("\n"))
        .unwrap_or(HostConnectionSurfaceState::Recovery)
}

fn classify_host_connection_error(message: &str) -> Option<HostConnectionSurfaceState> {
    let message = message.trim().to_ascii_lowercase();
    if message.is_empty() {
        return None;
    }
    Some(if message.contains("host key mismatch") {
        HostConnectionSurfaceState::HostKeyMismatch
    } else if message.contains("host key is not trusted")
        || message.contains("host key not trusted")
    {
        HostConnectionSurfaceState::HostKeyUnknown
    } else if message.contains("authentication")
        && (message.contains("denied")
            || message.contains("rejected")
            || message.contains("failed"))
    {
        HostConnectionSurfaceState::AuthenticationDenied
    } else if message.contains("credential store") || message.contains("keychain") {
        HostConnectionSurfaceState::CredentialStoreUnavailable
    } else if message.contains("timed out") || message.contains("timeout") {
        HostConnectionSurfaceState::Timeout
    } else if message.contains("permission denied") || message.contains("not permitted") {
        HostConnectionSurfaceState::PermissionDenied
    } else if message.contains("address resolution")
        || message.contains("unreachable")
        || message.contains("offline")
        || message.contains("route unavailable")
    {
        HostConnectionSurfaceState::Offline
    } else if message.contains("enter a host")
        || message.contains("requires a password")
        || message.contains("invalid")
    {
        HostConnectionSurfaceState::InvalidTarget
    } else if message.contains("unavailable") {
        HostConnectionSurfaceState::Unavailable
    } else {
        HostConnectionSurfaceState::Error
    })
}
