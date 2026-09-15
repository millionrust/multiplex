use gpui::{App, Context, Focusable, Window};
use termirust_domain::NotificationMode;
use termirust_ui_contract::{
    MessageId, SemanticActionValue, SettingControlKind, SettingId, SettingPresentation,
    SettingValuePresentation, SettingsAccessibilityCommand, SettingsSearchDocument,
    SettingsSemanticSnapshot, SettingsSurfaceState, search_settings,
};

use super::{NavSection, TermiRustApp};
use crate::models::ThemePreset;
use crate::ui::localization;

const SETTINGS_MIN_SESSION_LOG_LIMIT: i64 = 50;
const SETTINGS_MAX_SESSION_LOG_LIMIT: i64 = 1_000;

impl TermiRustApp {
    pub(super) fn settings_semantic_snapshot(&self, cx: &App) -> Option<SettingsSemanticSnapshot> {
        if self.nav_section != NavSection::Settings {
            return None;
        }
        let query = self.settings_inputs.search.read(cx).value().to_string();
        let query_active = !query.trim().is_empty();
        let documents = SettingId::ALL
            .into_iter()
            .map(|id| SettingsSearchDocument {
                id,
                section: id.section(),
                label: localization::static_message(id.label()),
                help: localization::static_message(id.description()),
            })
            .collect::<Vec<_>>();
        let search = search_settings(&query, &documents);
        let search_results: Vec<SettingId> = search
            .as_ref()
            .map(|results| results.iter().map(|result| result.id).collect())
            .unwrap_or_default();
        let state = if search.is_err() {
            SettingsSurfaceState::Error
        } else if self.metadata_recovery_plan.is_some() {
            SettingsSurfaceState::RecoveryRequired
        } else if self.diagnostic_operation.is_some()
            || self.health_operation.is_some()
            || self.metadata_recovery_operation.is_some()
        {
            SettingsSurfaceState::Saving
        } else if !self.error_message.is_empty() {
            SettingsSurfaceState::Error
        } else if query_active && search_results.is_empty() {
            SettingsSurfaceState::SearchEmpty
        } else if query_active {
            SettingsSurfaceState::SearchResults
        } else {
            SettingsSurfaceState::Ready
        };

        Some(SettingsSemanticSnapshot {
            state,
            settings: SettingId::ALL
                .into_iter()
                .map(|id| self.setting_presentation(id, cx))
                .collect(),
            search_results,
            query_active,
        })
    }

    fn setting_presentation(&self, id: SettingId, cx: &App) -> SettingPresentation {
        use SettingValuePresentation as Value;

        let value = match id {
            SettingId::Theme => Value::Choice(match self.saved.settings.theme_preset {
                ThemePreset::Daylight => MessageId::SettingsThemeDaylight,
                _ => MessageId::SettingsThemeOcean,
            }),
            SettingId::DevelopmentLocale => Value::Choice(match localization::current_locale() {
                termirust_ui_contract::Locale::EnUs => MessageId::SettingsLocaleEnglish,
                termirust_ui_contract::Locale::EnXa => MessageId::SettingsLocaleExpanded,
                termirust_ui_contract::Locale::ArXb => MessageId::SettingsLocaleRtl,
            }),
            SettingId::TerminalFontSize => Value::Number {
                current: i64::from(self.saved.settings.terminal_font_size),
                minimum: 8,
                maximum: 32,
            },
            SettingId::CopyOnSelect => Value::Boolean(self.saved.settings.copy_on_select),
            SettingId::ConfirmMultilinePaste => {
                Value::Boolean(self.saved.settings.confirm_multiline_paste)
            }
            SettingId::RestoreWorkspaces => {
                Value::Boolean(self.saved.settings.restore_workspaces_on_launch)
            }
            SettingId::SessionHistoryLimit => Value::Number {
                current: i64::from(self.saved.settings.session_log_limit),
                minimum: SETTINGS_MIN_SESSION_LOG_LIMIT,
                maximum: SETTINGS_MAX_SESSION_LOG_LIMIT,
            },
            SettingId::AutoReconnectAttempts => Value::Number {
                current: i64::from(self.saved.settings.auto_reconnect_attempts),
                minimum: 0,
                maximum: 10,
            },
            SettingId::SshKeepalive => Value::Number {
                current: i64::from(self.saved.settings.ssh_keepalive_secs),
                minimum: 0,
                maximum: 300,
            },
            SettingId::ReconnectDelay => Value::Number {
                current: i64::from(self.saved.settings.auto_reconnect_delay_secs),
                minimum: 1,
                maximum: 120,
            },
            SettingId::Diagnostics => Value::Boolean(self.saved.settings.diagnostics_enabled),
            SettingId::NotificationMode => {
                Value::Choice(match self.activity_center.policy().mode {
                    NotificationMode::Off => MessageId::NotificationModeOff,
                    NotificationMode::InApp => MessageId::NotificationModeInApp,
                    NotificationMode::Os => MessageId::NotificationModeOs,
                })
            }
            SettingId::RecordingFriendly => {
                Value::Boolean(self.activity_center.policy().recording_friendly)
            }
            SettingId::BackupExportPassphrase => {
                if self
                    .settings_inputs
                    .export_backup_passphrase
                    .read(cx)
                    .value()
                    .is_empty()
                {
                    Value::Unavailable
                } else {
                    Value::Masked
                }
            }
            SettingId::BackupImportPassphrase => {
                if self
                    .settings_inputs
                    .import_backup_passphrase
                    .read(cx)
                    .value()
                    .is_empty()
                {
                    Value::Unavailable
                } else {
                    Value::Masked
                }
            }
            _ => Value::None,
        };
        SettingPresentation {
            id,
            value,
            disabled: false,
            invalid: false,
            destructive: matches!(id, SettingId::StorageHealth),
        }
    }

