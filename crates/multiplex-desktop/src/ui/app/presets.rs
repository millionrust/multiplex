use std::path::Path;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{AppContext as _, Context, Entity, Window};
use gpui_component::input::InputState;
use multiplex_domain::{
    ExecutableSpec, LaunchPreset, PermissionPolicy, PresetDraft, PresetError, PresetId,
    PresetOrigin, RuntimeDetectionStatus, WorkingDirectoryRule, classify_argument_strings,
};
use multiplex_store::{PresetRepository, PresetSnapshot, StoreError, StoreHealth};
use multiplex_ui_contract::{
    MessageId, PresetMoveDirection, PresetPermissionChoice, PresetRuntimeAccessibilityCommand,
    PresetRuntimeAction, PresetRuntimeControl, PresetRuntimeControlRole, PresetRuntimeRow,
    PresetRuntimeRowId, PresetRuntimeRowKind, PresetRuntimeScreen, PresetRuntimeSemanticSnapshot,
    PresetRuntimeSurfaceState, PresetWorkingDirectoryChoice, SemanticActionValue,
    stable_capability_row_value, stable_runtime_row_value,
};

use super::MultiplexApp;
use super::runtimes::{
    executable_basename, runtime_capability_label, runtime_capability_message, runtime_label,
};
use crate::agents::{
    CliDiscovery, DiscoveryCancellation, RuntimeDiscoveryEntry, RuntimeDiscoveryReport,
    discovery_path_snapshot, known_runtime_descriptors,
};
use crate::storage::library_store_dir;
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
    SessionFolder,
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
        let repository = library_store_dir()
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

impl MultiplexApp {
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
            working_choice: PresetWorkingChoice::SessionFolder,
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
            WorkingDirectoryRule::SessionFolder => {
                (PresetWorkingChoice::SessionFolder, String::new())
            }
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
        if self.preset_argument_inputs.len() >= multiplex_domain::MAX_ARGUMENTS {
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
            PresetWorkingChoice::SessionFolder => WorkingDirectoryRule::SessionFolder,
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
            .unwrap_or(multiplex_domain::Revision::ZERO);
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
            working_directory: WorkingDirectoryRule::SessionFolder,
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
            self.preset_argument_inputs.len() >= multiplex_domain::MAX_ARGUMENTS;
        controls.push(add_argument);

        for (choice, name, selected) in [
            (
                PresetWorkingDirectoryChoice::SessionFolder,
                MessageId::PresetWorkingSessionFolder,
                editor.working_choice == PresetWorkingChoice::SessionFolder,
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
                        PresetWorkingDirectoryChoice::SessionFolder => {
                            PresetWorkingChoice::SessionFolder
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

fn new_argument_input(window: &mut Window, cx: &mut Context<MultiplexApp>) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder("--literal-argument"))
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
        | StoreError::StaleRevision { .. }
        | StoreError::RevisionOverflow
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
