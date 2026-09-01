use std::env;
use std::sync::{OnceLock, RwLock};

use termirust_ui_contract::*;

const DEVELOPMENT_LOCALE_ENV: &str = "TERMIRUST_DEV_LOCALE";
const DEVELOPMENT_CONTROLS_ENV: &str = "TERMIRUST_ENABLE_PSEUDO_LOCALES";

fn active_localizer() -> &'static RwLock<Localizer> {
    static LOCALIZER: OnceLock<RwLock<Localizer>> = OnceLock::new();
    LOCALIZER.get_or_init(|| {
        let requested = env::var(DEVELOPMENT_LOCALE_ENV).unwrap_or_else(|_| "en-US".to_string());
        let localizer = Localizer::try_new(&requested).unwrap_or_else(|error| {
            eprintln!("[localization] embedded catalog validation failed: {error}");
            Localizer::english()
        });
        RwLock::new(localizer)
    })
}

pub fn text<M: MessageArguments>(message: &M) -> String {
    let localizer = active_localizer()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    localizer.format(message)
}

pub fn current_locale() -> Locale {
    active_localizer()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .locale()
}

pub fn message_id(id: MessageId) -> Option<String> {
    active_localizer()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .format_static(id)
        .ok()
}

pub fn static_message(id: MessageId) -> String {
    message_id(id).unwrap_or_default()
}

pub fn set_development_locale(locale: &str) -> Result<Locale, String> {
    let locale = match locale
        .trim()
        .replace('_', "-")
        .to_ascii_lowercase()
        .as_str()
    {
        "en-us" => Locale::EnUs,
        "en-xa" => Locale::EnXa,
        "ar-xb" => Locale::ArXb,
        _ => return Err("development locale must be en-US, en-XA, or ar-XB".to_string()),
    };
    let next = Localizer::try_new(locale.tag()).map_err(|error| error.to_string())?;
    *active_localizer()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    Ok(locale)
}

pub fn development_controls_enabled() -> bool {
    env::var(DEVELOPMENT_CONTROLS_ENV).as_deref() == Ok("1")
}

pub const fn development_locales() -> [Locale; 3] {
    Locale::ALL
}

pub fn development_localization_title() -> String {
    text(&DevelopmentLocalizationTitleArgs::new())
}

pub fn development_localization_hint() -> String {
    text(&DevelopmentLocalizationHintArgs::new())
}

pub fn development_locale_active(locale: Locale) -> String {
    text(&DevelopmentLocaleActiveArgs::new(KeyName::new(
        locale.tag(),
    )))
}

pub fn common_cancel() -> String {
    text(&CommonCancelArgs::new())
}

pub fn common_close() -> String {
    text(&CommonCloseArgs::new())
}

pub fn common_connect() -> String {
    text(&CommonConnectArgs::new())
}

pub fn common_delete() -> String {
    text(&CommonDeleteArgs::new())
}

pub fn common_open() -> String {
    text(&CommonOpenArgs::new())
}

pub fn common_retry() -> String {
    text(&CommonRetryArgs::new())
}

pub fn sftp_opened_local_folder(path: impl Into<String>) -> String {
    text(&SftpOpenedLocalFolderArgs::new(UserData::new(path)))
}

pub fn snippet_prompt_placeholder(name: impl Into<String>) -> String {
    text(&SnippetPromptPlaceholderArgs::new(UserData::new(name)))
}

pub fn snippet_prompts_required(count: usize) -> String {
    text(&SnippetPromptsRequiredArgs::new(Count(count as u64)))
}

pub fn snippet_insert_review_summary(count: usize, target: impl Into<String>) -> String {
    text(&SnippetInsertReviewSummaryArgs::new(
        Count(count as u64),
        UserData::new(target),
    ))
}

pub fn snippet_count(count: usize) -> String {
    text(&SnippetCountArgs::new(Count(count as u64)))
}

pub fn snippet_assigned_vault(vault: impl Into<String>) -> String {
    text(&SnippetAssignedVaultArgs::new(UserData::new(vault)))
}

pub fn snippet_loaded_status(name: impl Into<String>) -> String {
    text(&SnippetLoadedStatusArgs::new(UserData::new(name)))
}

pub fn snippet_saved_status(name: impl Into<String>) -> String {
    text(&SnippetSavedStatusArgs::new(UserData::new(name)))
}

pub fn vault_host_count(count: usize) -> String {
    text(&VaultHostCountArgs::new(Count(count as u64)))
}

pub fn vault_key_count(count: usize) -> String {
    text(&VaultKeyCountArgs::new(Count(count as u64)))
}

pub fn vault_snippet_count(count: usize) -> String {
    text(&VaultSnippetCountArgs::new(Count(count as u64)))
}

pub fn vault_member_count(count: usize) -> String {
    text(&VaultMemberCountArgs::new(Count(count as u64)))
}

pub fn vault_loaded_status(name: impl Into<String>) -> String {
    text(&VaultLoadedStatusArgs::new(UserData::new(name)))
}

pub fn vault_exists_error(name: impl Into<String>) -> String {
    text(&VaultExistsErrorArgs::new(UserData::new(name)))
}

pub fn vault_saved_status(name: impl Into<String>) -> String {
    text(&VaultSavedStatusArgs::new(UserData::new(name)))
}

pub fn vault_member_saved_status(name: impl Into<String>) -> String {
    text(&VaultMemberSavedStatusArgs::new(UserData::new(name)))
}

pub fn key_host_summary(
    username: impl Into<String>,
    endpoint: impl Into<String>,
    auth: impl Into<String>,
) -> String {
    text(&KeyHostSummaryArgs::new(
        UserData::new(username),
        UserData::new(endpoint),
        Text::new(auth),
    ))
}

pub fn sftp_host_summary(username: impl Into<String>, endpoint: impl Into<String>) -> String {
    text(&SftpHostSummaryArgs::new(
        UserData::new(username),
        UserData::new(endpoint),
    ))
}

pub fn sftp_selected_folder(path: impl Into<String>) -> String {
    text(&SftpSelectedFolderArgs::new(UserData::new(path)))
}

pub fn sftp_selected_file(path: impl Into<String>, size: impl Into<String>) -> String {
    text(&SftpSelectedFileArgs::new(
        UserData::new(path),
        Text::new(size),
    ))
}

pub fn sftp_transfer_progress(
    transferred: impl Into<String>,
    total: impl Into<String>,
    percent: u32,
) -> String {
    text(&SftpTransferProgressArgs::new(
        Text::new(transferred),
        Text::new(total),
        Count(percent as u64),
    ))
}

pub fn sftp_transfer_progress_resumed(
    transferred: impl Into<String>,
    total: impl Into<String>,
    percent: u32,
    resumed: impl Into<String>,
) -> String {
    text(&SftpTransferProgressResumedArgs::new(
        Text::new(transferred),
        Text::new(total),
        Count(percent as u64),
        Text::new(resumed),
    ))
}

pub fn sftp_conflict_description(size: impl Into<String>) -> String {
    text(&SftpConflictDescriptionArgs::new(Text::new(size)))
}

pub fn sftp_checksum(checksum: impl Into<String>) -> String {
    text(&SftpChecksumArgs::new(UserData::new(checksum)))
}

pub fn common_run() -> String {
    text(&CommonRunArgs::new())
}

pub fn hosts_open_fleet_tooltip(count: usize) -> String {
    text(&HostsOpenFleetTooltipArgs::new(Count(count as u64)))
}

pub fn hosts_provider_unavailable(provider: impl Into<String>) -> String {
    text(&HostsProviderUnavailableArgs::new(Text::new(provider)))
}

pub fn hosts_imported_count(count: usize) -> String {
    text(&HostsImportedCountArgs::new(Count(count as u64)))
}

pub fn hosts_import_error(reason: impl Into<String>) -> String {
    text(&HostsImportErrorArgs::new(UserData::new(reason)))
}

pub fn hosts_selected_count(count: usize) -> String {
    text(&HostsSelectedCountArgs::new(Count(count as u64)))
}

pub fn hosts_diagnostic_summary(active: usize, passed: usize, attention: usize) -> String {
    text(&HostsDiagnosticSummaryArgs::new(
        Count(active as u64),
        Count(passed as u64),
        Count(attention as u64),
    ))
}

pub fn host_editor_vault_label(vault: impl Into<String>) -> String {
    text(&HostEditorVaultLabelArgs::new(UserData::new(vault)))
}

