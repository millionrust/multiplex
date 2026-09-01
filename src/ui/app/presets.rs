use std::path::Path;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{
    ExecutableSpec, LaunchPreset, PermissionPolicy, PresetDraft, PresetError, PresetId,
    PresetOrigin, RuntimeDetectionStatus, WorkingDirectoryRule, classify_argument_strings,
};
use termirust_store::{PresetRepository, PresetSnapshot, StoreError, StoreHealth};
use termirust_ui_contract::{
    MessageId, PresetMoveDirection, PresetPermissionChoice, PresetRuntimeAccessibilityCommand,
    PresetRuntimeAction, PresetRuntimeControl, PresetRuntimeControlRole, PresetRuntimeRow,
    PresetRuntimeRowId, PresetRuntimeRowKind, PresetRuntimeScreen, PresetRuntimeSemanticSnapshot,
    PresetRuntimeSurfaceState, PresetWorkingDirectoryChoice, SemanticActionValue,
    stable_capability_row_value, stable_runtime_row_value,
};

use super::runtimes::{
    capability_summary, detection_status_label, executable_basename, runtime_capability_label,
    runtime_capability_message, runtime_label,
};
use super::{TermiRustApp, theme};
use crate::agents::{
    CliDiscovery, DiscoveryCancellation, RuntimeDiscoveryEntry, RuntimeDiscoveryReport,
    discovery_path_snapshot, known_runtime_descriptors,
};
use crate::storage::project_store_dir;
use crate::ui::localization;

pub(super) enum PresetLibraryLoadState {
    Loading,
    Ready,
    Failed(PresetStoreFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresetStoreFailure {
    Corrupt,
    Newer,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PresetWorkingChoice {
    #[default]
    ProjectRoot,
    PlatformHome,
    ContainedSubdirectory,
}

pub(super) struct PresetEditorState {
    pub editing_id: Option<PresetId>,
    pub enabled: bool,
    pub favorite: bool,
    pub permission_policy: PermissionPolicy,
    pub working_choice: PresetWorkingChoice,
    pub runtime: Option<String>,
    pub confirm_risky_favorite: bool,
}

pub(super) struct PresetLibraryState {
    repository: Option<PresetRepository>,
    discovery: Arc<CliDiscovery>,
    pub load_state: PresetLibraryLoadState,
    pub snapshot: Option<PresetSnapshot>,
    pub editor: Option<PresetEditorState>,
    pub scan_report: Option<RuntimeDiscoveryReport>,
    pub scan_cancel: Option<DiscoveryCancellation>,
    scan_generation: u64,
}

impl PresetLibraryState {
    pub fn open_default() -> Self {
        let mut state = Self {
            repository: None,
            discovery: Arc::new(CliDiscovery::default()),
            load_state: PresetLibraryLoadState::Loading,
            snapshot: None,
            editor: None,
            scan_report: None,
            scan_cancel: None,
            scan_generation: 0,
        };
        let repository = project_store_dir()
            .map_err(|_| PresetStoreFailure::Unavailable)
            .and_then(|root| PresetRepository::open(root).map_err(classify_store_failure));
        match repository {
            Ok(repository) => {
                state.repository = Some(repository);
                state.reload();
            }
            Err(failure) => state.load_state = PresetLibraryLoadState::Failed(failure),
        }
        state
    }

    fn reload(&mut self) {
        let Some(repository) = &self.repository else {
            self.load_state = PresetLibraryLoadState::Failed(PresetStoreFailure::Unavailable);
            self.snapshot = None;
            return;
        };
        match repository.load() {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.load_state = PresetLibraryLoadState::Ready;
            }
            Err(error) => {
                self.snapshot = None;
                self.load_state = PresetLibraryLoadState::Failed(classify_store_failure(error));
            }
        }
    }

    fn recovery_message(&self) -> Option<String> {
        match &self.load_state {
            PresetLibraryLoadState::Loading | PresetLibraryLoadState::Ready => self
                .snapshot
                .as_ref()
                .filter(|snapshot| snapshot.health == StoreHealth::RecoveredLastGood)
                .map(|_| localization::preset_store_recovered()),
            PresetLibraryLoadState::Failed(PresetStoreFailure::Corrupt) => {
                Some(localization::preset_store_corrupt())
            }
            PresetLibraryLoadState::Failed(PresetStoreFailure::Newer) => {
                Some(localization::preset_store_newer())
            }
            PresetLibraryLoadState::Failed(PresetStoreFailure::Unavailable) => {
                Some(localization::preset_store_unavailable())
            }
        }
    }
}

impl TermiRustApp {
    pub(super) fn open_new_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.preset_label_input, String::new(), window, cx);
        Self::set_input_value(&self.preset_executable_input, String::new(), window, cx);
        Self::set_input_value(&self.preset_subdirectory_input, String::new(), window, cx);
        self.preset_argument_inputs = vec![new_argument_input(window, cx)];
        self.preset_library.editor = Some(PresetEditorState {
            editing_id: None,
            enabled: true,
            favorite: false,
            permission_policy: PermissionPolicy::AskAsNeeded,
            working_choice: PresetWorkingChoice::ProjectRoot,
            runtime: None,
            confirm_risky_favorite: false,
        });
        self.preset_label_input
            .update(cx, |input, cx| input.focus(window, cx));
        self.error_message.clear();
        cx.notify();
    }

