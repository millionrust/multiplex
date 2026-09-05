use std::fs;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Styled, Window, div,
    px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{
    GitReference, ManagedWorktreeId, PresetId, ProjectId, WorktreeError, WorktreeIntent,
    WorktreeIntentState, WorktreeLaunchDraft, WorktreeLaunchStage, WorktreeRegistration,
};
use termirust_store::StoreError;
use termirust_ui_contract::{
    MessageId, SemanticActionValue, WorktreeArtifactAccessibilityCommand, WorktreeArtifactAction,
    WorktreeArtifactControl, WorktreeArtifactControlRole, WorktreeArtifactProgress,
    WorktreeArtifactRow, WorktreeArtifactRowId, WorktreeArtifactScreen,
    WorktreeArtifactSemanticSnapshot, WorktreeArtifactSurfaceState,
};

use super::project_coordinator::{WorktreeInspectionRequest, WorktreePlanRequest};
use super::{TermiRustApp, theme};
use crate::storage::managed_agent_worktree_dir;
use crate::ui::localization;
use crate::worktree_launch::{WorktreeCancellation, WorktreeInspection, generated_worktree_branch};

pub(super) struct WorktreeLaunchUiState {
    pub source_project_id: ProjectId,
    pub worktree_id: ManagedWorktreeId,
    pub child_project_id: ProjectId,
    pub stage: WorktreeLaunchStage,
    pub inspection: Option<WorktreeInspection>,
    pub selected_preset_id: Option<PresetId>,
    pub registered_child_id: Option<ProjectId>,
    pub error: Option<WorktreeError>,
    pub generation: u64,
    pub cancellation: WorktreeCancellation,
    pub fetched: bool,
    pub current_branch_confirmed: bool,
    pub recovering: bool,
}

