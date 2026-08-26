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

pub fn common_run() -> String {
    text(&CommonRunArgs::new())
}

pub fn common_save() -> String {
    text(&CommonSaveArgs::new())
}

pub fn status_connecting(host: impl Into<String>) -> String {
    text(&StatusConnectingArgs::new(UserData::new(host)))
}

macro_rules! static_message {
    ($function:ident, $arguments:ty) => {
        pub fn $function() -> String {
            text(&<$arguments>::new())
        }
    };
}

static_message!(projects_nav_label, ProjectsNavLabelArgs);
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
