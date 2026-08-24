use std::path::{Path, PathBuf};
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
    DetectionCandidate, DetectionReport, DetectionStatus, ExecutableSpec, LaunchPreset,
    PermissionPolicy, PresetDraft, PresetError, PresetId, PresetOrigin, WorkingDirectoryRule,
    classify_argument_strings,
};
use termirust_store::{PresetRepository, PresetSnapshot, StoreError, StoreHealth};

use super::{TermiRustApp, theme};
use crate::agents::{
    CliDiscovery, DiscoveryCancellation, discovery_path_snapshot, known_runtime_descriptors,
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
    pub scan_report: Option<DetectionReport>,
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
                    } else if report.candidates.iter().all(|candidate| {
                        !matches!(
                            candidate.status,
                            DetectionStatus::Supported | DetectionStatus::DetectedUnknownVersion
                        )
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

    fn accept_detected_preset(&mut self, candidate: DetectionCandidate, cx: &mut Context<Self>) {
        if !matches!(
            candidate.status,
            DetectionStatus::Supported | DetectionStatus::DetectedUnknownVersion
        ) {
            return;
        }
        let label = runtime_label(candidate.runtime.as_str());
        let draft = PresetDraft {
            id: PresetId::new(),
            label: label.clone(),
            executable: candidate.executable.as_str().to_string(),
            args: Vec::new(),
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: Some(candidate.runtime.as_str().to_string()),
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
            .when_some(self.preset_library.scan_report.as_ref(), |this, report| {
                this.child(self.render_detection_report(report, cx))
            })
            .when_some(self.preset_library.editor.as_ref(), |this, editor| {
                this.child(self.render_preset_editor(editor, cx))
            })
            .child(content)
            .into_any_element()
    }

    fn render_detection_report(&self, report: &DetectionReport, cx: &Context<Self>) -> AnyElement {
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
            .when(report.partial, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::warning())
                        .child(localization::presets_scan_partial()),
                )
            })
            .children(
                report
                    .candidates
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, candidate)| {
                        let (status, color, icon) = detection_presentation(&candidate.status);
                        let accept = matches!(
                            candidate.status,
                            DetectionStatus::Supported | DetectionStatus::DetectedUnknownVersion
                        ) && !self.preset_exists_for(&candidate);
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
                                        div()
                                            .font_medium()
                                            .text_color(theme::text_main())
                                            .child(runtime_label(candidate.runtime.as_str())),
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
                                                candidate.version.clone(),
                                                |this, version| {
                                                    this.child(
                                                        localization::preset_detected_version(
                                                            version,
                                                        ),
                                                    )
                                                },
                                            ),
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

    fn preset_exists_for(&self, candidate: &DetectionCandidate) -> bool {
        self.preset_library
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.presets.iter().any(|preset| {
                    preset.runtime.as_ref() == Some(&candidate.runtime)
                        && preset.executable == candidate.executable
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

fn detection_presentation(status: &DetectionStatus) -> (String, gpui::Hsla, IconName) {
    match status {
        DetectionStatus::Supported => (
            localization::preset_status_supported(),
            theme::success(),
            IconName::Check,
        ),
        DetectionStatus::DetectedUnknownVersion => (
            localization::preset_status_unknown(),
            theme::warning(),
            IconName::TriangleAlert,
        ),
        DetectionStatus::UnsupportedVersion => (
            localization::preset_status_unsupported(),
            theme::warning(),
            IconName::TriangleAlert,
        ),
        DetectionStatus::Missing => (
            localization::preset_status_missing(),
            theme::text_muted(),
            IconName::Close,
        ),
        DetectionStatus::PermissionDenied => (
            localization::preset_status_permission(),
            theme::danger(),
            IconName::TriangleAlert,
        ),
        DetectionStatus::TimedOut => (
            localization::preset_status_timeout(),
            theme::warning(),
            IconName::TriangleAlert,
        ),
        DetectionStatus::Failed => (
            localization::preset_status_failed(),
            theme::danger(),
            IconName::TriangleAlert,
        ),
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
        ExecutableSpec::Absolute(value) => PathBuf::from(value)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Executable")
            .to_string(),
    }
}

fn runtime_label(runtime: &str) -> String {
    match runtime {
        "codex" => localization::runtime_label_codex(),
        "claude" => localization::runtime_label_claude(),
        "gemini" => localization::runtime_label_gemini(),
        value => value.to_string(),
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
        | StoreError::SessionDomain(_) => PresetStoreFailure::Unavailable,
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
            DetectionStatus::Supported,
            DetectionStatus::DetectedUnknownVersion,
            DetectionStatus::UnsupportedVersion,
            DetectionStatus::Missing,
            DetectionStatus::PermissionDenied,
            DetectionStatus::TimedOut,
            DetectionStatus::Failed,
        ] {
            let (text, _, _) = detection_presentation(&status);
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