macro_rules! static_message {
    ($(#[$attribute:meta])* $function:ident, $arguments:ty) => {
        $(#[$attribute])*
        pub fn $function() -> String {
            text(&<$arguments>::new())
        }
    };
}

static_message!(snippet_error_stale, SnippetErrorStaleArgs);
static_message!(
    snippet_error_terminal_required,
    SnippetErrorTerminalRequiredArgs
);
static_message!(snippet_error_stale_terminal, SnippetErrorStaleTerminalArgs);
static_message!(snippet_error_oversize, SnippetErrorOversizeArgs);
static_message!(
    snippet_multiline_review_required,
    SnippetMultilineReviewRequiredArgs
);
static_message!(snippet_inserted_as_text, SnippetInsertedAsTextArgs);
static_message!(snippet_prompts_cancelled, SnippetPromptsCancelledArgs);
static_message!(
    snippet_error_multiline_unsupported,
    SnippetErrorMultilineUnsupportedArgs
);
static_message!(snippet_insert_cancelled, SnippetInsertCancelledArgs);
static_message!(snippet_insert_review_title, SnippetInsertReviewTitleArgs);
static_message!(snippet_insert_action, SnippetInsertActionArgs);
static_message!(
    snippet_confirm_insert_action,
    SnippetConfirmInsertActionArgs
);
static_message!(snippet_cancel_insert_action, SnippetCancelInsertActionArgs);

static_message!(cli_settings_title, CliSettingsTitleArgs);
static_message!(shell_app_title, ShellAppTitleArgs);
static_message!(shell_layout_split_label, ShellLayoutSplitLabelArgs);
static_message!(shell_layout_split_tooltip, ShellLayoutSplitTooltipArgs);
static_message!(shell_layout_canvas_label, ShellLayoutCanvasLabelArgs);
static_message!(shell_layout_canvas_tooltip, ShellLayoutCanvasTooltipArgs);
static_message!(shell_tmux_missing, ShellTmuxMissingArgs);
static_message!(shell_tmux_install_guidance, ShellTmuxInstallGuidanceArgs);
static_message!(shell_tmux_install_generic, ShellTmuxInstallGenericArgs);
static_message!(shell_tmux_fallback, ShellTmuxFallbackArgs);
static_message!(
    overlay_snippet_prompts_title,
    OverlaySnippetPromptsTitleArgs
);
static_message!(overlay_paste_action, OverlayPasteActionArgs);
static_message!(palette_this_target, PaletteThisTargetArgs);
static_message!(palette_recent_command, PaletteRecentCommandArgs);
static_message!(
    #[cfg_attr(not(test), allow(dead_code))]
    palette_startup_path,
    PaletteStartupPathArgs
);
static_message!(
    #[cfg_attr(not(test), allow(dead_code))]
    palette_current_directory,
    PaletteCurrentDirectoryArgs
);
static_message!(
    #[cfg_attr(not(test), allow(dead_code))]
    palette_parent_directory,
    PaletteParentDirectoryArgs
);
static_message!(
    #[cfg_attr(not(test), allow(dead_code))]
    palette_recent_path,
    PaletteRecentPathArgs
);
static_message!(palette_git_branch, PaletteGitBranchArgs);
static_message!(palette_docker_target, PaletteDockerTargetArgs);
static_message!(palette_kubernetes_pod, PaletteKubernetesPodArgs);
static_message!(palette_systemd_unit, PaletteSystemdUnitArgs);

pub fn shell_duplicate_window_progress(target: impl Into<String>) -> String {
    text(&ShellDuplicateWindowProgressArgs::new(UserData::new(
        target,
    )))
}

pub fn shell_duplicate_window_error(reason: impl Into<String>) -> String {
    text(&ShellDuplicateWindowErrorArgs::new(UserData::new(reason)))
}

pub fn overlay_command_preview(command: impl Into<String>) -> String {
    text(&OverlayCommandPreviewArgs::new(UserData::new(command)))
}

pub fn overlay_paste_confirmation(count: usize) -> String {
    text(&OverlayPasteConfirmationArgs::new(Count(count as u64)))
}

pub fn overlay_paste_preview(preview: impl Into<String>) -> String {
    text(&OverlayPastePreviewArgs::new(UserData::new(preview)))
}

pub fn palette_history_detail(scope: impl Into<String>) -> String {
    text(&PaletteHistoryDetailArgs::new(UserData::new(scope)))
}

pub fn palette_pinned_snippet_detail(command: impl Into<String>) -> String {
    text(&PalettePinnedSnippetDetailArgs::new(UserData::new(command)))
}

pub fn palette_snippet_detail(command: impl Into<String>) -> String {
    text(&PaletteSnippetDetailArgs::new(UserData::new(command)))
}

pub fn palette_pinned_group_snippet_detail(
    group: impl Into<String>,
    command: impl Into<String>,
) -> String {
    text(&PalettePinnedGroupSnippetDetailArgs::new(
        UserData::new(group),
        UserData::new(command),
    ))
}

pub fn palette_group_snippet_detail(
    group: impl Into<String>,
    command: impl Into<String>,
) -> String {
    text(&PaletteGroupSnippetDetailArgs::new(
        UserData::new(group),
        UserData::new(command),
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn palette_files_scope(path: impl Into<String>) -> String {
    text(&PaletteFilesScopeArgs::new(UserData::new(path)))
}
static_message!(cli_settings_description, CliSettingsDescriptionArgs);
static_message!(cli_settings_state_available, CliSettingsStateAvailableArgs);
static_message!(
    cli_settings_state_host_unavailable,
    CliSettingsStateHostUnavailableArgs
);
static_message!(
    cli_settings_state_unavailable,
    CliSettingsStateUnavailableArgs
);
static_message!(
    cli_settings_protocol_unavailable,
    CliSettingsProtocolUnavailableArgs
);
static_message!(cli_settings_installed_path, CliSettingsInstalledPathArgs);
static_message!(
    cli_settings_path_unavailable,
    CliSettingsPathUnavailableArgs
);
static_message!(cli_settings_copy_path, CliSettingsCopyPathArgs);
static_message!(cli_settings_examples, CliSettingsExamplesArgs);
static_message!(cli_settings_help_hint, CliSettingsHelpHintArgs);
static_message!(cli_settings_path_copied, CliSettingsPathCopiedArgs);
static_message!(cli_settings_example_copied, CliSettingsExampleCopiedArgs);
static_message!(diagnostics_settings_title, DiagnosticsSettingsTitleArgs);
static_message!(
    diagnostics_settings_description,
    DiagnosticsSettingsDescriptionArgs
);
static_message!(diagnostics_status_disabled, DiagnosticsStatusDisabledArgs);
static_message!(diagnostics_status_healthy, DiagnosticsStatusHealthyArgs);
static_message!(diagnostics_status_dropping, DiagnosticsStatusDroppingArgs);
static_message!(
    diagnostics_status_disk_error,
    DiagnosticsStatusDiskErrorArgs
);
static_message!(diagnostics_enable_action, DiagnosticsEnableActionArgs);
static_message!(diagnostics_disable_action, DiagnosticsDisableActionArgs);
static_message!(diagnostics_clear_action, DiagnosticsClearActionArgs);
static_message!(diagnostics_preview_action, DiagnosticsPreviewActionArgs);
static_message!(diagnostics_export_action, DiagnosticsExportActionArgs);
static_message!(diagnostics_preview_required, DiagnosticsPreviewRequiredArgs);
static_message!(diagnostics_privacy_notice, DiagnosticsPrivacyNoticeArgs);
static_message!(diagnostics_clear_notice, DiagnosticsClearNoticeArgs);
static_message!(diagnostics_settings_saved, DiagnosticsSettingsSavedArgs);
static_message!(diagnostics_cleared, DiagnosticsClearedArgs);
static_message!(diagnostics_preview_ready, DiagnosticsPreviewReadyArgs);
static_message!(diagnostics_export_saved, DiagnosticsExportSavedArgs);
static_message!(diagnostics_operation_failed, DiagnosticsOperationFailedArgs);
static_message!(
    diagnostics_operation_running,
    DiagnosticsOperationRunningArgs
);
static_message!(
    diagnostics_operation_cancelled,
    DiagnosticsOperationCancelledArgs
);
static_message!(diagnostics_error_permission, DiagnosticsErrorPermissionArgs);
static_message!(diagnostics_error_size, DiagnosticsErrorSizeArgs);
static_message!(diagnostics_error_redaction, DiagnosticsErrorRedactionArgs);
static_message!(
    diagnostics_error_source_changed,
    DiagnosticsErrorSourceChangedArgs
);
static_message!(
    diagnostics_error_destination_exists,
    DiagnosticsErrorDestinationExistsArgs
);
static_message!(diagnostics_error_storage, DiagnosticsErrorStorageArgs);
static_message!(diagnostics_file_limit_label, DiagnosticsFileLimitLabelArgs);
static_message!(diagnostics_retention_label, DiagnosticsRetentionLabelArgs);
static_message!(diagnostics_preview_included, DiagnosticsPreviewIncludedArgs);
static_message!(diagnostics_preview_excluded, DiagnosticsPreviewExcludedArgs);
static_message!(health_settings_title, HealthSettingsTitleArgs);
static_message!(health_settings_description, HealthSettingsDescriptionArgs);
static_message!(health_not_scanned, HealthNotScannedArgs);
static_message!(health_scanning, HealthScanningArgs);
static_message!(health_scan_action, HealthScanActionArgs);
static_message!(health_cancel_action, HealthCancelActionArgs);
static_message!(health_project_session_label, HealthProjectSessionLabelArgs);
static_message!(health_palette_label, HealthPaletteLabelArgs);
static_message!(health_store_readable_label, HealthStoreReadableLabelArgs);
static_message!(health_store_version_label, HealthStoreVersionLabelArgs);
static_message!(health_record_hashes_label, HealthRecordHashesLabelArgs);
static_message!(health_review_findings, HealthReviewFindingsArgs);
static_message!(health_state_healthy, HealthStateHealthyArgs);
static_message!(health_state_partial, HealthStatePartialArgs);
static_message!(health_state_corrupt, HealthStateCorruptArgs);
static_message!(health_state_newer, HealthStateNewerArgs);
static_message!(health_state_permission, HealthStatePermissionArgs);
static_message!(health_state_unavailable, HealthStateUnavailableArgs);
static_message!(
    health_rebuild_project_session_action,
    HealthRebuildProjectSessionActionArgs
);
static_message!(
    health_rebuild_palette_action,
    HealthRebuildPaletteActionArgs
);
static_message!(health_unaffected_notice, HealthUnaffectedNoticeArgs);
static_message!(health_scan_complete, HealthScanCompleteArgs);
static_message!(health_repair_running, HealthRepairRunningArgs);
static_message!(health_repair_complete, HealthRepairCompleteArgs);
static_message!(health_operation_cancelled, HealthOperationCancelledArgs);
static_message!(health_error_stale, HealthErrorStaleArgs);
static_message!(health_error_newer, HealthErrorNewerArgs);
static_message!(health_error_permission, HealthErrorPermissionArgs);
static_message!(health_error_corrupt, HealthErrorCorruptArgs);
static_message!(health_error_storage, HealthErrorStorageArgs);
static_message!(recovery_title, RecoveryTitleArgs);
static_message!(recovery_description, RecoveryDescriptionArgs);
static_message!(recovery_safety_notice, RecoverySafetyNoticeArgs);
static_message!(recovery_prepare_action, RecoveryPrepareActionArgs);
static_message!(recovery_confirm_action, RecoveryConfirmActionArgs);

pub fn recovery_inspecting() -> String {
    health_scanning()
}

pub fn recovery_confirmation_required() -> String {
    health_review_findings()
}

pub fn recovery_running() -> String {
    health_repair_running()
}

pub fn recovery_complete() -> String {
    health_repair_complete()
}

pub fn recovery_cancelled() -> String {
    health_operation_cancelled()
}

pub fn recovery_no_change() -> String {
    health_state_healthy()
}

pub fn recovery_error_no_backup() -> String {
    health_error_corrupt()
}

pub fn recovery_error_verification() -> String {
    health_error_storage()
}

static_message!(remote_devices_title, RemoteDevicesTitleArgs);
static_message!(remote_devices_description, RemoteDevicesDescriptionArgs);
static_message!(remote_devices_route_label, RemoteDevicesRouteLabelArgs);
static_message!(remote_devices_route_off, RemoteDevicesRouteOffArgs);
static_message!(remote_devices_add_action, RemoteDevicesAddActionArgs);
static_message!(
    remote_devices_route_required,
    RemoteDevicesRouteRequiredArgs
);
static_message!(
    remote_devices_listener_binding,
    RemoteDevicesListenerBindingArgs
);
static_message!(
    remote_devices_listener_ready,
    RemoteDevicesListenerReadyArgs
);
static_message!(
    remote_devices_listener_interface_gone,
    RemoteDevicesListenerInterfaceGoneArgs
);
static_message!(
    remote_devices_listener_port_conflict,
    RemoteDevicesListenerPortConflictArgs
);
static_message!(
    remote_devices_listener_firewall_blocked,
    RemoteDevicesListenerFirewallBlockedArgs
);
static_message!(
    remote_devices_listener_failed,
    RemoteDevicesListenerFailedArgs
);
static_message!(
    remote_devices_listener_stopping,
    RemoteDevicesListenerStoppingArgs
);
static_message!(
    remote_devices_listener_stop_action,
    RemoteDevicesListenerStopActionArgs
);
static_message!(
    remote_devices_listener_guidance,
    RemoteDevicesListenerGuidanceArgs
);
static_message!(
    remote_devices_listener_confirm_title,
    RemoteDevicesListenerConfirmTitleArgs
);
static_message!(
    remote_devices_listener_port_help,
    RemoteDevicesListenerPortHelpArgs
);
static_message!(
    remote_devices_listener_port_placeholder,
    RemoteDevicesListenerPortPlaceholderArgs
);
static_message!(
    remote_devices_listener_enable_action,
    RemoteDevicesListenerEnableActionArgs
);
static_message!(
    remote_devices_listener_no_interface,
    RemoteDevicesListenerNoInterfaceArgs
);
static_message!(
    remote_devices_listener_use_network_action,
    RemoteDevicesListenerUseNetworkActionArgs
);
static_message!(
    remote_devices_listener_discovery_off,
    RemoteDevicesListenerDiscoveryOffArgs
);
static_message!(
    remote_devices_private_address_hidden,
    RemoteDevicesPrivateAddressHiddenArgs
);
static_message!(
    remote_devices_listener_port_invalid,
    RemoteDevicesListenerPortInvalidArgs
);
static_message!(
    remote_devices_listener_ready_notice,
    RemoteDevicesListenerReadyNoticeArgs
);
static_message!(
    remote_devices_listener_start_failed,
    RemoteDevicesListenerStartFailedArgs
);
static_message!(
    remote_devices_listener_stopped_notice,
    RemoteDevicesListenerStoppedNoticeArgs
);
static_message!(
    remote_devices_listener_stop_failed,
    RemoteDevicesListenerStopFailedArgs
);
static_message!(
    remote_devices_identity_label,
    RemoteDevicesIdentityLabelArgs
);
static_message!(
    remote_devices_identity_ready,
    RemoteDevicesIdentityReadyArgs
);
static_message!(
    remote_devices_identity_locked,
    RemoteDevicesIdentityLockedArgs
);
static_message!(remote_devices_identity_lost, RemoteDevicesIdentityLostArgs);
static_message!(
    remote_devices_permission_denied,
    RemoteDevicesPermissionDeniedArgs
);
static_message!(
    remote_devices_reset_required,
    RemoteDevicesResetRequiredArgs
);
static_message!(remote_devices_store_corrupt, RemoteDevicesStoreCorruptArgs);
static_message!(remote_devices_store_newer, RemoteDevicesStoreNewerArgs);
static_message!(remote_devices_unavailable, RemoteDevicesUnavailableArgs);
static_message!(remote_devices_copy_action, RemoteDevicesCopyActionArgs);
static_message!(
    remote_devices_fingerprint_copied,
    RemoteDevicesFingerprintCopiedArgs
);
static_message!(
    remote_devices_fingerprint_explanation,
    RemoteDevicesFingerprintExplanationArgs
);
static_message!(remote_devices_trusted_title, RemoteDevicesTrustedTitleArgs);
static_message!(remote_devices_empty, RemoteDevicesEmptyArgs);
static_message!(remote_devices_never_seen, RemoteDevicesNeverSeenArgs);
static_message!(remote_devices_status_online, RemoteDevicesStatusOnlineArgs);
static_message!(
    remote_devices_status_offline,
    RemoteDevicesStatusOfflineArgs
);
static_message!(
    remote_devices_status_revoked,
    RemoteDevicesStatusRevokedArgs
);
static_message!(
    remote_devices_allow_input_action,
    RemoteDevicesAllowInputActionArgs
);
static_message!(
    remote_devices_restrict_input_action,
    RemoteDevicesRestrictInputActionArgs
);
static_message!(remote_devices_revoke_action, RemoteDevicesRevokeActionArgs);
static_message!(
    remote_devices_name_edit_action,
    RemoteDevicesNameEditActionArgs
);
static_message!(
    remote_devices_name_placeholder,
    RemoteDevicesNamePlaceholderArgs
);
static_message!(
    remote_devices_name_save_action,
    RemoteDevicesNameSaveActionArgs
);
static_message!(remote_devices_name_saved, RemoteDevicesNameSavedArgs);
static_message!(
    remote_devices_revoked_notice,
    RemoteDevicesRevokedNoticeArgs
);
static_message!(
    remote_devices_capabilities_saved,
    RemoteDevicesCapabilitiesSavedArgs
);
static_message!(
    remote_devices_operation_failed,
    RemoteDevicesOperationFailedArgs
);
static_message!(remote_devices_reset_title, RemoteDevicesResetTitleArgs);
static_message!(
    remote_devices_reset_description,
    RemoteDevicesResetDescriptionArgs
);
static_message!(
    remote_devices_reset_placeholder,
    RemoteDevicesResetPlaceholderArgs
);
static_message!(remote_devices_reset_action, RemoteDevicesResetActionArgs);
static_message!(
    remote_devices_reset_confirmation_required,
    RemoteDevicesResetConfirmationRequiredArgs
);
static_message!(
    remote_devices_reset_complete,
    RemoteDevicesResetCompleteArgs
);
static_message!(
    remote_devices_reset_old_key_warning,
    RemoteDevicesResetOldKeyWarningArgs
);
static_message!(remote_devices_pairing_idle, RemoteDevicesPairingIdleArgs);
static_message!(
    remote_devices_pairing_generating,
    RemoteDevicesPairingGeneratingArgs
);
static_message!(
    remote_devices_pairing_waiting,
    RemoteDevicesPairingWaitingArgs
);
static_message!(
    remote_devices_pairing_sas_ready,
    RemoteDevicesPairingSasReadyArgs
);
static_message!(
    remote_devices_pairing_sas_mismatch,
    RemoteDevicesPairingSasMismatchArgs
);
static_message!(
    remote_devices_pairing_offer_help,
    RemoteDevicesPairingOfferHelpArgs
);
static_message!(
    remote_devices_pairing_offer_copy_action,
    RemoteDevicesPairingOfferCopyActionArgs
);
static_message!(
    remote_devices_pairing_offer_copied,
    RemoteDevicesPairingOfferCopiedArgs
);
static_message!(
    remote_devices_pairing_match_action,
    RemoteDevicesPairingMatchActionArgs
);
static_message!(
    remote_devices_pairing_reject_action,
    RemoteDevicesPairingRejectActionArgs
);
static_message!(
    remote_devices_pairing_expired,
    RemoteDevicesPairingExpiredArgs
);
static_message!(
    remote_devices_pairing_rate_limited,
    RemoteDevicesPairingRateLimitedArgs
);
static_message!(
    remote_devices_pairing_storage_failure,
    RemoteDevicesPairingStorageFailureArgs
);
static_message!(
    remote_devices_pairing_uncertain,
    RemoteDevicesPairingUncertainArgs
);
static_message!(
    remote_devices_pairing_paired,
    RemoteDevicesPairingPairedArgs
);
static_message!(
    remote_devices_pairing_revoked,
    RemoteDevicesPairingRevokedArgs
);

pub fn remote_devices_device_detail(
    suffix: impl Into<String>,
    status: impl Into<String>,
) -> String {
    text(&RemoteDevicesDeviceDetailArgs::new(
        UserData::new(suffix),
        Text::new(status),
    ))
}

pub fn remote_devices_last_seen(time: impl Into<String>) -> String {
    text(&RemoteDevicesLastSeenArgs::new(Text::new(time)))
}

pub fn cli_settings_schema(version: impl Into<String>) -> String {
    text(&CliSettingsSchemaArgs::new(Text::new(version)))
}

pub fn cli_settings_protocol(version: impl Into<String>) -> String {
    text(&CliSettingsProtocolArgs::new(Text::new(version)))
}

pub fn cli_settings_path_value(path: impl Into<String>) -> String {
    text(&CliSettingsPathValueArgs::new(UserData::new(path)))
}

pub fn cli_settings_example(command: impl Into<String>) -> String {
    text(&CliSettingsExampleArgs::new(KeyName::new(command)))
}

pub fn diagnostics_usage_summary(bytes: u64, files: u8, days: u8) -> String {
    text(&DiagnosticsUsageSummaryArgs::new(
        ByteSize(bytes),
        Count(u64::from(files)),
        Count(u64::from(days)),
    ))
}

pub fn recovery_impact(changed: usize, unchanged: usize, bytes: u64) -> String {
    text(&RecoveryImpactArgs::new(
        Count(changed as u64),
        Count(unchanged as u64),
        ByteSize(bytes),
    ))
}

pub fn host_recovery_impact(result: impl Into<String>, peers: usize, bytes: u64) -> String {
    text(&HostRecoveryImpactArgs::new(
        UserData::new(result),
        Count(peers as u64),
        ByteSize(bytes),
    ))
}

pub fn diagnostics_file_limit_option(count: u8) -> String {
    text(&DiagnosticsFileLimitOptionArgs::new(Count(u64::from(
        count,
    ))))
}

pub fn diagnostics_retention_option(count: u8) -> String {
    text(&DiagnosticsRetentionOptionArgs::new(Count(u64::from(
        count,
    ))))
}

pub fn diagnostics_preview_summary(entries: u64, bytes: u64, redactions: u64) -> String {
    text(&DiagnosticsPreviewSummaryArgs::new(
        Count(entries),
        ByteSize(bytes),
        Count(redactions),
    ))
}

pub fn common_save() -> String {
    text(&CommonSaveArgs::new())
}

static_message!(artifact_gallery_title, ArtifactGalleryTitleArgs);
static_message!(files_artifacts_title, FilesArtifactsTitleArgs);
static_message!(files_artifacts_description, FilesArtifactsDescriptionArgs);
static_message!(files_artifacts_session_tab, FilesArtifactsSessionTabArgs);
static_message!(files_artifacts_sftp_tab, FilesArtifactsSftpTabArgs);
static_message!(
    files_artifacts_authoritative_heading,
    FilesArtifactsAuthoritativeHeadingArgs
);
static_message!(files_artifacts_global_empty, FilesArtifactsGlobalEmptyArgs);
static_message!(files_artifacts_detail_empty, FilesArtifactsDetailEmptyArgs);
static_message!(artifact_private_row, ArtifactPrivateRowArgs);
static_message!(artifact_private_preview, ArtifactPrivatePreviewArgs);
static_message!(preset_private_row, PresetPrivateRowArgs);
static_message!(product_private_project_row, ProductPrivateProjectRowArgs);
static_message!(product_private_session_row, ProductPrivateSessionRowArgs);
static_message!(worktree_private_reference, WorktreePrivateReferenceArgs);
static_message!(worktree_private_path, WorktreePrivatePathArgs);
static_message!(artifact_gallery_description, ArtifactGalleryDescriptionArgs);
static_message!(artifact_gallery_empty, ArtifactGalleryEmptyArgs);
static_message!(artifact_gallery_loading, ArtifactGalleryLoadingArgs);
static_message!(artifact_import_action, ArtifactImportActionArgs);
static_message!(artifact_import_picker_title, ArtifactImportPickerTitleArgs);
static_message!(artifact_export_picker_title, ArtifactExportPickerTitleArgs);
static_message!(artifact_layout_list, ArtifactLayoutListArgs);
static_message!(artifact_layout_grid, ArtifactLayoutGridArgs);
static_message!(artifact_quota_unavailable, ArtifactQuotaUnavailableArgs);
static_message!(artifact_import_review_title, ArtifactImportReviewTitleArgs);
static_message!(
    artifact_import_source_preserved,
    ArtifactImportSourcePreservedArgs
);
static_message!(artifact_import_confirm, ArtifactImportConfirmArgs);
static_message!(artifact_operation_importing, ArtifactOperationImportingArgs);
static_message!(
    artifact_operation_previewing,
    ArtifactOperationPreviewingArgs
);
static_message!(artifact_operation_exporting, ArtifactOperationExportingArgs);
static_message!(
    artifact_operation_quarantining,
    ArtifactOperationQuarantiningArgs
);
static_message!(artifact_operation_restoring, ArtifactOperationRestoringArgs);
static_message!(artifact_operation_purging, ArtifactOperationPurgingArgs);
static_message!(artifact_preview_action, ArtifactPreviewActionArgs);
static_message!(artifact_export_action, ArtifactExportActionArgs);
static_message!(
    artifact_show_metadata_action,
    ArtifactShowMetadataActionArgs
);
static_message!(
    artifact_hide_metadata_action,
    ArtifactHideMetadataActionArgs
);
static_message!(artifact_quarantine_action, ArtifactQuarantineActionArgs);
static_message!(artifact_restore_action, ArtifactRestoreActionArgs);
static_message!(artifact_purge_action, ArtifactPurgeActionArgs);
static_message!(artifact_purge_warning, ArtifactPurgeWarningArgs);
static_message!(artifact_purge_confirm, ArtifactPurgeConfirmArgs);
static_message!(artifact_origin_label, ArtifactOriginLabelArgs);
static_message!(artifact_created_label, ArtifactCreatedLabelArgs);
static_message!(artifact_hash_label, ArtifactHashLabelArgs);
static_message!(artifact_origin_import, ArtifactOriginImportArgs);
static_message!(artifact_type_text, ArtifactTypeTextArgs);
static_message!(artifact_type_png, ArtifactTypePngArgs);
static_message!(artifact_type_jpeg, ArtifactTypeJpegArgs);
static_message!(artifact_type_file, ArtifactTypeFileArgs);
static_message!(artifact_state_staging, ArtifactStateStagingArgs);
static_message!(artifact_state_ready, ArtifactStateReadyArgs);
static_message!(artifact_state_quarantined, ArtifactStateQuarantinedArgs);
static_message!(artifact_state_corrupt, ArtifactStateCorruptArgs);
static_message!(artifact_preview_text, ArtifactPreviewTextArgs);
static_message!(artifact_preview_raster, ArtifactPreviewRasterArgs);
static_message!(
    artifact_preview_metadata_only,
    ArtifactPreviewMetadataOnlyArgs
);
static_message!(artifact_preview_corrupt, ArtifactPreviewCorruptArgs);
static_message!(artifact_preview_quarantined, ArtifactPreviewQuarantinedArgs);
static_message!(artifact_preview_truncated, ArtifactPreviewTruncatedArgs);
static_message!(artifact_notice_imported, ArtifactNoticeImportedArgs);
static_message!(artifact_notice_exported, ArtifactNoticeExportedArgs);
static_message!(artifact_notice_quarantined, ArtifactNoticeQuarantinedArgs);
static_message!(artifact_notice_restored, ArtifactNoticeRestoredArgs);
static_message!(artifact_notice_purged, ArtifactNoticePurgedArgs);
static_message!(artifact_error_quota, ArtifactErrorQuotaArgs);
static_message!(
    artifact_error_source_changed,
    ArtifactErrorSourceChangedArgs
);
static_message!(artifact_error_unsafe_source, ArtifactErrorUnsafeSourceArgs);
static_message!(
    artifact_error_export_conflict,
    ArtifactErrorExportConflictArgs
);
static_message!(artifact_error_corrupt, ArtifactErrorCorruptArgs);
static_message!(artifact_error_permission, ArtifactErrorPermissionArgs);
static_message!(artifact_error_storage_full, ArtifactErrorStorageFullArgs);
static_message!(artifact_error_cancelled, ArtifactErrorCancelledArgs);
static_message!(artifact_error_timeout, ArtifactErrorTimeoutArgs);
static_message!(artifact_error_decode, ArtifactErrorDecodeArgs);
static_message!(artifact_error_unavailable, ArtifactErrorUnavailableArgs);

pub fn artifact_quota_summary(used: impl Into<String>, limit: impl Into<String>) -> String {
    text(&ArtifactQuotaSummaryArgs::new(
        Text::new(used),
        Text::new(limit),
    ))
}

pub fn files_artifacts_count(count: usize) -> String {
    text(&FilesArtifactsCountArgs::new(Count(count as u64)))
}

pub fn artifact_semantic_provenance(
    kind: impl Into<String>,
    size: impl Into<String>,
    origin: impl Into<String>,
    project: impl Into<String>,
    session: impl Into<String>,
) -> String {
    text(&ArtifactSemanticProvenanceArgs::new(
        Text::new(kind),
        Text::new(size),
        Text::new(origin),
        UserData::new(project),
        UserData::new(session),
    ))
}

pub fn artifact_index_summary(
    kind: impl Into<String>,
    size: impl Into<String>,
    origin: impl Into<String>,
) -> String {
    text(&ArtifactIndexSummaryArgs::new(
        Text::new(kind),
        Text::new(size),
        Text::new(origin),
    ))
}

pub fn files_artifacts_project_session(
    project: impl Into<String>,
    session: impl Into<String>,
) -> String {
    text(&FilesArtifactsProjectSessionArgs::new(
        UserData::new(project),
        UserData::new(session),
    ))
}

pub fn artifact_import_review_file(name: impl Into<String>, size: impl Into<String>) -> String {
    text(&ArtifactImportReviewFileArgs::new(
        UserData::new(name),
        Text::new(size),
    ))
}

pub fn artifact_import_review_quota(used: impl Into<String>, limit: impl Into<String>) -> String {
    text(&ArtifactImportReviewQuotaArgs::new(
        Text::new(used),
        Text::new(limit),
    ))
}

pub fn artifact_import_progress(current: impl Into<String>, limit: impl Into<String>) -> String {
    text(&ArtifactImportProgressArgs::new(
        Text::new(current),
        Text::new(limit),
    ))
}

pub fn artifact_card_summary(
    type_label: impl Into<String>,
    size: impl Into<String>,
    state: impl Into<String>,
) -> String {
    text(&ArtifactCardSummaryArgs::new(
        Text::new(type_label),
        Text::new(size),
        Text::new(state),
    ))
}

pub fn artifact_position(position: usize, count: usize) -> String {
    text(&ArtifactPositionArgs::new(
        Count(position as u64),
        Count(count as u64),
    ))
}

pub fn artifact_preview_dimensions(width: usize, height: usize) -> String {
    text(&ArtifactPreviewDimensionsArgs::new(
        Count(width as u64),
        Count(height as u64),
    ))
}

pub fn dev_url_chip(origin: impl Into<String>) -> String {
    text(&DevUrlChipArgs::new(UserData::new(origin)))
}

pub fn dev_url_count(count: usize) -> String {
    text(&DevUrlCountArgs::new(Count(count as u64)))
}

pub fn dev_url_path(path: impl Into<String>) -> String {
    text(&DevUrlPathArgs::new(UserData::new(path)))
}

pub fn dev_url_confirm_exact(url: impl Into<String>) -> String {
    text(&DevUrlConfirmExactArgs::new(UserData::new(url)))
}

static_message!(dev_url_chip_masked, DevUrlChipMaskedArgs);
static_message!(dev_url_inspector_title, DevUrlInspectorTitleArgs);
static_message!(dev_url_empty, DevUrlEmptyArgs);
static_message!(dev_url_partial, DevUrlPartialArgs);
static_message!(dev_url_stale, DevUrlStaleArgs);
static_message!(dev_url_hidden_parameters, DevUrlHiddenParametersArgs);
static_message!(dev_url_clear_action, DevUrlClearActionArgs);
static_message!(dev_url_dismiss_action, DevUrlDismissActionArgs);
static_message!(dev_url_confirm_title, DevUrlConfirmTitleArgs);
static_message!(dev_url_confirm_warning, DevUrlConfirmWarningArgs);
static_message!(dev_url_opened, DevUrlOpenedArgs);
static_message!(dev_url_error_invalidated, DevUrlErrorInvalidatedArgs);
static_message!(dev_url_error_stale_host, DevUrlErrorStaleHostArgs);
static_message!(
    dev_url_error_session_unavailable,
    DevUrlErrorSessionUnavailableArgs
);
static_message!(
    dev_url_error_browser_unavailable,
    DevUrlErrorBrowserUnavailableArgs
);
static_message!(dev_url_error_permission, DevUrlErrorPermissionArgs);
static_message!(dev_url_error_dispatch, DevUrlErrorDispatchArgs);

pub fn status_connecting(host: impl Into<String>) -> String {
    text(&StatusConnectingArgs::new(UserData::new(host)))
}

static_message!(projects_nav_label, ProjectsNavLabelArgs);
static_message!(activity_center_nav_label, ActivityCenterNavLabelArgs);
static_message!(activity_center_title, ActivityCenterTitleArgs);
static_message!(activity_center_description, ActivityCenterDescriptionArgs);
static_message!(
    activity_center_settings_action,
    ActivityCenterSettingsActionArgs
);
static_message!(
    activity_center_store_corrupt,
    ActivityCenterStoreCorruptArgs
);
static_message!(activity_center_store_newer, ActivityCenterStoreNewerArgs);
static_message!(
    activity_center_store_permission_denied,
    ActivityCenterStorePermissionDeniedArgs
);
static_message!(
    activity_center_store_unavailable,
    ActivityCenterStoreUnavailableArgs
);
static_message!(activity_center_empty_title, ActivityCenterEmptyTitleArgs);
static_message!(
    activity_center_empty_description,
    ActivityCenterEmptyDescriptionArgs
);
static_message!(
    activity_center_dismiss_action,
    ActivityCenterDismissActionArgs
);

pub fn activity_center_position(position: usize, count: usize) -> String {
    text(&ActivityCenterPositionArgs::new(
        Count(position as u64),
        Count(count as u64),
    ))
}

static_message!(activity_center_dismissed, ActivityCenterDismissedArgs);
static_message!(activity_center_link_stale, ActivityCenterLinkStaleArgs);
static_message!(
    activity_center_operation_failed,
    ActivityCenterOperationFailedArgs
);
static_message!(notification_settings_title, NotificationSettingsTitleArgs);
static_message!(
    notification_settings_description,
    NotificationSettingsDescriptionArgs
);
static_message!(notification_mode_off, NotificationModeOffArgs);
static_message!(notification_mode_in_app, NotificationModeInAppArgs);
static_message!(notification_mode_os, NotificationModeOsArgs);
static_message!(notification_recording_title, NotificationRecordingTitleArgs);
static_message!(
    notification_recording_description,
    NotificationRecordingDescriptionArgs
);
static_message!(notification_toggle_on, NotificationToggleOnArgs);
static_message!(notification_toggle_off, NotificationToggleOffArgs);
static_message!(
    notification_permission_unknown,
    NotificationPermissionUnknownArgs
);
static_message!(
    notification_permission_granted,
    NotificationPermissionGrantedArgs
);
static_message!(
    notification_permission_denied,
    NotificationPermissionDeniedArgs
);
static_message!(
    notification_permission_unavailable,
    NotificationPermissionUnavailableArgs
);
static_message!(
    notification_permission_denied_guidance,
    NotificationPermissionDeniedGuidanceArgs
);
static_message!(notification_refresh_action, NotificationRefreshActionArgs);
static_message!(notification_reset_action, NotificationResetActionArgs);
static_message!(notification_preview, NotificationPreviewArgs);
static_message!(notification_settings_saved, NotificationSettingsSavedArgs);
static_message!(
    notification_permission_refreshed,
    NotificationPermissionRefreshedArgs
);
static_message!(notification_reset_complete, NotificationResetCompleteArgs);

pub fn notification_individual_payload(
    title: impl Into<String>,
    state: impl Into<String>,
) -> String {
    text(&NotificationIndividualPayloadArgs::new(
        UserData::new(title),
        Text::new(state),
    ))
}

pub fn notification_summary_payload(count: usize) -> String {
    text(&NotificationSummaryPayloadArgs::new(Count(count as u64)))
}

pub fn notification_permission_status(state: impl Into<String>) -> String {
    text(&NotificationPermissionStatusArgs::new(Text::new(state)))
}

static_message!(activity_age_just_now, ActivityAgeJustNowArgs);
static_message!(activity_age_yesterday, ActivityAgeYesterdayArgs);

pub fn activity_age_minutes(count: usize) -> String {
    text(&ActivityAgeMinutesArgs::new(Count(count as u64)))
}

pub fn activity_age_hours(count: usize) -> String {
    text(&ActivityAgeHoursArgs::new(Count(count as u64)))
}

pub fn activity_age_days(count: usize) -> String {
    text(&ActivityAgeDaysArgs::new(Count(count as u64)))
}

pub fn activity_age_weeks(count: usize) -> String {
    text(&ActivityAgeWeeksArgs::new(Count(count as u64)))
}

pub fn activity_age_months(count: usize) -> String {
    text(&ActivityAgeMonthsArgs::new(Count(count as u64)))
}

pub fn activity_age_years(count: usize) -> String {
    text(&ActivityAgeYearsArgs::new(Count(count as u64)))
}
static_message!(projects_title, ProjectsTitleArgs);
static_message!(projects_subtitle, ProjectsSubtitleArgs);
static_message!(
    projects_shortcut_description,
    ProjectsShortcutDescriptionArgs
);
static_message!(projects_loading, ProjectsLoadingArgs);
static_message!(projects_ready_status, ProjectsReadyStatusArgs);
static_message!(projects_add_action, ProjectsAddActionArgs);
static_message!(projects_empty_title, ProjectsEmptyTitleArgs);
static_message!(projects_empty_description, ProjectsEmptyDescriptionArgs);
static_message!(projects_folder_safety, ProjectsFolderSafetyArgs);
static_message!(projects_local_only, ProjectsLocalOnlyArgs);
static_message!(session_sidebar_title, SessionSidebarTitleArgs);
static_message!(session_sidebar_subtitle, SessionSidebarSubtitleArgs);
static_message!(session_sidebar_empty, SessionSidebarEmptyArgs);
static_message!(
    session_sidebar_select_project,
    SessionSidebarSelectProjectArgs
);
static_message!(global_palette_title, GlobalPaletteTitleArgs);
static_message!(global_palette_placeholder, GlobalPalettePlaceholderArgs);
static_message!(global_palette_searching, GlobalPaletteSearchingArgs);
static_message!(global_palette_empty, GlobalPaletteEmptyArgs);
static_message!(global_palette_empty_detail, GlobalPaletteEmptyDetailArgs);
static_message!(global_palette_no_match, GlobalPaletteNoMatchArgs);
static_message!(
    global_palette_no_match_detail,
    GlobalPaletteNoMatchDetailArgs
);
static_message!(
    global_palette_archived_fallback,
    GlobalPaletteArchivedFallbackArgs
);
static_message!(global_palette_pinned, GlobalPalettePinnedArgs);
static_message!(
    global_palette_category_attention,
    GlobalPaletteCategoryAttentionArgs
);
static_message!(
    global_palette_category_sessions,
    GlobalPaletteCategorySessionsArgs
);
static_message!(
    global_palette_category_projects,
    GlobalPaletteCategoryProjectsArgs
);
static_message!(
    global_palette_category_groups,
    GlobalPaletteCategoryGroupsArgs
);
static_message!(
    global_palette_category_presets,
    GlobalPaletteCategoryPresetsArgs
);
static_message!(
    global_palette_category_actions,
    GlobalPaletteCategoryActionsArgs
);
static_message!(
    global_palette_category_archive,
    GlobalPaletteCategoryArchiveArgs
);
static_message!(
    global_palette_category_commands,
    GlobalPaletteCategoryCommandsArgs
);
static_message!(
    global_palette_add_project_action,
    GlobalPaletteAddProjectActionArgs
);
static_message!(
    global_palette_new_session_action,
    GlobalPaletteNewSessionActionArgs
);
static_message!(
    global_palette_show_archive_action,
    GlobalPaletteShowArchiveActionArgs
);
static_message!(global_palette_query_too_long, GlobalPaletteQueryTooLongArgs);
static_message!(
    global_palette_too_many_tokens,
    GlobalPaletteTooManyTokensArgs
);
static_message!(global_palette_stale, GlobalPaletteStaleArgs);
static_message!(
    global_palette_project_required,
    GlobalPaletteProjectRequiredArgs
);
static_message!(
    global_palette_status_attention,
    GlobalPaletteStatusAttentionArgs
);
static_message!(global_palette_status_busy, GlobalPaletteStatusBusyArgs);
static_message!(global_palette_status_done, GlobalPaletteStatusDoneArgs);
static_message!(
    global_palette_status_running,
    GlobalPaletteStatusRunningArgs
);
static_message!(global_palette_status_idle, GlobalPaletteStatusIdleArgs);
static_message!(
    global_palette_status_unavailable,
    GlobalPaletteStatusUnavailableArgs
);
static_message!(
    global_palette_status_unknown,
    GlobalPaletteStatusUnknownArgs
);
static_message!(global_palette_ready, GlobalPaletteReadyArgs);

pub fn global_palette_shortcut_hint(modifier: impl Into<String>) -> String {
    text(&GlobalPaletteShortcutHintArgs::new(Text::new(modifier)))
}

pub fn global_palette_partial(count: usize) -> String {
    text(&GlobalPalettePartialArgs::new(Count(count as u64)))
}

pub fn global_palette_position(position: usize, count: usize) -> String {
    text(&GlobalPalettePositionArgs::new(
        Count(position as u64),
        Count(count as u64),
    ))
}

static_message!(group_ungrouped_label, GroupUngroupedLabelArgs);
static_message!(group_new_action, GroupNewActionArgs);
static_message!(group_editor_new_title, GroupEditorNewTitleArgs);
static_message!(group_editor_edit_title, GroupEditorEditTitleArgs);
static_message!(group_name_field, GroupNameFieldArgs);
static_message!(group_rename_action, GroupRenameActionArgs);
static_message!(group_collapse_action, GroupCollapseActionArgs);
static_message!(group_expand_action, GroupExpandActionArgs);
static_message!(group_move_up_action, GroupMoveUpActionArgs);
static_message!(group_move_down_action, GroupMoveDownActionArgs);
static_message!(group_remove_action, GroupRemoveActionArgs);
static_message!(group_remove_title, GroupRemoveTitleArgs);
static_message!(group_move_session_action, GroupMoveSessionActionArgs);
static_message!(group_move_to_root_action, GroupMoveToRootActionArgs);
static_message!(group_organization_updated, GroupOrganizationUpdatedArgs);
static_message!(group_undo_action, GroupUndoActionArgs);
static_message!(group_error_invalid_name, GroupErrorInvalidNameArgs);
static_message!(group_error_duplicate, GroupErrorDuplicateArgs);
static_message!(group_error_stale, GroupErrorStaleArgs);
static_message!(group_error_generic, GroupErrorGenericArgs);

pub fn group_session_count(count: usize) -> String {
    text(&GroupSessionCountArgs::new(Count(count as u64)))
}

pub fn group_running_summary(count: usize) -> String {
    text(&GroupRunningSummaryArgs::new(Count(count as u64)))
}

pub fn group_remove_description(name: impl Into<String>, count: usize) -> String {
    text(&GroupRemoveDescriptionArgs::new(
        UserData::new(name),
        Count(count as u64),
    ))
}

pub fn group_move_to_action(name: impl Into<String>) -> String {
    text(&GroupMoveToActionArgs::new(UserData::new(name)))
}

pub fn group_repair_status(count: usize) -> String {
    text(&GroupRepairStatusArgs::new(Count(count as u64)))
}
static_message!(project_review_title, ProjectReviewTitleArgs);
static_message!(project_validating, ProjectValidatingArgs);
static_message!(project_label_field, ProjectLabelFieldArgs);
static_message!(project_add_confirm, ProjectAddConfirmArgs);
static_message!(project_status_available, ProjectStatusAvailableArgs);
static_message!(project_status_unavailable, ProjectStatusUnavailableArgs);
static_message!(
    project_status_permission_denied,
    ProjectStatusPermissionDeniedArgs
);
static_message!(project_remove_action, ProjectRemoveActionArgs);
static_message!(project_files_stay, ProjectFilesStayArgs);
static_message!(project_undo_action, ProjectUndoActionArgs);
static_message!(project_undo_expired, ProjectUndoExpiredArgs);
static_message!(project_store_recovered, ProjectStoreRecoveredArgs);
static_message!(project_store_corrupt, ProjectStoreCorruptArgs);
static_message!(project_store_newer, ProjectStoreNewerArgs);
static_message!(project_store_unavailable, ProjectStoreUnavailableArgs);
static_message!(project_error_empty_path, ProjectErrorEmptyPathArgs);
static_message!(
    project_error_permission_denied,
    ProjectErrorPermissionDeniedArgs
);
static_message!(project_error_unavailable, ProjectErrorUnavailableArgs);
static_message!(project_error_not_directory, ProjectErrorNotDirectoryArgs);
static_message!(project_error_path_too_long, ProjectErrorPathTooLongArgs);
static_message!(project_error_invalid_label, ProjectErrorInvalidLabelArgs);
static_message!(project_error_stale, ProjectErrorStaleArgs);
static_message!(project_error_generic, ProjectErrorGenericArgs);
static_message!(presets_nav_label, PresetsNavLabelArgs);
static_message!(presets_title, PresetsTitleArgs);
static_message!(presets_subtitle, PresetsSubtitleArgs);
static_message!(presets_ready_status, PresetsReadyStatusArgs);
static_message!(presets_add_action, PresetsAddActionArgs);
static_message!(presets_scan_action, PresetsScanActionArgs);
static_message!(presets_scanning, PresetsScanningArgs);
static_message!(presets_scan_cancelled, PresetsScanCancelledArgs);
static_message!(presets_scan_partial, PresetsScanPartialArgs);
static_message!(presets_scan_none, PresetsScanNoneArgs);
static_message!(presets_detected_title, PresetsDetectedTitleArgs);
static_message!(presets_empty_title, PresetsEmptyTitleArgs);
static_message!(presets_empty_description, PresetsEmptyDescriptionArgs);
static_message!(preset_form_title_new, PresetFormTitleNewArgs);
static_message!(preset_form_title_edit, PresetFormTitleEditArgs);
static_message!(preset_label_field, PresetLabelFieldArgs);
static_message!(preset_executable_field, PresetExecutableFieldArgs);
static_message!(preset_arguments_field, PresetArgumentsFieldArgs);
static_message!(preset_argument_add, PresetArgumentAddArgs);
static_message!(preset_argument_remove, PresetArgumentRemoveArgs);
static_message!(
    preset_working_directory_field,
    PresetWorkingDirectoryFieldArgs
);
static_message!(preset_working_project_root, PresetWorkingProjectRootArgs);
static_message!(preset_working_home, PresetWorkingHomeArgs);
static_message!(preset_working_subdirectory, PresetWorkingSubdirectoryArgs);
static_message!(preset_subdirectory_field, PresetSubdirectoryFieldArgs);
static_message!(preset_permission_field, PresetPermissionFieldArgs);
static_message!(preset_permission_ask, PresetPermissionAskArgs);
static_message!(preset_permission_read_only, PresetPermissionReadOnlyArgs);
static_message!(
    preset_permission_workspace_write,
    PresetPermissionWorkspaceWriteArgs
);
static_message!(preset_enabled_field, PresetEnabledFieldArgs);
static_message!(preset_favorite_field, PresetFavoriteFieldArgs);
static_message!(preset_risk_confirm_field, PresetRiskConfirmFieldArgs);
static_message!(preset_risk_warning, PresetRiskWarningArgs);
static_message!(preset_safe_copy, PresetSafeCopyArgs);
static_message!(preset_save_action, PresetSaveActionArgs);
static_message!(preset_edit_action, PresetEditActionArgs);
static_message!(preset_delete_action, PresetDeleteActionArgs);
static_message!(preset_move_up_action, PresetMoveUpActionArgs);
static_message!(preset_move_down_action, PresetMoveDownActionArgs);
static_message!(preset_accept_action, PresetAcceptActionArgs);
static_message!(preset_status_supported, PresetStatusSupportedArgs);
static_message!(preset_status_unknown, PresetStatusUnknownArgs);
static_message!(preset_status_unsupported, PresetStatusUnsupportedArgs);
static_message!(preset_status_missing, PresetStatusMissingArgs);
static_message!(preset_status_permission, PresetStatusPermissionArgs);
static_message!(preset_status_timeout, PresetStatusTimeoutArgs);
static_message!(preset_status_failed, PresetStatusFailedArgs);
static_message!(preset_status_risky, PresetStatusRiskyArgs);
static_message!(preset_status_disabled, PresetStatusDisabledArgs);
static_message!(preset_store_recovered, PresetStoreRecoveredArgs);
static_message!(preset_store_corrupt, PresetStoreCorruptArgs);
static_message!(preset_store_newer, PresetStoreNewerArgs);
static_message!(preset_store_unavailable, PresetStoreUnavailableArgs);
static_message!(preset_error_invalid, PresetErrorInvalidArgs);
static_message!(preset_error_stale, PresetErrorStaleArgs);
static_message!(preset_error_risk_confirm, PresetErrorRiskConfirmArgs);
static_message!(runtime_label_codex, RuntimeLabelCodexArgs);
static_message!(runtime_label_claude, RuntimeLabelClaudeArgs);
static_message!(runtime_label_gemini, RuntimeLabelGeminiArgs);
static_message!(runtime_label_generic, RuntimeLabelGenericArgs);
static_message!(runtime_status_not_checked, RuntimeStatusNotCheckedArgs);
static_message!(runtime_capabilities_none, RuntimeCapabilitiesNoneArgs);
static_message!(
    runtime_capability_interactive,
    RuntimeCapabilityInteractiveArgs
);
static_message!(
    runtime_capability_structured,
    RuntimeCapabilityStructuredArgs
);
static_message!(runtime_capability_approvals, RuntimeCapabilityApprovalsArgs);
static_message!(
    runtime_capability_cancellation,
    RuntimeCapabilityCancellationArgs
);
static_message!(runtime_capability_context, RuntimeCapabilityContextArgs);
static_message!(runtime_capability_remote, RuntimeCapabilityRemoteArgs);
static_message!(runtime_capability_resume, RuntimeCapabilityResumeArgs);
static_message!(
    runtime_capability_transcript,
    RuntimeCapabilityTranscriptArgs
);
static_message!(
    runtime_inspector_runtime_label,
    RuntimeInspectorRuntimeLabelArgs
);
static_message!(
    runtime_inspector_version_label,
    RuntimeInspectorVersionLabelArgs
);
static_message!(
    runtime_inspector_ownership_label,
    RuntimeInspectorOwnershipLabelArgs
);
static_message!(
    runtime_inspector_confidence_label,
    RuntimeInspectorConfidenceLabelArgs
);
static_message!(
    runtime_inspector_generation_label,
    RuntimeInspectorGenerationLabelArgs
);
static_message!(
    runtime_inspector_capabilities_label,
    RuntimeInspectorCapabilitiesLabelArgs
);
static_message!(runtime_version_unverified, RuntimeVersionUnverifiedArgs);
static_message!(runtime_ownership_managed, RuntimeOwnershipManagedArgs);
static_message!(runtime_ownership_observed, RuntimeOwnershipObservedArgs);
static_message!(runtime_ownership_ambiguous, RuntimeOwnershipAmbiguousArgs);
static_message!(runtime_confidence_verified, RuntimeConfidenceVerifiedArgs);
static_message!(runtime_confidence_observed, RuntimeConfidenceObservedArgs);
static_message!(runtime_confidence_uncertain, RuntimeConfidenceUncertainArgs);
static_message!(runtime_stale_label, RuntimeStaleLabelArgs);
static_message!(
    runtime_unverified_explanation,
    RuntimeUnverifiedExplanationArgs
);
static_message!(transcript_export_action, TranscriptExportActionArgs);
static_message!(
    transcript_export_unavailable_unverified,
    TranscriptExportUnavailableUnverifiedArgs
);
static_message!(
    transcript_export_unavailable_contract,
    TranscriptExportUnavailableContractArgs
);
static_message!(
    transcript_export_unavailable_pending,
    TranscriptExportUnavailablePendingArgs
);
static_message!(worktree_new_action, WorktreeNewActionArgs);
static_message!(worktree_title, WorktreeTitleArgs);
static_message!(worktree_subtitle, WorktreeSubtitleArgs);
static_message!(worktree_repository_field, WorktreeRepositoryFieldArgs);
static_message!(worktree_base_field, WorktreeBaseFieldArgs);
static_message!(worktree_base_placeholder, WorktreeBasePlaceholderArgs);
static_message!(worktree_branch_field, WorktreeBranchFieldArgs);
static_message!(worktree_path_field, WorktreePathFieldArgs);
static_message!(worktree_preset_field, WorktreePresetFieldArgs);
static_message!(worktree_fetch_action, WorktreeFetchActionArgs);
static_message!(worktree_current_action, WorktreeCurrentActionArgs);
static_message!(worktree_refresh_action, WorktreeRefreshActionArgs);
static_message!(worktree_create_action, WorktreeCreateActionArgs);
static_message!(worktree_verify_action, WorktreeVerifyActionArgs);
static_message!(
    worktree_start_session_action,
    WorktreeStartSessionActionArgs
);
static_message!(worktree_stage_inspecting, WorktreeStageInspectingArgs);
static_message!(worktree_stage_ready, WorktreeStageReadyArgs);
static_message!(worktree_stage_creating, WorktreeStageCreatingArgs);
static_message!(worktree_stage_verifying, WorktreeStageVerifyingArgs);
static_message!(worktree_stage_registered, WorktreeStageRegisteredArgs);
static_message!(worktree_stage_launching, WorktreeStageLaunchingArgs);
static_message!(worktree_offline_status, WorktreeOfflineStatusArgs);
static_message!(worktree_fetched_status, WorktreeFetchedStatusArgs);
static_message!(worktree_current_warning, WorktreeCurrentWarningArgs);
static_message!(worktree_success, WorktreeSuccessArgs);
static_message!(worktree_failure_kept, WorktreeFailureKeptArgs);
static_message!(worktree_recovery_banner, WorktreeRecoveryBannerArgs);
static_message!(
    worktree_review_recovery_action,
    WorktreeReviewRecoveryActionArgs
);
static_message!(
    worktree_forget_recovery_action,
    WorktreeForgetRecoveryActionArgs
);
static_message!(
    worktree_error_invalid_repository,
    WorktreeErrorInvalidRepositoryArgs
);
static_message!(
    worktree_error_git_unavailable,
    WorktreeErrorGitUnavailableArgs
);
static_message!(worktree_error_fetch, WorktreeErrorFetchArgs);
static_message!(worktree_error_permission, WorktreeErrorPermissionArgs);
static_message!(worktree_error_storage_full, WorktreeErrorStorageFullArgs);
static_message!(
    worktree_error_invalid_reference,
    WorktreeErrorInvalidReferenceArgs
);
static_message!(
    worktree_error_resource_limit,
    WorktreeErrorResourceLimitArgs
);
static_message!(worktree_error_dirty_source, WorktreeErrorDirtySourceArgs);
static_message!(worktree_error_submodules, WorktreeErrorSubmodulesArgs);
static_message!(worktree_error_no_base, WorktreeErrorNoBaseArgs);
static_message!(worktree_error_collision, WorktreeErrorCollisionArgs);
static_message!(worktree_error_timeout, WorktreeErrorTimeoutArgs);
static_message!(worktree_error_cancelled, WorktreeErrorCancelledArgs);
static_message!(worktree_error_verification, WorktreeErrorVerificationArgs);
static_message!(worktree_error_conflict, WorktreeErrorConflictArgs);
static_message!(worktree_error_generic, WorktreeErrorGenericArgs);
static_message!(worktree_create_reason, WorktreeCreateReasonArgs);

pub fn runtime_registry_contract(version: u16) -> String {
    text(&RuntimeRegistryContractArgs::new(Count(u64::from(version))))
}

pub fn runtime_registry_executable(name: impl Into<String>) -> String {
    text(&RuntimeRegistryExecutableArgs::new(UserData::new(name)))
}

pub fn runtime_registry_capabilities(capabilities: impl Into<String>) -> String {
    text(&RuntimeRegistryCapabilitiesArgs::new(UserData::new(
        capabilities,
    )))
}

pub fn project_review_description(path: impl Into<String>) -> String {
    text(&ProjectReviewDescriptionArgs::new(UserData::new(path)))
}

pub fn project_added_status(name: impl Into<String>) -> String {
    text(&ProjectAddedStatusArgs::new(UserData::new(name)))
}

pub fn project_duplicate_status(name: impl Into<String>) -> String {
    text(&ProjectDuplicateStatusArgs::new(UserData::new(name)))
}

pub fn project_removed_status(name: impl Into<String>) -> String {
    text(&ProjectRemovedStatusArgs::new(UserData::new(name)))
}

pub fn project_restored_status(name: impl Into<String>) -> String {
    text(&ProjectRestoredStatusArgs::new(UserData::new(name)))
}

pub fn preset_saved_status(name: impl Into<String>) -> String {
    text(&PresetSavedStatusArgs::new(UserData::new(name)))
}

pub fn preset_removed_status(name: impl Into<String>) -> String {
    text(&PresetRemovedStatusArgs::new(UserData::new(name)))
}

pub fn preset_accepted_status(name: impl Into<String>) -> String {
    text(&PresetAcceptedStatusArgs::new(UserData::new(name)))
}

pub fn preset_detected_version(version: impl Into<String>) -> String {
    text(&PresetDetectedVersionArgs::new(UserData::new(version)))
}

pub fn preset_argument_count(count: usize) -> String {
    text(&PresetArgumentCountArgs::new(Count(count as u64)))
}

static_message!(new_session_action, NewSessionActionArgs);
static_message!(new_session_title, NewSessionTitleArgs);
static_message!(new_session_warning, NewSessionWarningArgs);
static_message!(new_session_legacy_warning, NewSessionLegacyWarningArgs);
static_message!(new_session_durable_copy, NewSessionDurableCopyArgs);
static_message!(new_session_project_field, NewSessionProjectFieldArgs);
static_message!(new_session_preset_field, NewSessionPresetFieldArgs);
static_message!(
    new_session_working_directory_field,
    NewSessionWorkingDirectoryFieldArgs
);
static_message!(
    new_session_initial_input_field,
    NewSessionInitialInputFieldArgs
);
static_message!(
    new_session_initial_input_hint,
    NewSessionInitialInputHintArgs
);
static_message!(
    new_session_initial_input_placeholder,
    NewSessionInitialInputPlaceholderArgs
);
static_message!(new_session_risk_warning, NewSessionRiskWarningArgs);
static_message!(new_session_cancel_start, NewSessionCancelStartArgs);
static_message!(new_session_starting_action, NewSessionStartingActionArgs);
static_message!(new_session_stop_action, NewSessionStopActionArgs);
static_message!(new_session_phase_draft, NewSessionPhaseDraftArgs);
static_message!(new_session_phase_validating, NewSessionPhaseValidatingArgs);
static_message!(new_session_phase_starting, NewSessionPhaseStartingArgs);
static_message!(
    new_session_phase_provisioning,
    NewSessionPhaseProvisioningArgs
);
static_message!(new_session_phase_attaching, NewSessionPhaseAttachingArgs);
static_message!(new_session_phase_replaying, NewSessionPhaseReplayingArgs);
static_message!(new_session_phase_live, NewSessionPhaseLiveArgs);
static_message!(
    new_session_phase_recording_paused,
    NewSessionPhaseRecordingPausedArgs
);
static_message!(new_session_phase_offline, NewSessionPhaseOfflineArgs);
static_message!(new_session_phase_orphaned, NewSessionPhaseOrphanedArgs);
static_message!(new_session_phase_gap, NewSessionPhaseGapArgs);
static_message!(
    new_session_phase_permission_denied,
    NewSessionPhasePermissionDeniedArgs
);
static_message!(
    new_session_phase_incompatible,
    NewSessionPhaseIncompatibleArgs
);
static_message!(new_session_phase_running, NewSessionPhaseRunningArgs);
static_message!(new_session_phase_failed, NewSessionPhaseFailedArgs);
static_message!(new_session_phase_cancelled, NewSessionPhaseCancelledArgs);
static_message!(new_session_phase_exited, NewSessionPhaseExitedArgs);
static_message!(new_session_status_starting, NewSessionStatusStartingArgs);
static_message!(new_session_status_stopping, NewSessionStatusStoppingArgs);
static_message!(new_session_status_ready, NewSessionStatusReadyArgs);
static_message!(
    new_session_status_ready_input,
    NewSessionStatusReadyInputArgs
);
static_message!(
    new_session_status_review_input,
    NewSessionStatusReviewInputArgs
);
static_message!(new_session_preset_required, NewSessionPresetRequiredArgs);
static_message!(new_session_review_stale, NewSessionReviewStaleArgs);
static_message!(new_session_project_missing, NewSessionProjectMissingArgs);
static_message!(new_session_preset_missing, NewSessionPresetMissingArgs);
static_message!(new_session_cancelled_clean, NewSessionCancelledCleanArgs);
static_message!(
    new_session_validation_cancelled,
    NewSessionValidationCancelledArgs
);
static_message!(new_session_terminal_error, NewSessionTerminalErrorArgs);
static_message!(
    new_session_exited_before_ready,
    NewSessionExitedBeforeReadyArgs
);
static_message!(new_session_platform_home, NewSessionPlatformHomeArgs);
static_message!(
    new_session_unavailable_value,
    NewSessionUnavailableValueArgs
);

pub fn new_session_start_error(detail: impl Into<String>) -> String {
    text(&NewSessionStartErrorArgs::new(UserData::new(detail)))
}

pub fn new_session_workspace_title(
    project: impl Into<String>,
    preset: impl Into<String>,
) -> String {
    text(&NewSessionWorkspaceTitleArgs::new(
        UserData::new(project),
        UserData::new(preset),
    ))
}

static_message!(session_library_title_field, SessionLibraryTitleFieldArgs);
static_message!(
    session_library_remove_confirm_placeholder,
    SessionLibraryRemoveConfirmPlaceholderArgs
);
static_message!(session_library_active_view, SessionLibraryActiveViewArgs);
static_message!(session_library_archive_view, SessionLibraryArchiveViewArgs);
static_message!(session_library_filter_all, SessionLibraryFilterAllArgs);
static_message!(
    session_library_filter_unread,
    SessionLibraryFilterUnreadArgs
);
static_message!(
    session_library_filter_pinned,
    SessionLibraryFilterPinnedArgs
);
static_message!(session_library_filter_empty, SessionLibraryFilterEmptyArgs);
static_message!(
    session_library_archive_empty,
    SessionLibraryArchiveEmptyArgs
);
static_message!(session_library_unread_badge, SessionLibraryUnreadBadgeArgs);
static_message!(session_library_pinned_badge, SessionLibraryPinnedBadgeArgs);
static_message!(
    session_library_rename_action,
    SessionLibraryRenameActionArgs
);
static_message!(session_library_pin_action, SessionLibraryPinActionArgs);
static_message!(session_library_unpin_action, SessionLibraryUnpinActionArgs);
static_message!(
    session_library_mark_read_action,
    SessionLibraryMarkReadActionArgs
);
static_message!(
    session_library_mark_unread_action,
    SessionLibraryMarkUnreadActionArgs
);
static_message!(
    session_library_archive_action,
    SessionLibraryArchiveActionArgs
);
static_message!(
    session_library_stop_archive_action,
    SessionLibraryStopArchiveActionArgs
);
static_message!(
    session_library_restore_action,
    SessionLibraryRestoreActionArgs
);
static_message!(
    session_library_resume_action,
    SessionLibraryResumeActionArgs
);
static_message!(
    session_library_resume_unavailable,
    SessionLibraryResumeUnavailableArgs
);
static_message!(session_resume_title, SessionResumeTitleArgs);
static_message!(session_resume_notice, SessionResumeNoticeArgs);
static_message!(session_resume_source_field, SessionResumeSourceFieldArgs);
static_message!(
    session_resume_successor_field,
    SessionResumeSuccessorFieldArgs
);
static_message!(
    session_resume_provider_field,
    SessionResumeProviderFieldArgs
);
pub fn session_resume_provider_value(version: impl Into<String>) -> String {
    text(&SessionResumeProviderValueArgs::new(KeyName::new(version)))
}
static_message!(
    session_resume_conversation_field,
    SessionResumeConversationFieldArgs
);
static_message!(
    session_resume_confirm_action,
    SessionResumeConfirmActionArgs
);
static_message!(
    session_resume_phase_validating,
    SessionResumePhaseValidatingArgs
);
static_message!(session_resume_phase_review, SessionResumePhaseReviewArgs);
static_message!(
    session_resume_phase_starting,
    SessionResumePhaseStartingArgs
);
static_message!(session_resume_phase_failed, SessionResumePhaseFailedArgs);
static_message!(session_resume_cancelled, SessionResumeCancelledArgs);
static_message!(session_resume_ready, SessionResumeReadyArgs);
pub fn session_resume_workspace_title(title: impl Into<String>) -> String {
    text(&SessionResumeWorkspaceTitleArgs::new(UserData::new(title)))
}
static_message!(
    session_resume_error_still_running,
    SessionResumeErrorStillRunningArgs
);
static_message!(
    session_resume_error_ownership,
    SessionResumeErrorOwnershipArgs
);
static_message!(session_resume_error_stale, SessionResumeErrorStaleArgs);
static_message!(
    session_resume_error_unsupported,
    SessionResumeErrorUnsupportedArgs
);
static_message!(session_resume_error_missing, SessionResumeErrorMissingArgs);
static_message!(
    session_resume_error_malformed,
    SessionResumeErrorMalformedArgs
);
static_message!(
    session_resume_error_permission,
    SessionResumeErrorPermissionArgs
);
static_message!(
    session_resume_error_provider,
    SessionResumeErrorProviderArgs
);
static_message!(session_resume_error_limit, SessionResumeErrorLimitArgs);
static_message!(
    session_resume_error_conflict,
    SessionResumeErrorConflictArgs
);
static_message!(
    session_library_remove_action,
    SessionLibraryRemoveActionArgs
);
static_message!(session_library_remove_title, SessionLibraryRemoveTitleArgs);
static_message!(
    session_library_remove_warning,
    SessionLibraryRemoveWarningArgs
);
static_message!(
    session_library_remove_metadata,
    SessionLibraryRemoveMetadataArgs
);
static_message!(
    session_library_remove_journal,
    SessionLibraryRemoveJournalArgs
);
static_message!(
    session_library_remove_transcript,
    SessionLibraryRemoveTranscriptArgs
);
static_message!(
    session_library_remove_artifacts,
    SessionLibraryRemoveArtifactsArgs
);
static_message!(session_library_remove_files, SessionLibraryRemoveFilesArgs);
static_message!(
    session_library_confirm_remove_action,
    SessionLibraryConfirmRemoveActionArgs
);
static_message!(
    session_library_inspector_title,
    SessionLibraryInspectorTitleArgs
);
static_message!(
    session_library_title_source_label,
    SessionLibraryTitleSourceLabelArgs
);
static_message!(
    session_library_activity_label,
    SessionLibraryActivityLabelArgs
);
static_message!(
    session_library_position_label,
    SessionLibraryPositionLabelArgs
);
static_message!(session_library_state_label, SessionLibraryStateLabelArgs);
static_message!(
    session_library_operation_complete,
    SessionLibraryOperationCompleteArgs
);
static_message!(
    session_library_operation_failed,
    SessionLibraryOperationFailedArgs
);
static_message!(
    session_library_stop_archive_pending,
    SessionLibraryStopArchivePendingArgs
);
static_message!(
    session_library_stop_archive_warning,
    SessionLibraryStopArchiveWarningArgs
);
static_message!(
    session_library_title_source_default,
    SessionLibraryTitleSourceDefaultArgs
);
static_message!(
    session_library_title_source_automatic,
    SessionLibraryTitleSourceAutomaticArgs
);
static_message!(
    session_library_title_source_imported,
    SessionLibraryTitleSourceImportedArgs
);
static_message!(
    session_library_title_source_manual,
    SessionLibraryTitleSourceManualArgs
);
static_message!(
    session_library_activity_unknown,
    SessionLibraryActivityUnknownArgs
);
static_message!(
    session_library_activity_idle,
    SessionLibraryActivityIdleArgs
);
static_message!(
    session_library_activity_busy,
    SessionLibraryActivityBusyArgs
);
static_message!(
    session_library_activity_needs_input,
    SessionLibraryActivityNeedsInputArgs
);
static_message!(
    session_library_activity_done,
    SessionLibraryActivityDoneArgs
);
static_message!(
    session_library_activity_failed,
    SessionLibraryActivityFailedArgs
);
static_message!(
    session_library_activity_estimated,
    SessionLibraryActivityEstimatedArgs
);
static_message!(
    session_library_recovered_last_good,
    SessionLibraryRecoveredLastGoodArgs
);
static_message!(
    session_library_store_corrupt,
    SessionLibraryStoreCorruptArgs
);
static_message!(session_library_store_newer, SessionLibraryStoreNewerArgs);
static_message!(
    session_library_store_permission,
    SessionLibraryStorePermissionArgs
);
static_message!(
    session_library_store_unavailable,
    SessionLibraryStoreUnavailableArgs
);

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static LOCALE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn localization_switch_preserves_form_values_and_focus_identity() {
        let _guard = LOCALE_TEST_LOCK.lock().unwrap();
        let form_value = String::from("user-entered.example");
        let focused_field = Some(String::from("host-address"));

        set_development_locale("en-XA").unwrap();
        assert_ne!(common_save(), "Save");
        assert_eq!(form_value, "user-entered.example");
        assert_eq!(focused_field.as_deref(), Some("host-address"));

        set_development_locale("ar-XB").unwrap();
        assert_eq!(
            current_locale().direction(),
            termirust_ui_contract::TextDirection::RightToLeft
        );
        assert_eq!(form_value, "user-entered.example");
        assert_eq!(focused_field.as_deref(), Some("host-address"));

        set_development_locale("en-US").unwrap();
        assert_eq!(common_save(), "Save");
    }

    #[test]
    fn project_review_survives_expanded_and_bidirectional_locales() {
        let _guard = LOCALE_TEST_LOCK.lock().unwrap();
        let project_name = String::from("Console workspace");
        let selected_path = String::from("/tmp/Console workspace");
        let focused_field = String::from("project-label");

        set_development_locale("en-XA").unwrap();
        let expanded = project_review_description(selected_path.clone());
        assert!(expanded.contains(&selected_path));
        assert_eq!(project_name, "Console workspace");
        assert_eq!(focused_field, "project-label");

        set_development_locale("ar-XB").unwrap();
        let bidirectional = project_review_description(selected_path.clone());
        assert!(bidirectional.contains(&selected_path));
        assert_eq!(
            current_locale().direction(),
            termirust_ui_contract::TextDirection::RightToLeft
        );
        assert_eq!(project_name, "Console workspace");
        assert_eq!(focused_field, "project-label");

        set_development_locale("en-US").unwrap();
        assert!(project_review_description(selected_path.clone()).contains(&selected_path));
    }

    #[test]
    fn localization_switch_rejects_unapproved_locale_without_mutating_active_bundle() {
        let _guard = LOCALE_TEST_LOCK.lock().unwrap();
        set_development_locale("en-US").unwrap();
        assert!(set_development_locale("fr-FR").is_err());
        assert_eq!(current_locale(), Locale::EnUs);
    }
}