    pub(super) fn handle_settings_accessibility_command(
        &mut self,
        command: SettingsAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            SettingsAccessibilityCommand::FocusSearch => {
                self.settings_inputs
                    .search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
            }
            SettingsAccessibilityCommand::SetSearchValue => {
                if let Some(SemanticActionValue::Text(value)) = value
                    && value.chars().count() <= termirust_ui_contract::MAX_SETTINGS_QUERY_CHARS
                    && !value.contains('\0')
                {
                    Self::set_input_value(&self.settings_inputs.search, value, window, cx);
                }
            }
            SettingsAccessibilityCommand::ClearSearch => {
                Self::set_input_value(&self.settings_inputs.search, "", window, cx);
                self.settings_inputs
                    .search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
            }
            SettingsAccessibilityCommand::FocusSection(section) => {
                Self::set_input_value(
                    &self.settings_inputs.search,
                    localization::static_message(section.title()),
                    window,
                    cx,
                );
                self.settings_inputs
                    .search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
            }
            SettingsAccessibilityCommand::FocusSetting(id) => {
                self.focus_settings_input(id, window, cx);
            }
            SettingsAccessibilityCommand::ActivateSetting(id) => {
                self.activate_setting(id, window, cx);
            }
            SettingsAccessibilityCommand::SetSettingValue(id) => {
                self.set_setting_accessibility_value(id, value, window, cx);
            }
        }
    }

    fn focus_settings_input(&self, id: SettingId, window: &mut Window, cx: &mut Context<Self>) {
        let input = match id {
            SettingId::TerminalFontFamily => Some(&self.settings_inputs.terminal_font_family),
            SettingId::DefaultSshDirectory => {
                Some(&self.settings_inputs.default_ssh_startup_directory)
            }
            SettingId::LocalShellProgram => Some(&self.settings_inputs.local_shell_program),
            SettingId::LocalShellWorkingDirectory => Some(&self.settings_inputs.local_shell_cwd),
            SettingId::BackupExportPassphrase => {
                Some(&self.settings_inputs.export_backup_passphrase)
            }
            SettingId::BackupImportPassphrase => {
                Some(&self.settings_inputs.import_backup_passphrase)
            }
            SettingId::MobilePairing => Some(&self.settings_inputs.mobile_pairing_request),
            SettingId::SyncFolder => Some(&self.settings_inputs.sync_folder_input),
            _ => None,
        };
        if let Some(input) = input {
            input.read(cx).focus_handle(cx).focus(window);
        } else {
            self.settings_inputs
                .search
                .read(cx)
                .focus_handle(cx)
                .focus(window);
        }
    }

    fn activate_setting(&mut self, id: SettingId, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            SettingId::CopyOnSelect => {
                self.update_copy_on_select(!self.saved.settings.copy_on_select, cx)
            }
            SettingId::ConfirmMultilinePaste => self
                .update_confirm_multiline_paste(!self.saved.settings.confirm_multiline_paste, cx),
            SettingId::RestoreWorkspaces => self.update_restore_workspaces_on_launch(
                !self.saved.settings.restore_workspaces_on_launch,
                cx,
            ),
            SettingId::Diagnostics => {
                self.update_diagnostics_enabled(!self.saved.settings.diagnostics_enabled, cx)
            }
            SettingId::RecordingFriendly => {
                let enabled = !self.activity_center.policy().recording_friendly;
                if self.activity_center.set_recording_friendly(enabled).is_ok() {
                    self.status_message = localization::notification_settings_saved();
                    self.error_message.clear();
                } else {
                    self.error_message = localization::activity_center_operation_failed();
                }
                cx.notify();
            }
            SettingId::Onboarding => self.reset_onboarding_panel(cx),
            SettingId::StorageHealth => self.scan_store_health(cx),
            SettingId::PortableData => self.export_portable_data(cx),
            SettingId::EncryptedBackup | SettingId::MobileVault | SettingId::SharedFolderSync => {
                self.focus_settings_input(
                    match id {
                        SettingId::SharedFolderSync => SettingId::SyncFolder,
                        _ => SettingId::BackupExportPassphrase,
                    },
                    window,
                    cx,
                )
            }
            _ => {}
        }
    }

    fn set_setting_accessibility_value(
        &mut self,
        id: SettingId,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id.kind() == SettingControlKind::Toggle {
            if let Some(SemanticActionValue::Boolean(enabled)) = value {
                match id {
                    SettingId::CopyOnSelect => self.update_copy_on_select(enabled, cx),
                    SettingId::ConfirmMultilinePaste => {
                        self.update_confirm_multiline_paste(enabled, cx)
                    }
                    SettingId::RestoreWorkspaces => {
                        self.update_restore_workspaces_on_launch(enabled, cx)
                    }
                    SettingId::Diagnostics => self.update_diagnostics_enabled(enabled, cx),
                    _ => {}
                }
            }
            return;
        }
        match (id, value) {
            (SettingId::TerminalFontSize, Some(SemanticActionValue::Number(value)))
                if (8..=32).contains(&value) =>
            {
                self.update_terminal_font_size(value as u16, window, cx);
            }
            (SettingId::SessionHistoryLimit, Some(SemanticActionValue::Number(value)))
                if (SETTINGS_MIN_SESSION_LOG_LIMIT..=SETTINGS_MAX_SESSION_LOG_LIMIT)
                    .contains(&value) =>
            {
                self.update_session_log_limit(value as u16, cx);
            }
            (SettingId::AutoReconnectAttempts, Some(SemanticActionValue::Number(value)))
                if (0..=10).contains(&value) =>
            {
                self.update_auto_reconnect_attempts(value as u8, cx);
            }
            (SettingId::SshKeepalive, Some(SemanticActionValue::Number(value)))
                if (0..=300).contains(&value) =>
            {
                self.update_ssh_keepalive_secs(value as u16, cx);
            }
            (SettingId::ReconnectDelay, Some(SemanticActionValue::Number(value)))
                if (1..=120).contains(&value) =>
            {
                self.update_auto_reconnect_delay(value as u8, cx);
            }
            (SettingId::Theme, Some(SemanticActionValue::Text(value))) => match value.as_str() {
                "ocean" => self.update_theme_preset(ThemePreset::Ocean, cx),
                "daylight" => self.update_theme_preset(ThemePreset::Daylight, cx),
                _ => {}
            },
            (SettingId::DevelopmentLocale, Some(SemanticActionValue::Text(value))) => {
                if localization::set_development_locale(&value).is_ok() {
                    self.status_message =
                        localization::development_locale_active(localization::current_locale());
                    self.error_message.clear();
                    cx.notify();
                }
            }
            (SettingId::NotificationMode, Some(SemanticActionValue::Text(value))) => {
                let mode = match value.as_str() {
                    "off" => Some(NotificationMode::Off),
                    "in-app" => Some(NotificationMode::InApp),
                    "os" => Some(NotificationMode::Os),
                    _ => None,
                };
                if let Some(mode) = mode {
                    if self.activity_center.set_mode(mode).is_ok() {
                        self.status_message = localization::notification_settings_saved();
                        self.error_message.clear();
                    } else {
                        self.error_message = localization::activity_center_operation_failed();
                    }
                    cx.notify();
                }
            }
            (id, Some(SemanticActionValue::Text(value)))
                if value.chars().count()
                    <= termirust_ui_contract::MAX_SEMANTIC_ACTION_VALUE_CHARS
                    && !value.contains('\0') =>
            {
                let input = match id {
                    SettingId::TerminalFontFamily => {
                        Some(&self.settings_inputs.terminal_font_family)
                    }
                    SettingId::DefaultSshDirectory => {
                        Some(&self.settings_inputs.default_ssh_startup_directory)
                    }
                    SettingId::LocalShellProgram => Some(&self.settings_inputs.local_shell_program),
                    SettingId::LocalShellWorkingDirectory => {
                        Some(&self.settings_inputs.local_shell_cwd)
                    }
                    SettingId::BackupExportPassphrase => {
                        Some(&self.settings_inputs.export_backup_passphrase)
                    }
                    SettingId::BackupImportPassphrase => {
                        Some(&self.settings_inputs.import_backup_passphrase)
                    }
                    SettingId::MobilePairing => Some(&self.settings_inputs.mobile_pairing_request),
                    SettingId::SyncFolder => Some(&self.settings_inputs.sync_folder_input),
                    _ => None,
                };
                if let Some(input) = input {
                    Self::set_input_value(input, value, window, cx);
                }
            }
            _ => {}
        }
    }
}
