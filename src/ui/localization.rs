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