impl TermiRustApp {
    pub(super) fn worktree_semantic_snapshot(
        &self,
        cx: &Context<Self>,
    ) -> Option<WorktreeArtifactSemanticSnapshot> {
        let state = self.worktree_launch.as_ref()?;
        let busy = worktree_busy(state.stage);
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let row_id = WorktreeArtifactRowId::worktree(state.worktree_id.as_uuid().as_u128());
        let inspection = state.inspection.as_ref();
        let name = inspection
            .map(|inspection| inspection.repository_basename.clone())
            .unwrap_or_else(|| state.worktree_id.to_string());
        let detail = inspection.map(|inspection| {
            format!(
                "{} · {} · {}",
                inspection.plan.selected_base.ref_name,
                inspection.plan.selected_base.commit_oid.short(),
                managed_path_preview(&inspection.plan),
            )
        });
        let path_missing = inspection.is_some_and(|inspection| {
            matches!(
                fs::symlink_metadata(inspection.plan.managed_path.as_path()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        });
        let mut controls = vec![
            worktree_text_control(
                WorktreeArtifactAction::SetWorktreeBase,
                MessageId::WorktreeBaseField,
                self.worktree_base_input.read(cx).value().to_string(),
                busy || state.recovering || state.stage == WorktreeLaunchStage::Registered,
            ),
            worktree_text_control(
                WorktreeArtifactAction::SetWorktreeBranch,
                MessageId::WorktreeBranchField,
                self.worktree_branch_input.read(cx).value().to_string(),
                busy || state.recovering || state.stage == WorktreeLaunchStage::Registered,
            ),
        ];
        if !state.recovering && state.stage != WorktreeLaunchStage::Registered {
            controls.extend([
                worktree_button(
                    WorktreeArtifactAction::ReviewWorktree,
                    MessageId::WorktreeRefreshAction,
                    busy,
                ),
                worktree_button(
                    WorktreeArtifactAction::FetchWorktree,
                    MessageId::WorktreeFetchAction,
                    busy,
                ),
                worktree_button(
                    WorktreeArtifactAction::ConfirmCurrentBase,
                    MessageId::WorktreeCurrentAction,
                    busy,
                ),
                worktree_button(
                    WorktreeArtifactAction::CreateWorktree,
                    MessageId::WorktreeCreateAction,
                    busy || inspection.is_none(),
                ),
            ]);
        }
        if state.recovering {
            controls.push(worktree_button(
                WorktreeArtifactAction::VerifyRecovery,
                MessageId::WorktreeVerifyAction,
                busy || path_missing,
            ));
            if path_missing {
                controls.push(worktree_button(
                    WorktreeArtifactAction::ForgetRecovery,
                    MessageId::WorktreeForgetRecoveryAction,
                    busy,
                ));
            }
        }
        if state.stage == WorktreeLaunchStage::Registered {
            controls.push(worktree_button(
                WorktreeArtifactAction::StartSession,
                MessageId::WorktreeStartSessionAction,
                state.selected_preset_id.is_none(),
            ));
        }
        if let Some(snapshot) = self.preset_library.snapshot.as_ref() {
            controls.extend(snapshot.presets.iter().filter(|preset| preset.enabled).map(
                |preset| WorktreeArtifactControl {
                    action: WorktreeArtifactAction::SelectPreset(preset.id.as_uuid().as_u128()),
                    parent: Some(row_id),
                    role: WorktreeArtifactControlRole::RadioButton,
                    name: MessageId::WorktreePresetField,
                    value: Some(preset.label.as_str().to_string()),
                    selected: state.selected_preset_id == Some(preset.id),
                    disabled: busy,
                    invalid: false,
                },
            ));
        }
        controls.push(worktree_button(
            WorktreeArtifactAction::CancelOrCloseWorktree,
            if busy {
                MessageId::CommonCancel
            } else {
                MessageId::CommonClose
            },
            false,
        ));

        Some(WorktreeArtifactSemanticSnapshot {
            screen: WorktreeArtifactScreen::WorktreeLaunch,
            state: worktree_surface_state(state),
            rows: vec![WorktreeArtifactRow {
                id: row_id,
                parent: None,
                name,
                status: worktree_stage_message_id(state.stage),
                detail,
                selected: true,
                disabled: false,
                expanded: None,
                invalid: state.error.is_some(),
                stale: state.recovering,
                position: 1,
                set_size: 1,
            }],
            controls,
            progress: busy.then_some(WorktreeArtifactProgress {
                label: worktree_stage_message_id(state.stage),
                current: worktree_stage_progress(state.stage),
                maximum: Some(4),
                cancellable: true,
            }),
            recording_friendly,
        })
    }

    pub(super) fn handle_worktree_accessibility_command(
        &mut self,
        command: WorktreeArtifactAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            WorktreeArtifactAccessibilityCommand::FocusRow(_)
            | WorktreeArtifactAccessibilityCommand::ActivateRow(_) => {
                self.project_list_focus.focus(window);
            }
            WorktreeArtifactAccessibilityCommand::FocusControl(action) => match action {
                WorktreeArtifactAction::SetWorktreeBase => self
                    .worktree_base_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                WorktreeArtifactAction::SetWorktreeBranch => self
                    .worktree_branch_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                _ => self.project_list_focus.focus(window),
            },
            WorktreeArtifactAccessibilityCommand::SetControlValue(action) => {
                let Some(SemanticActionValue::Text(value)) = value else {
                    return;
                };
                if !self.worktree_semantic_snapshot(cx).is_some_and(|snapshot| {
                    snapshot.controls.iter().any(|control| {
                        control.action == action
                            && control.role == WorktreeArtifactControlRole::TextField
                            && !control.disabled
                    })
                }) {
                    return;
                }
                match action {
                    WorktreeArtifactAction::SetWorktreeBase => {
                        Self::set_input_value(&self.worktree_base_input, value, window, cx);
                    }
                    WorktreeArtifactAction::SetWorktreeBranch => {
                        Self::set_input_value(&self.worktree_branch_input, value, window, cx);
                    }
                    _ => return,
                }
                cx.notify();
            }
            WorktreeArtifactAccessibilityCommand::ActivateControl(action) => {
                self.activate_worktree_accessibility_action(action, window, cx);
            }
            WorktreeArtifactAccessibilityCommand::CancelProgress => {
                self.close_worktree_launch(cx);
            }
        }
    }

    fn activate_worktree_accessibility_action(
        &mut self,
        action: WorktreeArtifactAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.worktree_semantic_snapshot(cx).is_some_and(|snapshot| {
            snapshot
                .controls
                .iter()
                .any(|control| control.action == action && !control.disabled)
        }) {
            return;
        }
        match action {
            WorktreeArtifactAction::ReviewWorktree => self.review_worktree_choices(window, cx),
            WorktreeArtifactAction::FetchWorktree => self.fetch_worktree_choices(window, cx),
            WorktreeArtifactAction::ConfirmCurrentBase => {
                self.confirm_current_worktree_base(window, cx);
            }
            WorktreeArtifactAction::CreateWorktree => self.create_worktree(window, cx),
            WorktreeArtifactAction::VerifyRecovery => {
                self.verify_recovered_worktree(window, cx);
            }
            WorktreeArtifactAction::ForgetRecovery => self.forget_empty_worktree_recovery(cx),
            WorktreeArtifactAction::SelectPreset(value) => {
                self.select_worktree_preset(PresetId::from_uuid(uuid::Uuid::from_u128(value)), cx)
            }
            WorktreeArtifactAction::StartSession => self.start_worktree_session(window, cx),
            WorktreeArtifactAction::CancelOrCloseWorktree => self.close_worktree_launch(cx),
            WorktreeArtifactAction::SetWorktreeBase
            | WorktreeArtifactAction::SetWorktreeBranch
            | WorktreeArtifactAction::SelectArtifactSession(_)
            | WorktreeArtifactAction::ImportArtifact(_)
            | WorktreeArtifactAction::ShowArtifactList
            | WorktreeArtifactAction::ShowArtifactGrid
            | WorktreeArtifactAction::ConfirmArtifactImport
            | WorktreeArtifactAction::CancelArtifactImport
            | WorktreeArtifactAction::CancelArtifactOperation
            | WorktreeArtifactAction::PreviewArtifact(_)
            | WorktreeArtifactAction::ExportArtifact(_)
            | WorktreeArtifactAction::ToggleArtifactMetadata(_)
            | WorktreeArtifactAction::QuarantineArtifact(_)
            | WorktreeArtifactAction::RestoreArtifact(_)
            | WorktreeArtifactAction::RequestArtifactPurge(_)
            | WorktreeArtifactAction::ConfirmArtifactPurge(_)
            | WorktreeArtifactAction::CancelArtifactPurge => {}
        }
    }

    pub(super) fn open_worktree_launch(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let available = self
            .project_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|summary| summary.project.id == project_id)
            })
            .is_some_and(|summary| summary.status == termirust_domain::ProjectStatus::Available);
        if !available {
            self.error_message = localization::worktree_error_invalid_repository();
            cx.notify();
            return;
        }
        let worktree_id = ManagedWorktreeId::new();
        let branch = generated_worktree_branch(worktree_id);
        let selected_preset_id = self
            .preset_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .presets
                    .iter()
                    .filter(|preset| preset.enabled)
                    .find(|preset| preset.favorite)
                    .or_else(|| snapshot.presets.iter().find(|preset| preset.enabled))
            })
            .map(|preset| preset.id);
        Self::set_input_value(&self.worktree_base_input, "", window, cx);
        Self::set_input_value(
            &self.worktree_branch_input,
            branch.as_str().to_string(),
            window,
            cx,
        );
        self.worktree_launch = Some(WorktreeLaunchUiState {
            source_project_id: project_id,
            worktree_id,
            child_project_id: ProjectId::new(),
            stage: WorktreeLaunchStage::Ready,
            inspection: None,
            selected_preset_id,
            registered_child_id: None,
            error: None,
            generation: 0,
            cancellation: WorktreeCancellation::default(),
            fetched: false,
            current_branch_confirmed: false,
            recovering: false,
        });
        self.start_worktree_inspection(false, false, false, window, cx);
    }

    fn start_worktree_inspection(
        &mut self,
        fetch: bool,
        confirm_current: bool,
        create_after_inspection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.worktree_launch.as_ref() else {
            return;
        };
        if worktree_busy(state.stage) {
            return;
        }
        let requested_base_value = self.worktree_base_input.read(cx).value().trim().to_string();
        let requested_base = if requested_base_value.is_empty() {
            None
        } else {
            match GitReference::new(&requested_base_value) {
                Ok(value) => Some(value),
                Err(error) => return self.fail_worktree_review(error, cx),
            }
        };
        let branch_value = self
            .worktree_branch_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let branch = match GitReference::new(&branch_value) {
            Ok(value) => value,
            Err(error) => return self.fail_worktree_review(error, cx),
        };
        let Some(project) = self
            .project_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|summary| summary.project.id == state.source_project_id)
            })
            .map(|summary| summary.project.clone())
        else {
            return self.fail_worktree_review(WorktreeError::InvalidRepository, cx);
        };
        let managed_root = match managed_agent_worktree_dir() {
            Ok(path) => path,
            Err(_) => return self.fail_worktree_review(WorktreeError::InvalidPath, cx),
        };
        let state = self
            .worktree_launch
            .as_mut()
            .expect("worktree state exists");
        state.cancellation.cancel();
        state.cancellation = WorktreeCancellation::default();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.stage = WorktreeLaunchStage::Inspecting;
        state.inspection = None;
        state.error = None;
        state.fetched = fetch;
        state.current_branch_confirmed = confirm_current;
        let generation = state.generation;
        let cancellation = state.cancellation.clone();
        let worktree_id = state.worktree_id;
        let child_project_id = state.child_project_id;
        let source_project_id = state.source_project_id;
        let draft = WorktreeLaunchDraft {
            source_project_id,
            requested_base,
            fetch,
            confirm_current_branch: confirm_current,
            branch,
            preset_id: state.selected_preset_id,
        };
        let project_coordinator = self.project_coordinator.clone();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    project_coordinator.inspect_worktree(WorktreeInspectionRequest {
                        project_root: project.canonical_root.as_path().to_path_buf(),
                        managed_root,
                        worktree_id,
                        child_project_id,
                        draft,
                        cancellation,
                    })
                })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.finish_worktree_inspection(
                        generation,
                        result,
                        create_after_inspection,
                        window,
                        cx,
                    );
                });
            });
        })
        .detach();
    }

    fn finish_worktree_inspection(
        &mut self,
        generation: u64,
        result: Result<WorktreeInspection, WorktreeError>,
        create_after_inspection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.worktree_launch.as_mut() else {
            return;
        };
        if state.generation != generation {
            return;
        }
        match result {
            Ok(inspection) => {
                state.inspection = Some(inspection);
                state.stage = WorktreeLaunchStage::Ready;
                state.error = None;
                if create_after_inspection {
                    self.begin_worktree_creation(window, cx);
                    return;
                }
            }
            Err(error) => {
                state.inspection = None;
                state.stage = WorktreeLaunchStage::Ready;
                state.error = Some(error);
            }
        }
        cx.notify();
    }

    fn review_worktree_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (fetch, current) = self
            .worktree_launch
            .as_ref()
            .map(|state| (state.fetched, state.current_branch_confirmed))
            .unwrap_or_default();
        self.start_worktree_inspection(fetch, current, false, window, cx);
    }

    fn fetch_worktree_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_worktree_inspection(true, false, false, window, cx);
    }

    fn confirm_current_worktree_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_worktree_inspection(false, true, false, window, cx);
    }

    fn create_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (fetch, current) = self
            .worktree_launch
            .as_ref()
            .map(|state| (state.fetched, state.current_branch_confirmed))
            .unwrap_or_default();
        self.start_worktree_inspection(fetch, current, true, window, cx);
    }

    fn begin_worktree_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((inspection, repository, revision)) = self
            .worktree_launch
            .as_ref()
            .and_then(|state| state.inspection.clone())
            .and_then(|inspection| {
                Some((
                    inspection,
                    self.project_library.repository.clone()?,
                    self.project_library.snapshot.as_ref()?.revision,
                ))
            })
        else {
            return self.fail_worktree_review(WorktreeError::RegistrationConflict, cx);
        };
        let label = child_project_label(&inspection);
        let intent = WorktreeIntent {
            plan: inspection.plan.clone(),
            child_display_name: label,
            state: WorktreeIntentState::Planned,
            revision: termirust_domain::Revision::ZERO,
        };
        let intent = match repository.begin_worktree_intent(intent, revision) {
            Ok(intent) => intent,
            Err(error) => return self.fail_worktree_review(store_worktree_error(error), cx),
        };
        self.project_library.reload();
        let Some(state) = self.worktree_launch.as_mut() else {
            return;
        };
        state.stage = WorktreeLaunchStage::Creating;
        state.error = None;
        let generation = state.generation;
        let cancellation = state.cancellation.clone();
        let plan = inspection.plan;
        let project_coordinator = self.project_coordinator.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    project_coordinator.create_worktree(WorktreePlanRequest { plan, cancellation })
                })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.finish_worktree_creation(generation, intent, result, window, cx);
                });
            });
        })
        .detach();
    }

    fn finish_worktree_creation(
        &mut self,
        generation: u64,
        intent: WorktreeIntent,
        result: Result<(), WorktreeError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.worktree_generation_matches(generation) {
            return;
        }
        if let Err(error) = result {
            self.preserve_worktree_intent(&intent, error, cx);
            return;
        }
        self.verify_and_register_worktree(generation, intent, window, cx);
    }

    fn verify_and_register_worktree(
        &mut self,
        generation: u64,
        intent: WorktreeIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.project_library.repository.clone() else {
            self.preserve_worktree_intent(&intent, WorktreeError::RegistrationConflict, cx);
            return;
        };
        let Some(state) = self.worktree_launch.as_mut() else {
            return;
        };
        state.stage = WorktreeLaunchStage::Verifying;
        state.error = None;
        let cancellation = state.cancellation.clone();
        let plan = intent.plan.clone();
        let project_coordinator = self.project_coordinator.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    project_coordinator.verify_worktree(WorktreePlanRequest {
                        plan: plan.clone(),
                        cancellation,
                    })?;
                    repository
                        .register_worktree_child(plan.id, intent.revision)
                        .map_err(store_worktree_error)
                })
                .await;
            let _ = cx.update(|_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.finish_worktree_verification(generation, intent, result, cx);
                });
            });
        })
        .detach();
    }

    fn finish_worktree_verification(
        &mut self,
        generation: u64,
        intent: WorktreeIntent,
        result: Result<(termirust_domain::Project, WorktreeRegistration), WorktreeError>,
        cx: &mut Context<Self>,
    ) {
        if !self.worktree_generation_matches(generation) {
            return;
        }
        match result {
            Ok((project, _)) => {
                self.project_library.reload();
                self.project_library.selected_id = Some(project.id);
                if let Some(state) = self.worktree_launch.as_mut() {
                    state.stage = WorktreeLaunchStage::Registered;
                    state.registered_child_id = Some(project.id);
                    state.recovering = false;
                    state.error = None;
                }
            }
            Err(error) => self.preserve_worktree_intent(&intent, error, cx),
        }
        cx.notify();
    }

    fn preserve_worktree_intent(
        &mut self,
        intent: &WorktreeIntent,
        error: WorktreeError,
        cx: &mut Context<Self>,
    ) {
        if let Some(repository) = self.project_library.repository.as_ref() {
            let _ =
                repository.mark_worktree_intent_needs_inspection(intent.plan.id, intent.revision);
        }
        self.project_library.reload();
        if let Some(state) = self.worktree_launch.as_mut() {
            state.stage = WorktreeLaunchStage::Ready;
            state.inspection = Some(WorktreeInspection {
                plan: intent.plan.clone(),
                repository_basename: intent
                    .plan
                    .repository_root
                    .as_path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("repository")
                    .to_string(),
                fetched: state.fetched,
                current_branch_fallback: state.current_branch_confirmed,
            });
            state.recovering = true;
            state.error = Some(error);
        }
        cx.notify();
    }

    pub(super) fn review_worktree_recovery(
        &mut self,
        id: ManagedWorktreeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(intent) = self
            .project_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .worktree_intents
                    .iter()
                    .find(|intent| intent.plan.id == id)
            })
            .cloned()
        else {
            self.error_message = localization::worktree_error_conflict();
            cx.notify();
            return;
        };
        Self::set_input_value(&self.worktree_base_input, "", window, cx);
        Self::set_input_value(
            &self.worktree_branch_input,
            intent.plan.generated_branch.as_str().to_string(),
            window,
            cx,
        );
        self.worktree_launch = Some(WorktreeLaunchUiState {
            source_project_id: intent.plan.source_project_id,
            worktree_id: intent.plan.id,
            child_project_id: intent.plan.child_project_id,
            stage: WorktreeLaunchStage::Ready,
            inspection: Some(WorktreeInspection {
                repository_basename: intent
                    .plan
                    .repository_root
                    .as_path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("repository")
                    .to_string(),
                plan: intent.plan,
                fetched: false,
                current_branch_fallback: false,
            }),
            selected_preset_id: self
                .preset_library
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.presets.iter().find(|preset| preset.enabled))
                .map(|preset| preset.id),
            registered_child_id: None,
            error: None,
            generation: 1,
            cancellation: WorktreeCancellation::default(),
            fetched: false,
            current_branch_confirmed: false,
            recovering: true,
        });
        cx.notify();
    }

    fn verify_recovered_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.worktree_launch.as_ref().map(|state| state.worktree_id) else {
            return;
        };
        let Some(intent) = self
            .project_library
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .worktree_intents
                    .iter()
                    .find(|intent| intent.plan.id == id)
            })
            .cloned()
        else {
            return self.fail_worktree_review(WorktreeError::RegistrationConflict, cx);
        };
        let generation = self
            .worktree_launch
            .as_ref()
            .map(|state| state.generation)
            .unwrap_or_default();
        self.verify_and_register_worktree(generation, intent, window, cx);
    }

    fn forget_empty_worktree_recovery(&mut self, cx: &mut Context<Self>) {
        let Some((id, path)) = self
            .worktree_launch
            .as_ref()
            .and_then(|state| state.inspection.as_ref())
            .map(|inspection| {
                (
                    inspection.plan.id,
                    inspection.plan.managed_path.as_path().to_path_buf(),
                )
            })
        else {
            return;
        };
        if !matches!(
            fs::symlink_metadata(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ) {
            self.fail_worktree_review(WorktreeError::RegistrationConflict, cx);
            return;
        }
        let Some(repository) = self.project_library.repository.as_ref() else {
            return;
        };
        let Some(revision) = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
        else {
            return;
        };
        match repository.cancel_worktree_intent(id, revision) {
            Ok(()) => {
                self.project_library.reload();
                self.worktree_launch = None;
            }
            Err(error) => self.fail_worktree_review(store_worktree_error(error), cx),
        }
        cx.notify();
    }

    fn select_worktree_preset(&mut self, preset_id: PresetId, cx: &mut Context<Self>) {
        if let Some(state) = self.worktree_launch.as_mut()
            && !worktree_busy(state.stage)
        {
            state.selected_preset_id = Some(preset_id);
            cx.notify();
        }
    }

    fn start_worktree_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project_id, preset_id)) = self
            .worktree_launch
            .as_ref()
            .and_then(|state| Some((state.registered_child_id?, state.selected_preset_id?)))
        else {
            return;
        };
        self.worktree_launch = None;
        self.open_new_session_with_preset(project_id, preset_id, window, cx);
    }

    pub(super) fn close_worktree_launch(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.worktree_launch.as_ref() else {
            return;
        };
        if worktree_busy(state.stage) {
            state.cancellation.cancel();
            cx.notify();
        } else {
            self.worktree_launch = None;
            cx.notify();
        }
    }

    fn worktree_generation_matches(&self, generation: u64) -> bool {
        self.worktree_launch
            .as_ref()
            .is_some_and(|state| state.generation == generation)
    }

    fn fail_worktree_review(&mut self, error: WorktreeError, cx: &mut Context<Self>) {
        if let Some(state) = self.worktree_launch.as_mut() {
            state.stage = WorktreeLaunchStage::Ready;
            state.error = Some(error);
        } else {
            self.error_message = worktree_error_message(&error);
        }
        cx.notify();
    }

    pub(super) fn render_worktree_launch_sheet(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.worktree_launch.as_ref() else {
            return div().into_any_element();
        };
        let busy = worktree_busy(state.stage);
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let stage = worktree_stage_message(state.stage);
        let presets = self
            .preset_library
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .presets
                    .iter()
                    .filter(|preset| preset.enabled)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let inspection = state.inspection.as_ref();
        let path_missing = inspection.is_some_and(|inspection| {
            matches!(
                fs::symlink_metadata(inspection.plan.managed_path.as_path()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        });

        div()
            .id("worktree-launch-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(theme::SPACE_4))
            .bg(theme::modal_scrim())
            .child(
                v_flex()
                    .id("worktree-launch-sheet")
                    .w(relative(0.96))
                    .max_w(px(theme::DIALOG_MAX_WIDTH))
                    .max_h(relative(0.92))
                    .overflow_y_scrollbar()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_start()
                            .gap(px(theme::SPACE_4))
                            .p(px(theme::SPACE_5))
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                            .font_semibold()
                                            .child(localization::worktree_title()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::worktree_subtitle()),
                                    ),
                            )
                            .child(
                                Button::new("worktree-launch-close")
                                    .icon(IconName::Close)
                                    .disabled(busy)
                                    .tooltip(localization::common_close())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_worktree_launch(cx);
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .p(px(theme::SPACE_5))
                            .gap(px(theme::SPACE_5))
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_3))
                                    .text_color(if state.error.is_some() {
                                        theme::warning()
                                    } else {
                                        theme::text_main()
                                    })
                                    .child(
                                        Icon::new(if busy {
                                            IconName::LoaderCircle
                                        } else if state.error.is_some() {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::Check
                                        })
                                        .size(px(theme::ICON_SIZE_DEFAULT)),
                                    )
                                    .child(stage),
                            )
                            .when_some(state.error.as_ref(), |this, error| {
                                this.child(
                                    v_flex()
                                        .gap(px(theme::SPACE_2))
                                        .p(px(theme::SPACE_4))
                                        .rounded(px(theme::CARD_RADIUS))
                                        .bg(theme::accent_soft())
                                        .text_color(theme::warning())
                                        .child(worktree_error_message(error))
                                        .when(state.recovering, |this| {
                                            this.child(localization::worktree_failure_kept())
                                        }),
                                )
                            })
                            .when_some(inspection, |this, inspection| {
                                this.child(review_row(
                                    localization::worktree_repository_field(),
                                    if recording_friendly {
                                        localization::product_private_project_row()
                                    } else {
                                        inspection.repository_basename.clone()
                                    },
                                ))
                                .child(review_row(
                                    localization::worktree_base_field(),
                                    if recording_friendly {
                                        localization::worktree_private_reference()
                                    } else {
                                        format!(
                                            "{} · {}",
                                            inspection.plan.selected_base.ref_name,
                                            inspection.plan.selected_base.commit_oid.short()
                                        )
                                    },
                                ))
                                .child(review_row(
                                    localization::worktree_path_field(),
                                    if recording_friendly {
                                        localization::worktree_private_path()
                                    } else {
                                        managed_path_preview(&inspection.plan)
                                    },
                                ))
                            })
                            .when(!state.recovering && state.stage != WorktreeLaunchStage::Registered, |this| {
                                this.child(
                                    v_flex()
                                        .gap(px(theme::SPACE_2))
                                        .child(field_label(localization::worktree_base_field()))
                                        .child(Input::new(&self.worktree_base_input)),
                                )
                                .child(
                                    v_flex()
                                        .gap(px(theme::SPACE_2))
                                        .child(field_label(localization::worktree_branch_field()))
                                        .child(Input::new(&self.worktree_branch_input)),
                                )
                                .child(
                                    h_flex()
                                        .flex_wrap()
                                        .gap(px(theme::SPACE_2))
                                        .child(
                                            Button::new("worktree-review")
                                                .icon(IconName::Redo2)
                                                .label(localization::worktree_refresh_action())
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.review_worktree_choices(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("worktree-fetch")
                                                .icon(IconName::ArrowDown)
                                                .label(localization::worktree_fetch_action())
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.fetch_worktree_choices(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("worktree-current")
                                                .label(localization::worktree_current_action())
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.confirm_current_worktree_base(window, cx);
                                                })),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(theme::SPACE_2))
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(if state.fetched || state.current_branch_confirmed {
                                            theme::warning()
                                        } else {
                                            theme::text_muted()
                                        })
                                        .child(Icon::new(if state.fetched || state.current_branch_confirmed {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::Info
                                        }).size(px(theme::TYPE_BODY_SMALL_SIZE)))
                                        .child(if state.fetched {
                                            localization::worktree_fetched_status()
                                        } else if state.current_branch_confirmed {
                                            localization::worktree_current_warning()
                                        } else {
                                            localization::worktree_offline_status()
                                        }),
                                )
                            })
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(field_label(localization::worktree_preset_field()))
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap(px(theme::SPACE_2))
                                            .children(presets.iter().map(|preset| {
                                                let preset_id = preset.id;
                                                Button::new((
                                                    "worktree-preset",
                                                    preset_id.as_uuid().as_u128() as u64,
                                                ))
                                                .small()
                                                .label(if recording_friendly {
                                                    localization::preset_private_row()
                                                } else {
                                                    preset.label.as_str().to_string()
                                                })
                                                .selected(state.selected_preset_id == Some(preset_id))
                                                .disabled(busy)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_worktree_preset(preset_id, cx);
                                                }))
                                            })),
                                    ),
                            )
                            .when(state.stage == WorktreeLaunchStage::Registered, |this| {
                                this.child(
                                    h_flex()
                                        .gap(px(theme::SPACE_2))
                                        .text_color(theme::success())
                                        .child(Icon::new(IconName::Check).size(px(theme::ICON_SIZE_DEFAULT)))
                                        .child(localization::worktree_success()),
                                )
                            })
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap(px(theme::SPACE_3))
                                    .when(!state.recovering && state.stage != WorktreeLaunchStage::Registered, |this| {
                                        this.child(
                                            Button::new("worktree-create")
                                                .primary()
                                                .icon(IconName::Plus)
                                                .label(localization::worktree_create_action())
                                                .disabled(busy || inspection.is_none())
                                                .tooltip(if inspection.is_none() {
                                                    localization::worktree_create_reason()
                                                } else {
                                                    localization::worktree_create_action()
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.create_worktree(window, cx);
                                                })),
                                        )
                                    })
                                    .when(state.recovering, |this| {
                                        this.child(
                                            Button::new("worktree-recover-register")
                                                .primary()
                                                .label(localization::worktree_verify_action())
                                                .disabled(busy || path_missing)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.verify_recovered_worktree(window, cx);
                                                })),
                                        )
                                        .when(path_missing, |this| {
                                            this.child(
                                                Button::new("worktree-forget-empty")
                                                    .label(localization::worktree_forget_recovery_action())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.forget_empty_worktree_recovery(cx);
                                                    })),
                                            )
                                        })
                                    })
                                    .when(state.stage == WorktreeLaunchStage::Registered, |this| {
                                        this.child(
                                            Button::new("worktree-start-session")
                                                .primary()
                                                .icon(IconName::SquareTerminal)
                                                .label(localization::worktree_start_session_action())
                                                .disabled(state.selected_preset_id.is_none())
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.start_worktree_session(window, cx);
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("worktree-cancel")
                                            .label(if busy {
                                                localization::common_cancel()
                                            } else {
                                                localization::common_close()
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.close_worktree_launch(cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn worktree_button(
    action: WorktreeArtifactAction,
    name: MessageId,
    disabled: bool,
) -> WorktreeArtifactControl {
    WorktreeArtifactControl {
        action,
        parent: None,
        role: WorktreeArtifactControlRole::Button,
        name,
        value: None,
        selected: false,
        disabled,
        invalid: false,
    }
}

fn worktree_text_control(
    action: WorktreeArtifactAction,
    name: MessageId,
    value: String,
    disabled: bool,
) -> WorktreeArtifactControl {
    WorktreeArtifactControl {
        action,
        parent: None,
        role: WorktreeArtifactControlRole::TextField,
        name,
        value: Some(value),
        selected: false,
        disabled,
        invalid: false,
    }
}

fn worktree_surface_state(state: &WorktreeLaunchUiState) -> WorktreeArtifactSurfaceState {
    if state.recovering {
        return if state.error.is_some() {
            WorktreeArtifactSurfaceState::UnknownCompletion
        } else {
            WorktreeArtifactSurfaceState::Recovery
        };
    }
    if let Some(error) = state.error.as_ref() {
        return match error {
            WorktreeError::FetchFailed => WorktreeArtifactSurfaceState::Offline,
            WorktreeError::PermissionDenied => WorktreeArtifactSurfaceState::PermissionDenied,
            WorktreeError::StorageFull => WorktreeArtifactSurfaceState::DiskFull,
            WorktreeError::Timeout => WorktreeArtifactSurfaceState::Timeout,
            WorktreeError::Cancelled => WorktreeArtifactSurfaceState::Cancelled,
            WorktreeError::GitUnavailable => WorktreeArtifactSurfaceState::Unavailable,
            WorktreeError::OutputLimit | WorktreeError::ResourceLimit { .. } => {
                WorktreeArtifactSurfaceState::Quota
            }
            WorktreeError::InvalidReference
            | WorktreeError::InvalidOid
            | WorktreeError::InvalidPath => WorktreeArtifactSurfaceState::Malformed,
            WorktreeError::VerificationMismatch
            | WorktreeError::Containment
            | WorktreeError::SymlinkSwap
            | WorktreeError::RegistrationConflict
            | WorktreeError::Store { .. } => WorktreeArtifactSurfaceState::Recovery,
            WorktreeError::DirtySource
            | WorktreeError::SubmodulesUnsupported
            | WorktreeError::DetachedHead => WorktreeArtifactSurfaceState::RiskReview,
            WorktreeError::InvalidRepository
            | WorktreeError::NoBase
            | WorktreeError::BranchCollision
            | WorktreeError::PathCollision
            | WorktreeError::GitFailed { .. } => WorktreeArtifactSurfaceState::Error,
        };
    }
    match state.stage {
        WorktreeLaunchStage::Inspecting => WorktreeArtifactSurfaceState::Inspecting,
        WorktreeLaunchStage::Ready => WorktreeArtifactSurfaceState::Ready,
        WorktreeLaunchStage::Creating => WorktreeArtifactSurfaceState::Creating,
        WorktreeLaunchStage::Verifying | WorktreeLaunchStage::Launching => {
            WorktreeArtifactSurfaceState::Verifying
        }
        WorktreeLaunchStage::Registered => WorktreeArtifactSurfaceState::Registered,
    }
}

fn worktree_stage_message_id(stage: WorktreeLaunchStage) -> MessageId {
    match stage {
        WorktreeLaunchStage::Inspecting => MessageId::WorktreeStageInspecting,
        WorktreeLaunchStage::Ready => MessageId::WorktreeStageReady,
        WorktreeLaunchStage::Creating => MessageId::WorktreeStageCreating,
        WorktreeLaunchStage::Verifying => MessageId::WorktreeStageVerifying,
        WorktreeLaunchStage::Registered => MessageId::WorktreeStageRegistered,
        WorktreeLaunchStage::Launching => MessageId::WorktreeStageLaunching,
    }
}

fn worktree_stage_progress(stage: WorktreeLaunchStage) -> u64 {
    match stage {
        WorktreeLaunchStage::Inspecting => 1,
        WorktreeLaunchStage::Creating => 2,
        WorktreeLaunchStage::Verifying => 3,
        WorktreeLaunchStage::Registered | WorktreeLaunchStage::Launching => 4,
        WorktreeLaunchStage::Ready => 0,
    }
}

fn child_project_label(inspection: &WorktreeInspection) -> String {
    let branch = inspection
        .plan
        .generated_branch
        .as_str()
        .rsplit('/')
        .next()
        .unwrap_or("isolated");
    format!("{} · {branch}", inspection.repository_basename)
        .chars()
        .take(termirust_domain::MAX_LABEL_SCALARS)
        .collect()
}

fn managed_path_preview(plan: &termirust_domain::WorktreePlan) -> String {
    let path = plan.managed_path.as_path();
    let parent = path
        .parent()
        .and_then(|value| value.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("repository");
    let child = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("worktree");
    format!("{parent}/{child}")
}

fn worktree_busy(stage: WorktreeLaunchStage) -> bool {
    matches!(
        stage,
        WorktreeLaunchStage::Inspecting
            | WorktreeLaunchStage::Creating
            | WorktreeLaunchStage::Verifying
            | WorktreeLaunchStage::Launching
    )
}

fn worktree_stage_message(stage: WorktreeLaunchStage) -> String {
    match stage {
        WorktreeLaunchStage::Inspecting => localization::worktree_stage_inspecting(),
        WorktreeLaunchStage::Ready => localization::worktree_stage_ready(),
        WorktreeLaunchStage::Creating => localization::worktree_stage_creating(),
        WorktreeLaunchStage::Verifying => localization::worktree_stage_verifying(),
        WorktreeLaunchStage::Registered => localization::worktree_stage_registered(),
        WorktreeLaunchStage::Launching => localization::worktree_stage_launching(),
    }
}

fn worktree_error_message(error: &WorktreeError) -> String {
    match error {
        WorktreeError::InvalidRepository => localization::worktree_error_invalid_repository(),
        WorktreeError::GitUnavailable => localization::worktree_error_git_unavailable(),
        WorktreeError::FetchFailed => localization::worktree_error_fetch(),
        WorktreeError::PermissionDenied => localization::worktree_error_permission(),
        WorktreeError::StorageFull => localization::worktree_error_storage_full(),
        WorktreeError::DirtySource => localization::worktree_error_dirty_source(),
        WorktreeError::SubmodulesUnsupported => localization::worktree_error_submodules(),
        WorktreeError::NoBase | WorktreeError::DetachedHead => {
            localization::worktree_error_no_base()
        }
        WorktreeError::BranchCollision | WorktreeError::PathCollision => {
            localization::worktree_error_collision()
        }
        WorktreeError::Timeout => localization::worktree_error_timeout(),
        WorktreeError::Cancelled => localization::worktree_error_cancelled(),
        WorktreeError::VerificationMismatch
        | WorktreeError::Containment
        | WorktreeError::SymlinkSwap => localization::worktree_error_verification(),
        WorktreeError::RegistrationConflict | WorktreeError::Store { .. } => {
            localization::worktree_error_conflict()
        }
        WorktreeError::InvalidReference | WorktreeError::InvalidOid => {
            localization::worktree_error_invalid_reference()
        }
        WorktreeError::OutputLimit | WorktreeError::ResourceLimit { .. } => {
            localization::worktree_error_resource_limit()
        }
        WorktreeError::InvalidPath | WorktreeError::GitFailed { .. } => {
            localization::worktree_error_generic()
        }
    }
}

fn store_worktree_error(error: StoreError) -> WorktreeError {
    match error {
        StoreError::WorktreeDomain(error) => error,
        StoreError::Domain(termirust_domain::ProjectError::StaleRevision { .. }) => {
            WorktreeError::RegistrationConflict
        }
        StoreError::Domain(termirust_domain::ProjectError::ResourceLimit { limit }) => {
            WorktreeError::ResourceLimit { limit }
        }
        StoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        } => WorktreeError::PermissionDenied,
        StoreError::Io {
            kind: std::io::ErrorKind::StorageFull,
            ..
        } => WorktreeError::StorageFull,
        _ => WorktreeError::Store {
            code: "project-store",
        },
    }
}

fn field_label(label: String) -> AnyElement {
    div()
        .font_semibold()
        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
        .text_color(theme::text_main())
        .child(label)
        .into_any_element()
}

fn review_row(label: String, value: String) -> AnyElement {
    v_flex()
        .gap(px(theme::SPACE_2))
        .child(field_label(label))
        .child(
            div()
                .min_w_0()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .text_color(theme::text_muted())
                .child(value),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::WorktreePlan;

    #[test]
    fn every_stage_and_error_has_localized_visible_copy() {
        for stage in [
            WorktreeLaunchStage::Inspecting,
            WorktreeLaunchStage::Ready,
            WorktreeLaunchStage::Creating,
            WorktreeLaunchStage::Verifying,
            WorktreeLaunchStage::Registered,
            WorktreeLaunchStage::Launching,
        ] {
            assert!(!worktree_stage_message(stage).is_empty());
        }
        for error in [
            WorktreeError::DirtySource,
            WorktreeError::SubmodulesUnsupported,
            WorktreeError::GitUnavailable,
            WorktreeError::FetchFailed,
            WorktreeError::PermissionDenied,
            WorktreeError::StorageFull,
            WorktreeError::InvalidReference,
            WorktreeError::OutputLimit,
            WorktreeError::ResourceLimit { limit: 1 },
            WorktreeError::NoBase,
            WorktreeError::BranchCollision,
            WorktreeError::Timeout,
            WorktreeError::Cancelled,
            WorktreeError::VerificationMismatch,
            WorktreeError::RegistrationConflict,
            WorktreeError::GitFailed { code: "fixture" },
        ] {
            let message = worktree_error_message(&error);
            assert!(!message.is_empty());
            assert!(!message.contains("fixture"));
        }
    }

    #[test]
    fn child_label_is_bounded_and_uses_safe_basename_only() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = fixture.path().join("repository");
        let managed = fixture.path().join("managed");
        fs::create_dir(&repository).unwrap();
        fs::create_dir(&managed).unwrap();
        let id = ManagedWorktreeId::new();
        let canonical_managed = termirust_domain::CanonicalPath::resolve(&managed).unwrap();
        let inspection = WorktreeInspection {
            plan: WorktreePlan::new(
                id,
                ProjectId::new(),
                ProjectId::new(),
                termirust_domain::CanonicalPath::resolve(&repository).unwrap(),
                canonical_managed.clone(),
                termirust_domain::BaseCandidate {
                    ref_name: GitReference::new("main").unwrap(),
                    commit_oid: termirust_domain::CommitOid::new(&"a".repeat(40)).unwrap(),
                    source: termirust_domain::BaseSource::ConfiguredMainline,
                },
                GitReference::new("termirust/worktree/feature").unwrap(),
                termirust_domain::ManagedPath::new(canonical_managed.as_path().join("child"))
                    .unwrap(),
            )
            .unwrap(),
            repository_basename: "Repository".repeat(100),
            fetched: false,
            current_branch_fallback: false,
        };
        let label = child_project_label(&inspection);
        let preview = managed_path_preview(&inspection.plan);
        assert!(label.chars().count() <= termirust_domain::MAX_LABEL_SCALARS);
        assert!(!label.contains(fixture.path().to_string_lossy().as_ref()));
        assert!(preview.starts_with("managed/child"));
        assert!(!preview.contains(fixture.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn busy_state_is_cancelable_and_registered_is_not_busy() {
        assert!(worktree_busy(WorktreeLaunchStage::Inspecting));
        assert!(worktree_busy(WorktreeLaunchStage::Creating));
        assert!(worktree_busy(WorktreeLaunchStage::Verifying));
        assert!(!worktree_busy(WorktreeLaunchStage::Ready));
        assert!(!worktree_busy(WorktreeLaunchStage::Registered));
    }
}