    fn edit_preset(&mut self, id: PresetId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self
            .preset_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.presets.iter().find(|preset| preset.id == id))
            .cloned()
        else {
            return;
        };
        Self::set_input_value(
            &self.preset_label_input,
            preset.label.as_str().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.preset_executable_input,
            preset.executable.as_str().to_string(),
            window,
            cx,
        );
        let (working_choice, subdirectory) = match &preset.working_directory {
            WorkingDirectoryRule::ProjectRoot => (PresetWorkingChoice::ProjectRoot, String::new()),
            WorkingDirectoryRule::PlatformHome => {
                (PresetWorkingChoice::PlatformHome, String::new())
            }
            WorkingDirectoryRule::ContainedSubdirectory(value) => {
                (PresetWorkingChoice::ContainedSubdirectory, value.clone())
            }
        };
        Self::set_input_value(&self.preset_subdirectory_input, subdirectory, window, cx);
        self.preset_argument_inputs = if preset.args.is_empty() {
            vec![new_argument_input(window, cx)]
        } else {
            preset
                .args
                .iter()
                .map(|argument| {
                    let input = new_argument_input(window, cx);
                    Self::set_input_value(&input, argument.as_str().to_string(), window, cx);
                    input
                })
                .collect()
        };
        self.preset_library.editor = Some(PresetEditorState {
            editing_id: Some(id),
            enabled: preset.enabled,
            favorite: preset.favorite,
            permission_policy: preset.permission_policy,
            working_choice,
            runtime: preset
                .runtime
                .as_ref()
                .map(|runtime| runtime.as_str().to_string()),
            confirm_risky_favorite: preset.risk.is_risky(),
        });
        self.preset_label_input
            .update(cx, |input, cx| input.focus(window, cx));
        self.error_message.clear();
        cx.notify();
    }

    fn cancel_preset_editor(&mut self, cx: &mut Context<Self>) {
        self.preset_library.editor = None;
        self.error_message.clear();
        cx.notify();
    }

    fn add_preset_argument(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preset_argument_inputs.len() >= termirust_domain::MAX_ARGUMENTS {
            self.error_message = localization::preset_error_invalid();
            cx.notify();
            return;
        }
        let input = new_argument_input(window, cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        self.preset_argument_inputs.push(input);
        cx.notify();
    }

    fn remove_preset_argument(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.preset_argument_inputs.len() {
            self.preset_argument_inputs.remove(index);
        }
        if self.preset_argument_inputs.is_empty()
            && let Some(editor) = self.preset_library.editor.as_mut()
        {
            editor.confirm_risky_favorite = false;
        }
        cx.notify();
    }

    fn preset_draft(&self, cx: &Context<Self>) -> Option<PresetDraft> {
        let editor = self.preset_library.editor.as_ref()?;
        let args = self
            .preset_argument_inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect::<Vec<_>>();
        let working_directory = match editor.working_choice {
            PresetWorkingChoice::ProjectRoot => WorkingDirectoryRule::ProjectRoot,
            PresetWorkingChoice::PlatformHome => WorkingDirectoryRule::PlatformHome,
            PresetWorkingChoice::ContainedSubdirectory => {
                WorkingDirectoryRule::ContainedSubdirectory(
                    self.preset_subdirectory_input.read(cx).value().to_string(),
                )
            }
        };
        Some(PresetDraft {
            id: editor.editing_id.unwrap_or_else(PresetId::new),
            label: self.preset_label_input.read(cx).value().to_string(),
            executable: self.preset_executable_input.read(cx).value().to_string(),
            args,
            working_directory,
            runtime: editor.runtime.clone(),
            enabled: editor.enabled,
            favorite: editor.favorite,
            permission_policy: editor.permission_policy,
            origin: PresetOrigin::User,
            confirm_risky_favorite: editor.confirm_risky_favorite,
        })
    }

    pub(super) fn save_preset_editor(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.preset_draft(cx) else {
            return;
        };
        let name = draft.label.trim().to_string();
        let Some(repository) = self.preset_library.repository.as_ref() else {
            self.error_message = localization::preset_store_unavailable();
            cx.notify();
            return;
        };
        let expected = self
            .preset_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
            .unwrap_or(termirust_domain::Revision::ZERO);
        match repository.save_preset(draft, expected) {
            Ok(_) => {
                self.preset_library.editor = None;
                self.preset_library.reload();
                self.status_message = localization::preset_saved_status(name);
                self.error_message.clear();
            }
            Err(error) => {
                if matches!(
                    error,
                    StoreError::PresetDomain(PresetError::StaleRevision { .. })
                ) {
                    self.preset_library.reload();
                }
                self.error_message = preset_store_error_message(&error);
            }
        }
        cx.notify();
    }

    fn delete_preset(&mut self, id: PresetId, cx: &mut Context<Self>) {
        let Some(repository) = self.preset_library.repository.as_ref() else {
            return;
        };
        let Some(snapshot) = self.preset_library.snapshot.as_ref() else {
            return;
        };
        let name = snapshot
            .presets
            .iter()
            .find(|preset| preset.id == id)
            .map(|preset| preset.label.as_str().to_string())
            .unwrap_or_default();
        match repository.remove_preset(id, snapshot.revision) {
            Ok(_) => {
                self.preset_library.reload();
                self.status_message = localization::preset_removed_status(name);
                self.error_message.clear();
            }
            Err(error) => self.error_message = preset_store_error_message(&error),
        }
        cx.notify();
    }

    fn update_preset_flags(
        &mut self,
        id: PresetId,
        enabled: Option<bool>,
        favorite: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.preset_library.snapshot.as_ref() else {
            return;
        };
        let Some(preset) = snapshot.presets.iter().find(|preset| preset.id == id) else {
            return;
        };
        if favorite == Some(true) && preset.risk.is_risky() {
            self.error_message = localization::preset_error_risk_confirm();
            cx.notify();
            return;
        }
        let mut draft = preset.to_draft();
        if let Some(enabled) = enabled {
            draft.enabled = enabled;
        }
        if let Some(favorite) = favorite {
            draft.favorite = favorite;
            draft.confirm_risky_favorite = false;
        }
        let Some(repository) = self.preset_library.repository.as_ref() else {
            return;
        };
        match repository.save_preset(draft, snapshot.revision) {
            Ok(_) => {
                self.preset_library.reload();
                self.error_message.clear();
            }
            Err(error) => self.error_message = preset_store_error_message(&error),
        }
        cx.notify();
    }

    fn move_preset(&mut self, id: PresetId, direction: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = self.preset_library.snapshot.as_ref() else {
            return;
        };
        let Some(index) = snapshot.presets.iter().position(|preset| preset.id == id) else {
            return;
        };
        let target = index as isize + direction;
        if target < 0 || target >= snapshot.presets.len() as isize {
            return;
        }
        let before = if direction < 0 {
            Some(snapshot.presets[target as usize].id)
        } else {
            snapshot.presets.get(index + 2).map(|preset| preset.id)
        };
        let Some(repository) = self.preset_library.repository.as_ref() else {
            return;
        };
        match repository.move_preset_before(id, before, snapshot.revision) {
            Ok(_) => {
                self.preset_library.reload();
                self.error_message.clear();
            }
            Err(error) => self.error_message = preset_store_error_message(&error),
        }
        cx.notify();
    }

    pub(super) fn start_preset_scan(
        &mut self,
        refresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(cancel) = self.preset_library.scan_cancel.take() {
            cancel.cancel();
        }
        self.preset_library.scan_generation = self.preset_library.scan_generation.wrapping_add(1);
        let generation = self.preset_library.scan_generation;
        let cancel = DiscoveryCancellation::default();
        self.preset_library.scan_cancel = Some(cancel.clone());
        let discovery = self.preset_library.discovery.clone();
        let path = discovery_path_snapshot();
        self.status_message = localization::presets_scanning();
        self.error_message.clear();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move {
                    discovery.discover(&known_runtime_descriptors(), &path, &cancel, refresh)
                })
                .await;
            let _ = cx.update(|_, cx| {
                let _ = this.update(cx, |app, cx| {
                    if app.preset_library.scan_generation != generation {
                        return;
                    }
                    app.preset_library.scan_cancel = None;
                    app.status_message = if report.cancelled {
                        localization::presets_scan_cancelled()
                    } else if report.partial {
                        localization::presets_scan_partial()
                    } else if report.entries.iter().all(|entry| {
                        entry.result.status != RuntimeDetectionStatus::Available
                            || entry.result.capabilities.is_empty()
                    }) {
                        localization::presets_scan_none()
                    } else {
                        localization::presets_ready_status()
                    };
                    app.preset_library.scan_report = Some(report);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn cancel_preset_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.preset_library.scan_cancel {
            cancel.cancel();
            self.status_message = localization::presets_scan_cancelled();
        }
        cx.notify();
    }

    fn accept_detected_preset(&mut self, candidate: RuntimeDiscoveryEntry, cx: &mut Context<Self>) {
        if candidate.result.status != RuntimeDetectionStatus::Available
            || candidate.result.capabilities.is_empty()
        {
            return;
        }
        let Some(executable) = candidate.executable.as_ref() else {
            return;
        };
        let label = runtime_label(candidate.result.runtime_id.as_str());
        let draft = PresetDraft {
            id: PresetId::new(),
            label: label.clone(),
            executable: executable.as_str().to_string(),
            args: Vec::new(),
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: Some(candidate.result.runtime_id.as_str().to_string()),
            enabled: true,
            favorite: false,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::Detected,
            confirm_risky_favorite: false,
        };
        let Some(snapshot) = self.preset_library.snapshot.as_ref() else {
            return;
        };
        let Some(repository) = self.preset_library.repository.as_ref() else {
            return;
        };
        match repository.save_preset(draft, snapshot.revision) {
            Ok(_) => {
                self.preset_library.reload();
                self.status_message = localization::preset_accepted_status(label);
                self.error_message.clear();
            }
            Err(error) => self.error_message = preset_store_error_message(&error),
        }
        cx.notify();
    }

    pub(super) fn retry_preset_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preset_library.reload();
        if matches!(
            self.preset_library.load_state,
            PresetLibraryLoadState::Ready
        ) {
            self.status_message = localization::presets_ready_status();
            self.error_message.clear();
            self.preset_list_focus.focus(window);
        } else {
            self.error_message = self
                .preset_library
                .recovery_message()
                .unwrap_or_else(localization::preset_store_unavailable);
        }
        cx.notify();
    }

    pub(super) fn preset_runtime_semantic_snapshot(
        &self,
        cx: &Context<Self>,
    ) -> PresetRuntimeSemanticSnapshot {
        let snapshot = self.preset_library.snapshot.as_ref();
        let presets = snapshot
            .map(|snapshot| snapshot.presets.as_slice())
            .unwrap_or_default();
        let read_only = snapshot.is_some_and(|snapshot| snapshot.read_only);
        let runtime_count = self
            .preset_library
            .scan_report
            .as_ref()
            .map(|report| report.entries.len())
            .unwrap_or_else(|| known_runtime_descriptors().len());
        let top_level_count = (presets.len() + runtime_count).max(1);
        let editing = self
            .preset_library
            .editor
            .as_ref()
            .and_then(|editor| editor.editing_id);
        let mut rows = presets
            .iter()
            .enumerate()
            .map(|(index, preset)| {
                let available = executable_available(&preset.executable);
                PresetRuntimeRow {
                    id: accessible_preset_id(preset.id),
                    parent: None,
                    name: preset.label.as_str().to_string(),
                    status: preset_status_message_id(preset, available),
                    detail: Some(format!(
                        "{}; {}",
                        executable_display(&preset.executable),
                        localization::preset_argument_count(preset.args.len())
                    )),
                    selected: editing == Some(preset.id),
                    disabled: read_only,
                    checked: Some(preset.enabled),
                    risky: preset.risk.is_risky(),
                    stale: false,
                    position: index + 1,
                    set_size: top_level_count,
                }
            })
            .collect::<Vec<_>>();

        if let Some(report) = self.preset_library.scan_report.as_ref() {
            for (index, candidate) in report.entries.iter().enumerate() {
                append_runtime_semantic_rows(
                    &mut rows,
                    candidate,
                    presets.len() + index + 1,
                    top_level_count,
                );
            }
        } else {
            for (index, descriptor) in known_runtime_descriptors().into_iter().enumerate() {
                rows.push(PresetRuntimeRow {
                    id: runtime_row_id(descriptor.id.as_str()),
                    parent: None,
                    name: runtime_label(descriptor.id.as_str()),
                    status: MessageId::RuntimeStatusNotChecked,
                    detail: Some(localization::runtime_registry_contract(
                        descriptor.descriptor_version,
                    )),
                    selected: false,
                    disabled: false,
                    checked: None,
                    risky: false,
                    stale: false,
                    position: presets.len() + index + 1,
                    set_size: top_level_count,
                });
            }
        }

        PresetRuntimeSemanticSnapshot {
            screen: PresetRuntimeScreen::PresetsAndRuntimes,
            state: self.preset_runtime_surface_state(cx),
            controls: self.preset_runtime_controls(cx),
            rows,
            recording_friendly: self.activity_center.policy().recording_friendly,
        }
    }

    fn preset_runtime_surface_state(&self, cx: &Context<Self>) -> PresetRuntimeSurfaceState {
        match self.preset_library.load_state {
            PresetLibraryLoadState::Loading => return PresetRuntimeSurfaceState::Loading,
            PresetLibraryLoadState::Failed(PresetStoreFailure::Corrupt) => {
                return PresetRuntimeSurfaceState::Corrupt;
            }
            PresetLibraryLoadState::Failed(PresetStoreFailure::Newer) => {
                return PresetRuntimeSurfaceState::NewerFormat;
            }
            PresetLibraryLoadState::Failed(PresetStoreFailure::Unavailable) => {
                return PresetRuntimeSurfaceState::Unavailable;
            }
            PresetLibraryLoadState::Ready => {}
        }
        if self.preset_library.scan_cancel.is_some() {
            return PresetRuntimeSurfaceState::Scanning;
        }
        if let Some(editor) = self.preset_library.editor.as_ref() {
            let args = self
                .preset_argument_inputs
                .iter()
                .map(|input| input.read(cx).value().to_string())
                .collect::<Vec<_>>();
            if classify_argument_strings(editor.runtime.as_deref(), &args).is_risky() {
                return PresetRuntimeSurfaceState::RiskReview;
            }
        }
        if let Some(report) = self.preset_library.scan_report.as_ref() {
            if report.cancelled {
                return PresetRuntimeSurfaceState::Cancelled;
            }
            if report
                .entries
                .iter()
                .any(|entry| entry.result.status == RuntimeDetectionStatus::PermissionDenied)
            {
                return PresetRuntimeSurfaceState::PermissionDenied;
            }
            if report
                .entries
                .iter()
                .any(|entry| entry.result.diagnostic_code.as_deref() == Some("timeout"))
            {
                return PresetRuntimeSurfaceState::Timeout;
            }
            if report
                .entries
                .iter()
                .any(|entry| entry.result.diagnostic_code.as_deref() == Some("malformed-version"))
            {
                return PresetRuntimeSurfaceState::Malformed;
            }
            if report.partial {
                return PresetRuntimeSurfaceState::Partial;
            }
            if !report.entries.is_empty()
                && report.entries.iter().all(|entry| {
                    entry.result.status != RuntimeDetectionStatus::Available
                        || entry.result.capabilities.is_empty()
                })
            {
                return PresetRuntimeSurfaceState::Unsupported;
            }
        }
        if self
            .preset_library
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.health == StoreHealth::RecoveredLastGood)
        {
            return PresetRuntimeSurfaceState::Recovery;
        }
        if self
            .preset_library
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.presets.is_empty())
        {
            PresetRuntimeSurfaceState::Empty
        } else {
            PresetRuntimeSurfaceState::Ready
        }
    }

    fn preset_runtime_controls(&self, cx: &Context<Self>) -> Vec<PresetRuntimeControl> {
        if matches!(
            self.preset_library.load_state,
            PresetLibraryLoadState::Failed(_)
        ) {
            return vec![preset_runtime_button(
                PresetRuntimeAction::RetryStore,
                MessageId::CommonRetry,
                None,
            )];
        }

        let snapshot = self.preset_library.snapshot.as_ref();
        let read_only = snapshot.is_some_and(|snapshot| snapshot.read_only);
        let scanning = self.preset_library.scan_cancel.is_some();
        let mut controls = vec![preset_runtime_button(
            if scanning {
                PresetRuntimeAction::CancelScan
            } else {
                PresetRuntimeAction::StartScan
            },
            if scanning {
                MessageId::CommonCancel
            } else {
                MessageId::PresetsScanAction
            },
            None,
        )];
        controls.push(preset_runtime_button(
            PresetRuntimeAction::AddPreset,
            MessageId::PresetsAddAction,
            None,
        ));
        controls.last_mut().expect("add control exists").disabled = read_only;

        if let Some(report) = self.preset_library.scan_report.as_ref() {
            for candidate in &report.entries {
                let row = runtime_row_id(candidate.result.runtime_id.as_str());
                let mut control = preset_runtime_button(
                    PresetRuntimeAction::AcceptRuntime(row),
                    MessageId::PresetAcceptAction,
                    Some(row),
                );
                control.disabled = scanning
                    || read_only
                    || candidate.result.status != RuntimeDetectionStatus::Available
                    || candidate.result.capabilities.is_empty()
                    || candidate.executable.is_none()
                    || self.preset_exists_for(candidate);
                controls.push(control);
            }
        }

        if let Some(snapshot) = snapshot {
            for (index, preset) in snapshot.presets.iter().enumerate() {
                let row = accessible_preset_id(preset.id);
                let actions = [
                    (
                        PresetRuntimeAction::MovePreset(row, PresetMoveDirection::Up),
                        MessageId::PresetMoveUpAction,
                        index == 0,
                    ),
                    (
                        PresetRuntimeAction::MovePreset(row, PresetMoveDirection::Down),
                        MessageId::PresetMoveDownAction,
                        index + 1 == snapshot.presets.len(),
                    ),
                    (
                        PresetRuntimeAction::TogglePresetEnabled(row),
                        MessageId::PresetEnabledField,
                        false,
                    ),
                    (
                        PresetRuntimeAction::TogglePresetFavorite(row),
                        MessageId::PresetFavoriteField,
                        preset.risk.is_risky() && !preset.favorite,
                    ),
                    (
                        PresetRuntimeAction::EditPreset(row),
                        MessageId::PresetEditAction,
                        false,
                    ),
                    (
                        PresetRuntimeAction::DeletePreset(row),
                        MessageId::PresetDeleteAction,
                        false,
                    ),
                ];
                for (action, name, unavailable) in actions {
                    let mut control = preset_runtime_button(action, name, Some(row));
                    control.disabled = read_only || unavailable;
                    control.selected = match action {
                        PresetRuntimeAction::TogglePresetEnabled(_) => preset.enabled,
                        PresetRuntimeAction::TogglePresetFavorite(_) => preset.favorite,
                        _ => false,
                    };
                    controls.push(control);
                }
            }
        }

        if let Some(editor) = self.preset_library.editor.as_ref() {
            controls.extend(self.preset_editor_semantic_controls(editor, cx));
        }
        controls
    }

    fn preset_editor_semantic_controls(
        &self,
        editor: &PresetEditorState,
        cx: &Context<Self>,
    ) -> Vec<PresetRuntimeControl> {
        let mut controls = vec![
            preset_runtime_text_field(
                PresetRuntimeAction::SetPresetLabel,
                MessageId::PresetLabelField,
                self.preset_label_input.read(cx).value().to_string(),
            ),
            preset_runtime_text_field(
                PresetRuntimeAction::SetPresetExecutable,
                MessageId::PresetExecutableField,
                self.preset_executable_input.read(cx).value().to_string(),
            ),
        ];
        for (index, input) in self.preset_argument_inputs.iter().enumerate() {
            controls.push(preset_runtime_text_field(
                PresetRuntimeAction::SetPresetArgument(index),
                MessageId::PresetArgumentsField,
                input.read(cx).value().to_string(),
            ));
            controls.push(preset_runtime_button(
                PresetRuntimeAction::RemovePresetArgument(index),
                MessageId::PresetArgumentRemove,
                None,
            ));
        }
        let mut add_argument = preset_runtime_button(
            PresetRuntimeAction::AddPresetArgument,
            MessageId::PresetArgumentAdd,
            None,
        );
        add_argument.disabled =
            self.preset_argument_inputs.len() >= termirust_domain::MAX_ARGUMENTS;
        controls.push(add_argument);

        for (choice, name, selected) in [
            (
                PresetWorkingDirectoryChoice::ProjectRoot,
                MessageId::PresetWorkingProjectRoot,
                editor.working_choice == PresetWorkingChoice::ProjectRoot,
            ),
            (
                PresetWorkingDirectoryChoice::PlatformHome,
                MessageId::PresetWorkingHome,
                editor.working_choice == PresetWorkingChoice::PlatformHome,
            ),
            (
                PresetWorkingDirectoryChoice::ContainedSubdirectory,
                MessageId::PresetWorkingSubdirectory,
                editor.working_choice == PresetWorkingChoice::ContainedSubdirectory,
            ),
        ] {
            controls.push(preset_runtime_choice(
                PresetRuntimeAction::SelectWorkingDirectory(choice),
                name,
                selected,
            ));
        }
        if editor.working_choice == PresetWorkingChoice::ContainedSubdirectory {
            controls.push(preset_runtime_text_field(
                PresetRuntimeAction::SetPresetSubdirectory,
                MessageId::PresetSubdirectoryField,
                self.preset_subdirectory_input.read(cx).value().to_string(),
            ));
        }
        for (choice, name, selected) in [
            (
                PresetPermissionChoice::AskAsNeeded,
                MessageId::PresetPermissionAsk,
                editor.permission_policy == PermissionPolicy::AskAsNeeded,
            ),
            (
                PresetPermissionChoice::ReadOnly,
                MessageId::PresetPermissionReadOnly,
                editor.permission_policy == PermissionPolicy::ReadOnly,
            ),
            (
                PresetPermissionChoice::WorkspaceWrite,
                MessageId::PresetPermissionWorkspaceWrite,
                editor.permission_policy == PermissionPolicy::WorkspaceWrite,
            ),
        ] {
            controls.push(preset_runtime_choice(
                PresetRuntimeAction::SelectPermission(choice),
                name,
                selected,
            ));
        }
        controls.push(preset_runtime_checkbox(
            PresetRuntimeAction::ToggleEditorEnabled,
            MessageId::PresetEnabledField,
            editor.enabled,
            false,
        ));
        controls.push(preset_runtime_checkbox(
            PresetRuntimeAction::ToggleEditorFavorite,
            MessageId::PresetFavoriteField,
            editor.favorite,
            false,
        ));

        let args = self
            .preset_argument_inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect::<Vec<_>>();
        let risky = classify_argument_strings(editor.runtime.as_deref(), &args).is_risky();
        if risky {
            controls.push(preset_runtime_checkbox(
                PresetRuntimeAction::ConfirmRisk,
                MessageId::PresetRiskConfirmField,
                editor.confirm_risky_favorite,
                !editor.confirm_risky_favorite,
            ));
        }
        let mut save = preset_runtime_button(
            PresetRuntimeAction::SavePreset,
            MessageId::PresetSaveAction,
            None,
        );
        save.disabled = risky && editor.favorite && !editor.confirm_risky_favorite;
        controls.push(save);
        controls.push(preset_runtime_button(
            PresetRuntimeAction::CancelPreset,
            MessageId::CommonCancel,
            None,
        ));
        controls
    }

    pub(super) fn handle_preset_runtime_accessibility_command(
        &mut self,
        command: PresetRuntimeAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PresetRuntimeAccessibilityCommand::FocusRow(_) => {
                self.preset_list_focus.focus(window);
            }
            PresetRuntimeAccessibilityCommand::ActivateRow(row) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && self.preset_exists(id)
                {
                    self.edit_preset(id, window, cx);
                }
            }
            PresetRuntimeAccessibilityCommand::FocusControl(action) => match action {
                PresetRuntimeAction::SetPresetLabel => self
                    .preset_label_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                PresetRuntimeAction::SetPresetExecutable => self
                    .preset_executable_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                PresetRuntimeAction::SetPresetSubdirectory => self
                    .preset_subdirectory_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                PresetRuntimeAction::SetPresetArgument(index) => {
                    if let Some(input) = self.preset_argument_inputs.get(index) {
                        input.update(cx, |input, cx| input.focus(window, cx));
                    }
                }
                _ => self.preset_list_focus.focus(window),
            },
            PresetRuntimeAccessibilityCommand::SetControlValue(action) => {
                let Some(SemanticActionValue::Text(value)) = value else {
                    return;
                };
                if self.preset_library.editor.is_none() {
                    return;
                }
                match action {
                    PresetRuntimeAction::SetPresetLabel => {
                        Self::set_input_value(&self.preset_label_input, value, window, cx);
                    }
                    PresetRuntimeAction::SetPresetExecutable => {
                        Self::set_input_value(&self.preset_executable_input, value, window, cx);
                    }
                    PresetRuntimeAction::SetPresetSubdirectory => {
                        Self::set_input_value(&self.preset_subdirectory_input, value, window, cx);
                    }
                    PresetRuntimeAction::SetPresetArgument(index) => {
                        if let Some(input) = self.preset_argument_inputs.get(index).cloned() {
                            Self::set_input_value(&input, value, window, cx);
                        }
                    }
                    _ => return,
                }
                cx.notify();
            }
            PresetRuntimeAccessibilityCommand::ActivateControl(action) => {
                self.activate_preset_runtime_control(action, window, cx);
            }
        }
    }

    fn activate_preset_runtime_control(
        &mut self,
        action: PresetRuntimeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PresetRuntimeAction::RetryStore => self.retry_preset_library(window, cx),
            PresetRuntimeAction::StartScan => self.start_preset_scan(true, window, cx),
            PresetRuntimeAction::CancelScan => self.cancel_preset_scan(cx),
            PresetRuntimeAction::AddPreset => self.open_new_preset(window, cx),
            PresetRuntimeAction::AcceptRuntime(row) => {
                if row.kind != PresetRuntimeRowKind::Runtime {
                    return;
                }
                let candidate = self
                    .preset_library
                    .scan_report
                    .as_ref()
                    .and_then(|report| {
                        report.entries.iter().find(|candidate| {
                            runtime_row_id(candidate.result.runtime_id.as_str()) == row
                        })
                    })
                    .cloned();
                if let Some(candidate) = candidate {
                    self.accept_detected_preset(candidate, cx);
                }
            }
            PresetRuntimeAction::MovePreset(row, direction) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && self.preset_exists(id)
                {
                    self.move_preset(
                        id,
                        if direction == PresetMoveDirection::Up {
                            -1
                        } else {
                            1
                        },
                        cx,
                    );
                }
            }
            PresetRuntimeAction::TogglePresetEnabled(row) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && let Some(preset) = self.preset(id)
                {
                    self.update_preset_flags(id, Some(!preset.enabled), None, cx);
                }
            }
            PresetRuntimeAction::TogglePresetFavorite(row) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && let Some(preset) = self.preset(id)
                {
                    self.update_preset_flags(id, None, Some(!preset.favorite), cx);
                }
            }
            PresetRuntimeAction::EditPreset(row) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && self.preset_exists(id)
                {
                    self.edit_preset(id, window, cx);
                }
            }
            PresetRuntimeAction::DeletePreset(row) => {
                if let Some(id) = accessible_preset_row_id(row)
                    && self.preset_exists(id)
                {
                    self.delete_preset(id, cx);
                }
            }
            PresetRuntimeAction::AddPresetArgument => self.add_preset_argument(window, cx),
            PresetRuntimeAction::RemovePresetArgument(index) => {
                self.remove_preset_argument(index, cx);
            }
            PresetRuntimeAction::SelectWorkingDirectory(choice) => {
                if let Some(editor) = self.preset_library.editor.as_mut() {
                    editor.working_choice = match choice {
                        PresetWorkingDirectoryChoice::ProjectRoot => {
                            PresetWorkingChoice::ProjectRoot
                        }
                        PresetWorkingDirectoryChoice::PlatformHome => {
                            PresetWorkingChoice::PlatformHome
                        }
                        PresetWorkingDirectoryChoice::ContainedSubdirectory => {
                            PresetWorkingChoice::ContainedSubdirectory
                        }
                    };
                    cx.notify();
                }
            }
            PresetRuntimeAction::SelectPermission(choice) => {
                if let Some(editor) = self.preset_library.editor.as_mut() {
                    editor.permission_policy = match choice {
                        PresetPermissionChoice::AskAsNeeded => PermissionPolicy::AskAsNeeded,
                        PresetPermissionChoice::ReadOnly => PermissionPolicy::ReadOnly,
                        PresetPermissionChoice::WorkspaceWrite => PermissionPolicy::WorkspaceWrite,
                    };
                    cx.notify();
                }
            }
            PresetRuntimeAction::ToggleEditorEnabled => {
                if let Some(editor) = self.preset_library.editor.as_mut() {
                    editor.enabled = !editor.enabled;
                    cx.notify();
                }
            }
            PresetRuntimeAction::ToggleEditorFavorite => {
                if let Some(editor) = self.preset_library.editor.as_mut() {
                    editor.favorite = !editor.favorite;
                    cx.notify();
                }
            }
            PresetRuntimeAction::ConfirmRisk => {
                if let Some(editor) = self.preset_library.editor.as_mut() {
                    editor.confirm_risky_favorite = !editor.confirm_risky_favorite;
                    cx.notify();
                }
            }
            PresetRuntimeAction::SavePreset => self.save_preset_editor(cx),
            PresetRuntimeAction::CancelPreset => self.cancel_preset_editor(cx),
            PresetRuntimeAction::SetPresetLabel
            | PresetRuntimeAction::SetPresetExecutable
            | PresetRuntimeAction::SetPresetArgument(_)
            | PresetRuntimeAction::SetPresetSubdirectory => {}
        }
    }

    fn preset(&self, id: PresetId) -> Option<LaunchPreset> {
        self.preset_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.presets.iter().find(|preset| preset.id == id))
            .cloned()
    }

    fn preset_exists(&self, id: PresetId) -> bool {
        self.preset(id).is_some()
    }

    pub(super) fn render_presets_view(&self, cx: &Context<Self>) -> AnyElement {
        let read_only = self
            .preset_library
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.read_only);
        let scanning = self.preset_library.scan_cancel.is_some();
        let content = match &self.preset_library.load_state {
            PresetLibraryLoadState::Loading => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(localization::presets_scanning())
                .into_any_element(),
            PresetLibraryLoadState::Failed(_) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE_4))
                .child(
                    self.preset_library
                        .recovery_message()
                        .unwrap_or_else(localization::preset_store_unavailable),
                )
                .child(
                    Button::new("presets-store-retry")
                        .icon(IconName::Redo2)
                        .label(localization::common_retry())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.retry_preset_library(window, cx);
                        })),
                )
                .into_any_element(),
            PresetLibraryLoadState::Ready => self.render_preset_list(cx),
        };

        v_flex()
            .id("presets-view")
            .debug_selector(|| "presets-view".to_string())
            .track_focus(&self.preset_list_focus)
            .flex_1()
            .min_h_0()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .gap(px(theme::SPACE_4))
                    .px(px(theme::SPACE_6))
                    .py(px(theme::SPACE_5))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_HEADING_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::presets_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::presets_subtitle()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_3))
                            .when(scanning, |this| {
                                this.child(
                                    Button::new("presets-scan-cancel")
                                        .label(localization::common_cancel())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_preset_scan(cx);
                                        })),
                                )
                            })
                            .when(!scanning, |this| {
                                this.child(
                                    Button::new("presets-scan")
                                        .icon(IconName::Redo2)
                                        .label(localization::presets_scan_action())
                                        .disabled(read_only)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.start_preset_scan(true, window, cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new("presets-add")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(localization::presets_add_action())
                                    .disabled(read_only)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_new_preset(window, cx);
                                    })),
                            ),
                    ),
            )
            .when_some(self.preset_library.recovery_message(), |this, message| {
                this.child(status_banner("preset-recovery", message, theme::warning()))
            })
            .when(scanning, |this| {
                this.child(status_banner(
                    "preset-scanning",
                    localization::presets_scanning(),
                    theme::accent(),
                ))
            })
            .when(self.preset_library.scan_report.is_none(), |this| {
                this.child(self.render_pending_runtime_report(scanning))
            })
            .when_some(self.preset_library.scan_report.as_ref(), |this, report| {
                this.child(self.render_detection_report(report, scanning, cx))
            })
            .when_some(self.preset_library.editor.as_ref(), |this, editor| {
                this.child(self.render_preset_editor(editor, cx))
            })
            .child(content)
            .into_any_element()
    }

    fn render_detection_report(
        &self,
        report: &RuntimeDiscoveryReport,
        scanning: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        v_flex()
            .id("preset-detection-results")
            .mx(px(theme::SPACE_6))
            .mt(px(theme::SPACE_5))
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .child(
                div()
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(localization::presets_detected_title()),
            )
            .when(report.partial && !scanning, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::warning())
                        .child(localization::presets_scan_partial()),
                )
            })
            .children(
                report
                    .entries
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, candidate)| {
                        let (status, color, icon) = if scanning {
                            (
                                localization::presets_scanning(),
                                theme::accent(),
                                IconName::LoaderCircle,
                            )
                        } else {
                            detection_presentation(candidate.result.status)
                        };
                        let accept = !scanning
                            && candidate.result.status == RuntimeDetectionStatus::Available
                            && !candidate.result.capabilities.is_empty()
                            && candidate.executable.is_some()
                            && !self.preset_exists_for(&candidate);
                        h_flex()
                            .id(("detected-preset", index))
                            .justify_between()
                            .items_center()
                            .flex_wrap()
                            .gap(px(theme::SPACE_3))
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div().font_medium().text_color(theme::text_main()).child(
                                            runtime_label(candidate.result.runtime_id.as_str()),
                                        ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(px(theme::SPACE_2))
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(color)
                                            .child(
                                                Icon::new(icon)
                                                    .size(px(theme::TYPE_BODY_SMALL_SIZE)),
                                            )
                                            .child(status)
                                            .when_some(
                                                candidate.result.safe_version.clone(),
                                                |this, version| {
                                                    this.child(
                                                        localization::preset_detected_version(
                                                            version,
                                                        ),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap(px(theme::SPACE_2))
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::runtime_registry_contract(
                                                candidate.result.descriptor_version,
                                            ))
                                            .when_some(
                                                candidate.executable.as_ref(),
                                                |this, executable| {
                                                    this.child(
                                                        localization::runtime_registry_executable(
                                                            executable_display(executable),
                                                        ),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::runtime_registry_capabilities(
                                                capability_summary(&candidate.result.capabilities),
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(("accept-detected-preset", index))
                                    .small()
                                    .icon(IconName::Plus)
                                    .label(localization::preset_accept_action())
                                    .disabled(!accept)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.accept_detected_preset(candidate.clone(), cx);
                                    })),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn render_pending_runtime_report(&self, scanning: bool) -> AnyElement {
        v_flex()
            .id("preset-detection-results")
            .mx(px(theme::SPACE_6))
            .mt(px(theme::SPACE_5))
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .child(
                div()
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(localization::presets_detected_title()),
            )
            .children(known_runtime_descriptors().into_iter().enumerate().map(
                |(index, descriptor)| {
                    h_flex()
                        .id(("pending-runtime", index))
                        .justify_between()
                        .items_center()
                        .flex_wrap()
                        .gap(px(theme::SPACE_3))
                        .child(
                            v_flex()
                                .gap(px(theme::SPACE_2))
                                .child(
                                    div()
                                        .font_medium()
                                        .text_color(theme::text_main())
                                        .child(runtime_label(descriptor.id.as_str())),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(theme::SPACE_2))
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(if scanning {
                                            theme::accent()
                                        } else {
                                            theme::text_muted()
                                        })
                                        .child(
                                            Icon::new(if scanning {
                                                IconName::LoaderCircle
                                            } else {
                                                IconName::Minus
                                            })
                                            .size(px(theme::TYPE_BODY_SMALL_SIZE)),
                                        )
                                        .child(if scanning {
                                            localization::presets_scanning()
                                        } else {
                                            localization::runtime_status_not_checked()
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(localization::runtime_registry_contract(
                                            descriptor.descriptor_version,
                                        )),
                                ),
                        )
                        .into_any_element()
                },
            ))
            .into_any_element()
    }

    fn preset_exists_for(&self, candidate: &RuntimeDiscoveryEntry) -> bool {
        let Some(executable) = candidate.executable.as_ref() else {
            return false;
        };
        self.preset_library
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.presets.iter().any(|preset| {
                    preset.runtime.as_ref() == Some(&candidate.result.runtime_id)
                        && preset.executable == *executable
                })
            })
    }

    fn render_preset_list(&self, cx: &Context<Self>) -> AnyElement {
        let presets = self
            .preset_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.presets.as_slice())
            .unwrap_or_default();
        if presets.is_empty() {
            return v_flex()
                .id("presets-empty")
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE_4))
                .p(px(theme::SPACE_7))
                .child(
                    Icon::new(IconName::SquareTerminal)
                        .size(px(theme::SPACE_7))
                        .text_color(theme::accent()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                        .font_semibold()
                        .child(localization::presets_empty_title()),
                )
                .child(
                    div()
                        .text_center()
                        .text_color(theme::text_muted())
                        .child(localization::presets_empty_description()),
                )
                .into_any_element();
        }
        v_flex()
            .id("presets-list")
            .flex_1()
            .min_h_0()
            .gap(px(theme::SPACE_3))
            .p(px(theme::SPACE_6))
            .overflow_y_scroll()
            .children(
                presets.iter().enumerate().map(|(index, preset)| {
                    self.render_preset_row(index, preset, presets.len(), cx)
                }),
            )
            .into_any_element()
    }

    fn render_preset_row(
        &self,
        index: usize,
        preset: &LaunchPreset,
        count: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = preset.id;
        let key = preset_element_key(id);
        let available = executable_available(&preset.executable);
        let enabled = preset.enabled;
        let favorite = preset.favorite;
        h_flex()
            .id(("preset-row", key))
            .debug_selector(|| "preset-row".to_string())
            .w_full()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap(px(theme::SPACE_5))
            .p(px(theme::SPACE_5))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .child(
                v_flex()
                    .min_w_0()
                    .gap(px(theme::SPACE_2))
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .items_center()
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(preset.label.as_str().to_string()),
                            )
                            .when(preset.favorite, |this| {
                                this.child(
                                    Icon::new(IconName::Star)
                                        .size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::warning()),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .flex_wrap()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(executable_display(&preset.executable))
                            .child(localization::preset_argument_count(preset.args.len())),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_3))
                            .flex_wrap()
                            .child(preset_status_chip(
                                if available {
                                    localization::preset_status_supported()
                                } else {
                                    localization::preset_status_missing()
                                },
                                if available {
                                    theme::success()
                                } else {
                                    theme::warning()
                                },
                                if available {
                                    IconName::Check
                                } else {
                                    IconName::TriangleAlert
                                },
                            ))
                            .when(!preset.enabled, |this| {
                                this.child(preset_status_chip(
                                    localization::preset_status_disabled(),
                                    theme::text_muted(),
                                    IconName::Close,
                                ))
                            })
                            .when(preset.risk.is_risky(), |this| {
                                this.child(preset_status_chip(
                                    localization::preset_status_risky(),
                                    theme::danger(),
                                    IconName::TriangleAlert,
                                ))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new(("preset-up", key))
                            .small()
                            .icon(IconName::ChevronUp)
                            .label(localization::preset_move_up_action())
                            .disabled(index == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_preset(id, -1, cx);
                            })),
                    )
                    .child(
                        Button::new(("preset-down", key))
                            .small()
                            .icon(IconName::ChevronDown)
                            .label(localization::preset_move_down_action())
                            .disabled(index + 1 == count)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_preset(id, 1, cx);
                            })),
                    )
                    .child(
                        Button::new(("preset-enabled", key))
                            .small()
                            .label(localization::preset_enabled_field())
                            .selected(enabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_preset_flags(id, Some(!enabled), None, cx);
                            })),
                    )
                    .child(
                        Button::new(("preset-favorite", key))
                            .small()
                            .icon(IconName::Star)
                            .label(localization::preset_favorite_field())
                            .selected(favorite)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_preset_flags(id, None, Some(!favorite), cx);
                            })),
                    )
                    .child(
                        Button::new(("preset-edit", key))
                            .small()
                            .label(localization::preset_edit_action())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_preset(id, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("preset-delete", key))
                            .small()
                            .danger()
                            .icon(IconName::Delete)
                            .label(localization::preset_delete_action())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_preset(id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_preset_editor(&self, editor: &PresetEditorState, cx: &Context<Self>) -> AnyElement {
        let args = self
            .preset_argument_inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect::<Vec<_>>();
        let risk = classify_argument_strings(editor.runtime.as_deref(), &args);
        v_flex()
            .id("preset-editor")
            .debug_selector(|| "preset-editor".to_string())
            .mx(px(theme::SPACE_6))
            .mt(px(theme::SPACE_5))
            .p(px(theme::SPACE_5))
            .gap(px(theme::SPACE_4))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(if risk.is_risky() {
                theme::danger()
            } else {
                theme::focus_ring()
            })
            .child(
                div()
                    .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .font_semibold()
                    .child(if editor.editing_id.is_some() {
                        localization::preset_form_title_edit()
                    } else {
                        localization::preset_form_title_new()
                    }),
            )
            .child(form_field(
                localization::preset_label_field(),
                Input::new(&self.preset_label_input).into_any_element(),
            ))
            .child(form_field(
                localization::preset_executable_field(),
                Input::new(&self.preset_executable_input).into_any_element(),
            ))
            .child(
                v_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .font_medium()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .child(localization::preset_arguments_field()),
                    )
                    .children(self.preset_argument_inputs.iter().enumerate().map(
                        |(index, input)| {
                            h_flex()
                                .gap(px(theme::SPACE_2))
                                .child(div().flex_1().child(Input::new(input)))
                                .child(
                                    Button::new(("remove-preset-argument", index))
                                        .small()
                                        .icon(IconName::Close)
                                        .label(localization::preset_argument_remove())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_preset_argument(index, cx);
                                        })),
                                )
                        },
                    ))
                    .child(
                        Button::new("add-preset-argument")
                            .small()
                            .icon(IconName::Plus)
                            .label(localization::preset_argument_add())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_preset_argument(window, cx);
                            })),
                    ),
            )
            .child(
                option_group(
                    localization::preset_working_directory_field(),
                    [
                        (
                            PresetWorkingChoice::ProjectRoot,
                            localization::preset_working_project_root(),
                        ),
                        (
                            PresetWorkingChoice::PlatformHome,
                            localization::preset_working_home(),
                        ),
                        (
                            PresetWorkingChoice::ContainedSubdirectory,
                            localization::preset_working_subdirectory(),
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (choice, label))| {
                        Button::new(("preset-working-choice", index))
                            .small()
                            .label(label)
                            .selected(editor.working_choice == choice)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(editor) = &mut this.preset_library.editor {
                                    editor.working_choice = choice;
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
                )
                .when(
                    editor.working_choice == PresetWorkingChoice::ContainedSubdirectory,
                    |this| {
                        this.child(form_field(
                            localization::preset_subdirectory_field(),
                            Input::new(&self.preset_subdirectory_input).into_any_element(),
                        ))
                    },
                ),
            )
            .child(option_group(
                localization::preset_permission_field(),
                [
                    (
                        PermissionPolicy::AskAsNeeded,
                        localization::preset_permission_ask(),
                    ),
                    (
                        PermissionPolicy::ReadOnly,
                        localization::preset_permission_read_only(),
                    ),
                    (
                        PermissionPolicy::WorkspaceWrite,
                        localization::preset_permission_workspace_write(),
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (policy, label))| {
                    Button::new(("preset-permission", index))
                        .small()
                        .label(label)
                        .selected(editor.permission_policy == policy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.preset_library.editor {
                                editor.permission_policy = policy;
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                }),
            ))
            .child(
                h_flex()
                    .gap(px(theme::SPACE_3))
                    .flex_wrap()
                    .child(toggle_button(
                        "preset-editor-enabled",
                        localization::preset_enabled_field(),
                        editor.enabled,
                        cx.listener(|this, _, _, cx| {
                            if let Some(editor) = &mut this.preset_library.editor {
                                editor.enabled = !editor.enabled;
                            }
                            cx.notify();
                        }),
                    ))
                    .child(toggle_button(
                        "preset-editor-favorite",
                        localization::preset_favorite_field(),
                        editor.favorite,
                        cx.listener(|this, _, _, cx| {
                            if let Some(editor) = &mut this.preset_library.editor {
                                editor.favorite = !editor.favorite;
                            }
                            cx.notify();
                        }),
                    )),
            )
            .when(risk.is_risky(), |this| {
                this.child(
                    v_flex()
                        .gap(px(theme::SPACE_2))
                        .p(px(theme::SPACE_3))
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::with_alpha(theme::danger(), 0.1))
                        .text_color(theme::danger())
                        .child(
                            h_flex()
                                .gap(px(theme::SPACE_2))
                                .child(Icon::new(IconName::TriangleAlert).size(px(theme::SPACE_4)))
                                .child(localization::preset_risk_warning()),
                        )
                        .child(toggle_button(
                            "preset-risk-confirm",
                            localization::preset_risk_confirm_field(),
                            editor.confirm_risky_favorite,
                            cx.listener(|this, _, _, cx| {
                                if let Some(editor) = &mut this.preset_library.editor {
                                    editor.confirm_risky_favorite = !editor.confirm_risky_favorite;
                                }
                                cx.notify();
                            }),
                        )),
                )
            })
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::preset_safe_copy()),
            )
            .child(
                h_flex()
                    .gap(px(theme::SPACE_3))
                    .child(
                        Button::new("preset-save")
                            .primary()
                            .label(localization::preset_save_action())
                            .disabled(
                                risk.is_risky()
                                    && editor.favorite
                                    && !editor.confirm_risky_favorite,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_preset_editor(cx);
                            })),
                    )
                    .child(
                        Button::new("preset-cancel")
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_preset_editor(cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn accessible_preset_id(id: PresetId) -> PresetRuntimeRowId {
    PresetRuntimeRowId::preset(id.as_uuid().as_u128())
}

fn accessible_preset_row_id(row: PresetRuntimeRowId) -> Option<PresetId> {
    (row.kind == PresetRuntimeRowKind::Preset)
        .then(|| PresetId::from_uuid(uuid::Uuid::from_u128(row.value)))
}

fn runtime_row_id(runtime_id: &str) -> PresetRuntimeRowId {
    PresetRuntimeRowId::runtime(stable_runtime_row_value(runtime_id))
}

fn runtime_detection_message_id(status: RuntimeDetectionStatus) -> MessageId {
    match status {
        RuntimeDetectionStatus::Available => MessageId::PresetStatusSupported,
        RuntimeDetectionStatus::UnsupportedVersion => MessageId::PresetStatusUnsupported,
        RuntimeDetectionStatus::Missing => MessageId::PresetStatusMissing,
        RuntimeDetectionStatus::PermissionDenied => MessageId::PresetStatusPermission,
        RuntimeDetectionStatus::Partial => MessageId::PresetStatusFailed,
    }
}

fn preset_status_message_id(preset: &LaunchPreset, executable_available: bool) -> MessageId {
    if preset.risk.is_risky() {
        MessageId::PresetStatusRisky
    } else if !preset.enabled {
        MessageId::PresetStatusDisabled
    } else if executable_available {
        MessageId::PresetStatusSupported
    } else {
        MessageId::PresetStatusMissing
    }
}

fn append_runtime_semantic_rows(
    rows: &mut Vec<PresetRuntimeRow>,
    candidate: &RuntimeDiscoveryEntry,
    position: usize,
    set_size: usize,
) {
    let runtime_id = candidate.result.runtime_id.as_str();
    let runtime_row = runtime_row_id(runtime_id);
    let capabilities = candidate.result.capabilities.iter().collect::<Vec<_>>();
    rows.push(PresetRuntimeRow {
        id: runtime_row,
        parent: None,
        name: runtime_label(runtime_id),
        status: runtime_detection_message_id(candidate.result.status),
        detail: Some(
            candidate
                .result
                .safe_version
                .clone()
                .unwrap_or_else(localization::runtime_version_unverified),
        ),
        selected: false,
        disabled: candidate.result.status != RuntimeDetectionStatus::Available,
        checked: Some(!capabilities.is_empty()),
        risky: false,
        stale: false,
        position,
        set_size,
    });
    let capability_count = capabilities.len().max(1);
    for (index, capability) in capabilities.into_iter().enumerate() {
        let message = runtime_capability_message(capability);
        rows.push(PresetRuntimeRow {
            id: PresetRuntimeRowId::capability(stable_capability_row_value(runtime_id, message)),
            parent: Some(runtime_row),
            name: runtime_capability_label(capability),
            status: MessageId::RuntimeConfidenceVerified,
            detail: None,
            selected: false,
            disabled: false,
            checked: Some(true),
            risky: false,
            stale: false,
            position: index + 1,
            set_size: capability_count,
        });
    }
}

fn preset_runtime_button(
    action: PresetRuntimeAction,
    name: MessageId,
    parent: Option<PresetRuntimeRowId>,
) -> PresetRuntimeControl {
    PresetRuntimeControl {
        action,
        parent,
        role: PresetRuntimeControlRole::Button,
        name,
        value: None,
        selected: false,
        disabled: false,
        invalid: false,
    }
}

fn preset_runtime_text_field(
    action: PresetRuntimeAction,
    name: MessageId,
    value: String,
) -> PresetRuntimeControl {
    PresetRuntimeControl {
        action,
        parent: None,
        role: PresetRuntimeControlRole::TextField,
        name,
        value: Some(value),
        selected: false,
        disabled: false,
        invalid: false,
    }
}

fn preset_runtime_choice(
    action: PresetRuntimeAction,
    name: MessageId,
    selected: bool,
) -> PresetRuntimeControl {
    PresetRuntimeControl {
        action,
        parent: None,
        role: PresetRuntimeControlRole::RadioButton,
        name,
        value: None,
        selected,
        disabled: false,
        invalid: false,
    }
}

fn preset_runtime_checkbox(
    action: PresetRuntimeAction,
    name: MessageId,
    selected: bool,
    invalid: bool,
) -> PresetRuntimeControl {
    PresetRuntimeControl {
        action,
        parent: None,
        role: PresetRuntimeControlRole::Checkbox,
        name,
        value: None,
        selected,
        disabled: false,
        invalid,
    }
}

fn new_argument_input(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder("--literal-argument"))
}

fn form_field(label: String, control: AnyElement) -> impl IntoElement {
    v_flex()
        .gap(px(theme::SPACE_2))
        .child(
            div()
                .font_medium()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .child(label),
        )
        .child(control)
}

fn option_group(label: String, options: impl IntoIterator<Item = AnyElement>) -> gpui::Div {
    v_flex()
        .gap(px(theme::SPACE_2))
        .child(
            div()
                .font_medium()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .child(label),
        )
        .child(
            h_flex()
                .gap(px(theme::SPACE_2))
                .flex_wrap()
                .children(options),
        )
}

fn toggle_button(
    id: &'static str,
    label: String,
    selected: bool,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .small()
        .icon(if selected {
            IconName::Check
        } else {
            IconName::Close
        })
        .label(label)
        .selected(selected)
        .on_click(listener)
}

fn status_banner(id: &'static str, message: String, color: gpui::Hsla) -> impl IntoElement {
    h_flex()
        .id(id)
        .mx(px(theme::SPACE_6))
        .mt(px(theme::SPACE_5))
        .p(px(theme::SPACE_4))
        .gap(px(theme::SPACE_3))
        .rounded(px(theme::CARD_RADIUS))
        .bg(theme::with_alpha(color, 0.1))
        .text_color(color)
        .child(Icon::new(IconName::TriangleAlert).size(px(theme::SPACE_5)))
        .child(message)
}

fn preset_status_chip(message: String, color: gpui::Hsla, icon: IconName) -> impl IntoElement {
    h_flex()
        .gap(px(theme::SPACE_2))
        .text_size(px(theme::TYPE_CAPTION_SIZE))
        .text_color(color)
        .child(Icon::new(icon).size(px(theme::TYPE_BODY_SMALL_SIZE)))
        .child(message)
}

fn detection_presentation(status: RuntimeDetectionStatus) -> (String, gpui::Hsla, IconName) {
    let label = detection_status_label(status);
    match status {
        RuntimeDetectionStatus::Available => (label, theme::success(), IconName::Check),
        RuntimeDetectionStatus::UnsupportedVersion => {
            (label, theme::warning(), IconName::TriangleAlert)
        }
        RuntimeDetectionStatus::Missing => (label, theme::text_muted(), IconName::Close),
        RuntimeDetectionStatus::PermissionDenied => {
            (label, theme::danger(), IconName::TriangleAlert)
        }
        RuntimeDetectionStatus::Partial => (label, theme::danger(), IconName::TriangleAlert),
    }
}

fn executable_available(executable: &ExecutableSpec) -> bool {
    match executable {
        ExecutableSpec::Absolute(value) => is_executable_file(Path::new(value)),
        ExecutableSpec::SearchPath(value) => {
            let path = discovery_path_snapshot();
            if path.is_empty() {
                false
            } else {
                std::env::split_paths(&path)
                    .take(128)
                    .any(|directory| is_executable_file(&directory.join(value)))
            }
        }
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn executable_display(executable: &ExecutableSpec) -> String {
    match executable {
        ExecutableSpec::SearchPath(value) => value.clone(),
        ExecutableSpec::Absolute(value) => executable_basename(value),
    }
}

fn classify_store_failure(error: StoreError) -> PresetStoreFailure {
    match error {
        StoreError::StoreNewer { .. } => PresetStoreFailure::Newer,
        StoreError::Corrupt { .. }
        | StoreError::UnsafeEntry { .. }
        | StoreError::TooLarge { .. } => PresetStoreFailure::Corrupt,
        StoreError::Io { .. }
        | StoreError::InvalidInstanceId
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_) => PresetStoreFailure::Unavailable,
    }
}

fn preset_store_error_message(error: &StoreError) -> String {
    match error {
        StoreError::PresetDomain(PresetError::StaleRevision { .. }) => {
            localization::preset_error_stale()
        }
        StoreError::PresetDomain(PresetError::RiskConfirmationRequired) => {
            localization::preset_error_risk_confirm()
        }
        StoreError::PresetDomain(_) => localization::preset_error_invalid(),
        _ => match classify_store_failure(error.clone()) {
            PresetStoreFailure::Corrupt => localization::preset_store_corrupt(),
            PresetStoreFailure::Newer => localization::preset_store_newer(),
            PresetStoreFailure::Unavailable => localization::preset_store_unavailable(),
        },
    }
}

fn preset_element_key(id: PresetId) -> u64 {
    let value = id.as_uuid().as_u128();
    (value as u64) ^ ((value >> 64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detection_state_has_text_color_and_shape() {
        for status in [
            RuntimeDetectionStatus::Available,
            RuntimeDetectionStatus::UnsupportedVersion,
            RuntimeDetectionStatus::Missing,
            RuntimeDetectionStatus::PermissionDenied,
            RuntimeDetectionStatus::Partial,
        ] {
            let (text, _, _) = detection_presentation(status);
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn absolute_executable_display_redacts_parent_path() {
        let executable = ExecutableSpec::parse("/Users/private/customer/codex").unwrap();
        assert_eq!(executable_display(&executable), "codex");
        assert!(!executable_display(&executable).contains("private"));
    }

    #[test]
    fn risk_copy_is_distinct_from_missing_and_disabled_copy() {
        assert_ne!(
            localization::preset_status_risky(),
            localization::preset_status_missing()
        );
        assert_ne!(
            localization::preset_status_risky(),
            localization::preset_status_disabled()
        );
    }
}
