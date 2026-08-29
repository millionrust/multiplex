use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Styled, Window, div,
    px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use termirust_domain::{
    CommandId, ContinuityLink, HostedSessionId, HostedSessionState, OutputSequence,
    PermissionPolicy, ResumeError, ResumePlan, ResumeRequest, RuntimeCapability,
    RuntimeCapabilitySet, RuntimeDetectionResult, RuntimeDetectionStatus, SessionLaunchRoute,
    TitleSource, evaluate_resume,
};
use termirust_store::ContinuityRepository;

use super::hosted_session::{DurableContinuityCommit, DurableLaunch, DurableSessionPaths};
use super::session_coordinator::SessionStartRequest;
use super::{AppAttachedPaneState, TermiRustApp, theme};
use crate::agents::{
    ResumeValidationCancellation, build_codex_resume_plan, discover_codex_conversation_handle,
};
use crate::models::{ConnectRequest, LocalShellConfig, SavedAppAttachedSession, SavedDurableHost};
use crate::storage::{app_dir, project_store_dir, save_saved_state};
use crate::ui::localization;
use crate::ui::util::current_unix_millis;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionResumePhase {
    Validating,
    Review,
    Starting,
    Failed,
}

pub(super) struct SessionResumeState {
    source_session_id: HostedSessionId,
    replacement_session_id: HostedSessionId,
    command_id: CommandId,
    phase: SessionResumePhase,
    plan: Option<ResumePlan>,
    cancellation: ResumeValidationCancellation,
    error: Option<ResumeError>,
    spawned_pane_id: Option<u64>,
    source_title: String,
    project_label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SessionResumeProjection {
    pub visible: bool,
    pub enabled: bool,
    pub reason: Option<ResumeError>,
}

impl SessionResumeProjection {
    fn hidden() -> Self {
        Self {
            visible: false,
            enabled: false,
            reason: None,
        }
    }
}

pub(super) fn session_resume_projection(
    saved: &SavedAppAttachedSession,
    metadata: &termirust_domain::HostedSession,
) -> SessionResumeProjection {
    let Some(host) = saved.durable_host.as_ref() else {
        return SessionResumeProjection::hidden();
    };
    let Some(recognition) = host.runtime_recognition.as_ref() else {
        return SessionResumeProjection::hidden();
    };
    let Some(occupant) = recognition.occupant.as_ref() else {
        return SessionResumeProjection::hidden();
    };
    if occupant.runtime_id.as_str() != "codex" {
        return SessionResumeProjection::hidden();
    }
    let request = ResumeRequest {
        command_id: CommandId::new(),
        session_id: saved.id,
        expected_generation: occupant.generation,
        expected_revision: metadata.revision,
    };
    match evaluate_resume(
        request,
        metadata,
        Some(recognition),
        host.conversation_handle.clone(),
    ) {
        Ok(_) | Err(ResumeError::ConversationMissing) => SessionResumeProjection {
            visible: true,
            enabled: true,
            reason: None,
        },
        Err(error) => SessionResumeProjection {
            visible: true,
            enabled: false,
            reason: Some(error),
        },
    }
}

impl TermiRustApp {
    pub(super) fn open_session_resume(
        &mut self,
        source_session_id: HostedSessionId,
        cx: &mut Context<Self>,
    ) {
        if self.session_resume.is_some() || self.new_session.is_some() {
            return;
        }
        let Some(metadata) = self.session_library.session(source_session_id).cloned() else {
            self.error_message = localization::session_library_operation_failed();
            cx.notify();
            return;
        };
        let Some(source) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == source_session_id)
            .cloned()
        else {
            self.error_message = localization::session_library_operation_failed();
            cx.notify();
            return;
        };
        let Some(host) = source.durable_host.as_ref().cloned() else {
            self.error_message = resume_error_message(ResumeError::OwnershipUnproven);
            cx.notify();
            return;
        };
        let Some(recognition) = host.runtime_recognition.clone() else {
            self.error_message = resume_error_message(ResumeError::OwnershipUnproven);
            cx.notify();
            return;
        };
        let Some(occupant) = recognition.occupant.as_ref() else {
            self.error_message = resume_error_message(ResumeError::OwnershipUnproven);
            cx.notify();
            return;
        };
        let Some(working_directory) = host.working_directory.as_deref().map(PathBuf::from) else {
            self.error_message = resume_error_message(ResumeError::ConversationMalformed);
            cx.notify();
            return;
        };
        let Some(executable) = host.executable.as_deref().map(PathBuf::from) else {
            self.error_message = resume_error_message(ResumeError::ProviderUnavailable);
            cx.notify();
            return;
        };
        let Some(conversation_root) = codex_conversation_root() else {
            self.error_message = resume_error_message(ResumeError::ConversationMissing);
            cx.notify();
            return;
        };

        let command_id = CommandId::new();
        let replacement_session_id = HostedSessionId::new();
        let request = ResumeRequest {
            command_id,
            session_id: source_session_id,
            expected_generation: occupant.generation,
            expected_revision: metadata.revision,
        };
        let cancellation = ResumeValidationCancellation::new();
        self.session_resume = Some(SessionResumeState {
            source_session_id,
            replacement_session_id,
            command_id,
            phase: SessionResumePhase::Validating,
            plan: None,
            cancellation: cancellation.clone(),
            error: None,
            spawned_pane_id: None,
            source_title: source.title.clone(),
            project_label: source.project_label.clone(),
        });
        self.error_message.clear();
        cx.notify();

        let handle = host.conversation_handle;
        let canonical_project = source.origin.project_id;
        let permission_policy = host.permission_policy;
        let not_before_millis = source.started_at.saturating_sub(5_000);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let handle = match handle {
                        Some(handle) => handle,
                        None => discover_codex_conversation_handle(
                            &conversation_root,
                            &working_directory,
                            not_before_millis,
                            &cancellation,
                        )?,
                    };
                    let candidate =
                        evaluate_resume(request, &metadata, Some(&recognition), Some(handle))?;
                    build_codex_resume_plan(
                        candidate,
                        &conversation_root,
                        canonical_project,
                        &working_directory,
                        permission_policy,
                        &executable,
                        replacement_session_id,
                        &cancellation,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_session_resume_validation(command_id, result, cx);
            });
        })
        .detach();
    }

    fn finish_session_resume_validation(
        &mut self,
        command_id: CommandId,
        result: Result<ResumePlan, ResumeError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session_resume.as_mut() else {
            return;
        };
        if state.command_id != command_id || state.phase != SessionResumePhase::Validating {
            return;
        }
        match result {
            Ok(plan)
                if self
                    .session_library
                    .session(state.source_session_id)
                    .is_some_and(|source| {
                        source.revision == plan.candidate.request.expected_revision
                    }) =>
            {
                if let Some(host) = self
                    .saved
                    .app_attached_sessions
                    .iter_mut()
                    .find(|session| session.id == state.source_session_id)
                    .and_then(|session| session.durable_host.as_mut())
                {
                    host.conversation_handle = Some(plan.candidate.handle.clone());
                    let _ = save_saved_state(&self.saved);
                }
                state.plan = Some(plan);
                state.phase = SessionResumePhase::Review;
                state.error = None;
            }
            Ok(_) => {
                state.phase = SessionResumePhase::Failed;
                state.error = Some(ResumeError::StaleRevision);
            }
            Err(error) => {
                state.phase = SessionResumePhase::Failed;
                state.error = Some(error);
            }
        }
        cx.notify();
    }

    pub(super) fn start_session_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((source_session_id, command_id, plan)) = self
            .session_resume
            .as_ref()
            .filter(|state| state.phase == SessionResumePhase::Review)
            .and_then(|state| {
                state
                    .plan
                    .clone()
                    .map(|plan| (state.source_session_id, state.command_id, plan))
            })
        else {
            return;
        };
        let Some(metadata) = self.session_library.session(source_session_id) else {
            return self.fail_session_resume(ResumeError::StaleRevision, cx);
        };
        let Some(source) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == source_session_id)
            .cloned()
        else {
            return self.fail_session_resume(ResumeError::StaleRevision, cx);
        };
        let Some(host) = source.durable_host.as_ref() else {
            return self.fail_session_resume(ResumeError::OwnershipUnproven, cx);
        };
        if metadata.revision != plan.candidate.request.expected_revision
            || host.conversation_handle.as_ref() != Some(&plan.candidate.handle)
        {
            return self.fail_session_resume(ResumeError::StaleRevision, cx);
        }
        let store_root = match project_store_dir() {
            Ok(root) => root,
            Err(_) => return self.fail_session_resume(ResumeError::PermissionDenied, cx),
        };
        let continuity = match ContinuityRepository::open(&store_root)
            .and_then(|repository| repository.load())
        {
            Ok(snapshot) => snapshot,
            Err(_) => return self.fail_session_resume(ResumeError::ContinuityConflict, cx),
        };
        let paths = match app_dir().and_then(|directory| {
            DurableSessionPaths::create(&directory, plan.replacement_session_id).map_err(Into::into)
        }) {
            Ok(paths) => paths,
            Err(_) => return self.fail_session_resume(ResumeError::PermissionDenied, cx),
        };
        let replacement_generation = plan.candidate.prior_generation.next();
        let link = ContinuityLink {
            command_id,
            source_session_id,
            replacement_session_id: plan.replacement_session_id,
            runtime_id: plan.candidate.runtime_id.clone(),
            prior_generation: plan.candidate.prior_generation,
            replacement_generation,
            committed_at: current_unix_millis(),
        };
        let detection = RuntimeDetectionResult {
            runtime_id: plan.candidate.runtime_id.clone(),
            descriptor_version: 1,
            status: RuntimeDetectionStatus::Available,
            fingerprint: Some(plan.candidate.expected_executable_fingerprint),
            safe_version: Some("0.150.1".to_string()),
            capabilities: RuntimeCapabilitySet::new([
                RuntimeCapability::InteractivePty,
                RuntimeCapability::Cancellation,
                RuntimeCapability::Resume,
            ]),
            diagnostic_code: None,
        };
        let now = current_unix_millis();
        let position = self
            .saved
            .next_app_attached_session_position(source.origin.project_id, source.group_id);
        let record = SavedAppAttachedSession {
            id: plan.replacement_session_id,
            route: SessionLaunchRoute::DurableHost,
            origin: source.origin,
            state: HostedSessionState::Provisioning,
            project_label: source.project_label.clone(),
            preset_label: source.preset_label.clone(),
            title: source.title.clone(),
            title_source: if source.title_source == TitleSource::Manual {
                TitleSource::Manual
            } else {
                TitleSource::Imported
            },
            activity: termirust_domain::ActivityAggregate {
                generation: replacement_generation,
                ..termirust_domain::ActivityAggregate::default()
            },
            pinned: source.pinned,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: termirust_domain::Revision::ZERO,
            durable_host: Some(SavedDurableHost {
                runtime_root: paths.runtime_root.to_string_lossy().into_owned(),
                session_dir: paths.session_dir.to_string_lossy().into_owned(),
                working_directory: Some(plan.working_directory.to_string_lossy().into_owned()),
                last_sequence: 0,
                durable_sequence: 0,
                runtime_recognition: None,
                conversation_handle: Some(plan.candidate.handle.clone()),
                executable: Some(plan.executable.to_string_lossy().into_owned()),
                permission_policy: plan.permission_policy,
                continuity_source_id: None,
            }),
            group_id: source.group_id,
            position,
            started_at: now,
            updated_at: now,
        };
        if self
            .session_library
            .create_from_saved(&mut self.saved, record)
            .is_err()
        {
            return self.fail_session_resume(ResumeError::ContinuityConflict, cx);
        }
        if let Err(error) = save_saved_state(&self.saved) {
            eprintln!("[session-resume] compatibility projection save failed: {error:#}");
        }

        let pane_id = self.next_session_id();
        let local_config = LocalShellConfig {
            program: plan.executable.to_string_lossy().into_owned(),
            args: plan.arguments.clone(),
            cwd: Some(plan.working_directory.to_string_lossy().into_owned()),
        };
        let mut request = ConnectRequest::local_shell_with_config(pane_id, local_config);
        request.title = localization::session_resume_workspace_title(&source.title);
        let runtime = self.session_coordinator.start(SessionStartRequest::launch(
            pane_id,
            plan.replacement_session_id,
            paths,
            DurableLaunch {
                executable: plan.executable.clone(),
                arguments: plan.arguments.clone(),
                cwd: plan.working_directory.clone(),
                runtime_detection: Some(detection),
            },
            Some(replacement_generation),
            Some(DurableContinuityCommit {
                store_root,
                expected_revision: continuity.revision,
                link,
            }),
        ));
        let terminal_focus = cx.focus_handle().tab_stop(true);
        self.register_pane(request.clone(), runtime, terminal_focus);
        self.open_spawned_pane_workspace(&request, pane_id);
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.app_attached = Some(AppAttachedPaneState {
                hosted_session_id: plan.replacement_session_id,
                route: SessionLaunchRoute::DurableHost,
                origin: source.origin,
                pending_initial_input: None,
                cancel_requested: false,
                last_sequence: 0,
                has_writer_lease: false,
                dev_urls: termirust_client::DevUrlProjection::new(plan.replacement_session_id),
            });
            pane.status = localization::session_resume_phase_starting();
            pane.terminal_focus.focus(window);
        }
        if let Some(state) = self.session_resume.as_mut() {
            state.phase = SessionResumePhase::Starting;
            state.spawned_pane_id = Some(pane_id);
        }
        self.status_message = localization::session_resume_phase_starting();
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    pub(super) fn cancel_session_resume(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.session_resume.take() else {
            return;
        };
        state.cancellation.cancel();
        if state.phase == SessionResumePhase::Starting
            && let Some(pane_id) = state.spawned_pane_id
            && let Some(pane) = self.pane_mut(pane_id)
        {
            pane.user_closed = true;
            if let Some(attached) = pane.app_attached.as_mut() {
                attached.cancel_requested = true;
            }
            let _ = pane
                .runtime
                .command_tx
                .send(crate::ssh::SessionCommand::Disconnect);
        }
        self.status_message = localization::session_resume_cancelled();
        cx.notify();
    }

    fn fail_session_resume(&mut self, error: ResumeError, cx: &mut Context<Self>) {
        if let Some(state) = self.session_resume.as_mut() {
            state.phase = SessionResumePhase::Failed;
            state.error = Some(error);
        }
        cx.notify();
    }

    pub(super) fn complete_session_resume_ready(&mut self, session_id: HostedSessionId) -> bool {
        if let Some(source_session_id) = self.session_resume.as_ref().and_then(|state| {
            (state.replacement_session_id == session_id).then_some(state.source_session_id)
        }) {
            if let Some(host) = self
                .saved
                .app_attached_sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .and_then(|session| session.durable_host.as_mut())
            {
                host.continuity_source_id = Some(source_session_id);
                let _ = save_saved_state(&self.saved);
            }
            self.session_resume = None;
            self.status_message = localization::session_resume_ready();
            return true;
        }
        false
    }

    pub(super) fn fail_active_session_resume(
        &mut self,
        session_id: HostedSessionId,
        error: ResumeError,
    ) {
        if let Some(state) = self.session_resume.as_mut()
            && state.replacement_session_id == session_id
        {
            state.phase = SessionResumePhase::Failed;
            state.error = Some(error);
        }
    }

    pub(super) fn render_session_resume_sheet(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.session_resume.as_ref() else {
            return div().into_any_element();
        };
        let busy = matches!(
            state.phase,
            SessionResumePhase::Validating | SessionResumePhase::Starting
        );
        let phase = match state.phase {
            SessionResumePhase::Validating => localization::session_resume_phase_validating(),
            SessionResumePhase::Review => localization::session_resume_phase_review(),
            SessionResumePhase::Starting => localization::session_resume_phase_starting(),
            SessionResumePhase::Failed => localization::session_resume_phase_failed(),
        };
        div()
            .id("session-resume-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(theme::SPACE_4))
            .bg(theme::modal_scrim())
            .child(
                v_flex()
                    .id("session-resume-sheet")
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
                                            .child(localization::session_resume_title()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(phase),
                                    ),
                            )
                            .child(
                                Button::new("session-resume-close")
                                    .icon(IconName::Close)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_session_resume(cx);
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_4))
                            .p(px(theme::SPACE_5))
                            .child(resume_notice())
                            .child(resume_review_row(
                                localization::session_resume_source_field(),
                                state.source_title.clone(),
                            ))
                            .child(resume_review_row(
                                localization::new_session_project_field(),
                                state.project_label.clone(),
                            ))
                            .when_some(state.plan.as_ref(), |this, plan| {
                                this.child(resume_review_row(
                                    localization::session_resume_provider_field(),
                                    localization::session_resume_provider_value("0.150.1"),
                                ))
                                .child(resume_review_row(
                                    localization::session_resume_conversation_field(),
                                    plan.safe_conversation_label.clone(),
                                ))
                                .child(resume_review_row(
                                    localization::new_session_working_directory_field(),
                                    basename_or_path(plan.working_directory()),
                                ))
                                .child(resume_review_row(
                                    localization::preset_permission_field(),
                                    resume_permission_label(plan.permission_policy),
                                ))
                            })
                            .when_some(state.error, |this, error| {
                                this.child(resume_error_banner(resume_error_message(error)))
                            })
                            .child(
                                h_flex()
                                    .justify_end()
                                    .flex_wrap()
                                    .gap(px(theme::SPACE_3))
                                    .child(
                                        Button::new("session-resume-cancel")
                                            .label(localization::common_cancel())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_session_resume(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("session-resume-confirm")
                                            .primary()
                                            .icon(if busy {
                                                IconName::LoaderCircle
                                            } else {
                                                IconName::SquareTerminal
                                            })
                                            .label(localization::session_resume_confirm_action())
                                            .disabled(state.phase != SessionResumePhase::Review)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.start_session_resume(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn codex_conversation_root() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|root| root.join("sessions"))
}

fn basename_or_path(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn resume_permission_label(policy: PermissionPolicy) -> String {
    match policy {
        PermissionPolicy::AskAsNeeded => localization::preset_permission_ask(),
        PermissionPolicy::ReadOnly => localization::preset_permission_read_only(),
        PermissionPolicy::WorkspaceWrite => localization::preset_permission_workspace_write(),
    }
}

pub(super) fn resume_error_message(error: ResumeError) -> String {
    match error {
        ResumeError::StillRunning => localization::session_resume_error_still_running(),
        ResumeError::OwnershipUnproven => localization::session_resume_error_ownership(),
        ResumeError::StaleOccupant | ResumeError::StaleRevision => {
            localization::session_resume_error_stale()
        }
        ResumeError::UnsupportedVersion => localization::session_resume_error_unsupported(),
        ResumeError::ConversationMissing => localization::session_resume_error_missing(),
        ResumeError::ConversationMalformed => localization::session_resume_error_malformed(),
        ResumeError::PermissionDenied => localization::session_resume_error_permission(),
        ResumeError::ProviderUnavailable => localization::session_resume_error_provider(),
        ResumeError::ResourceLimit => localization::session_resume_error_limit(),
        ResumeError::Cancelled => localization::session_resume_cancelled(),
        ResumeError::ContinuityConflict => localization::session_resume_error_conflict(),
    }
}

fn resume_notice() -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(theme::SPACE_3))
        .p(px(theme::SPACE_3))
        .rounded(px(theme::CARD_RADIUS))
        .bg(theme::accent_soft())
        .text_color(theme::text_main())
        .child(
            Icon::new(IconName::Info)
                .size(px(theme::ICON_SIZE_DEFAULT))
                .text_color(theme::accent()),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .child(localization::session_resume_notice()),
        )
        .into_any_element()
}

fn resume_review_row(label: String, value: String) -> AnyElement {
    h_flex()
        .items_start()
        .justify_between()
        .gap(px(theme::SPACE_4))
        .child(
            div()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .text_right()
                .font_medium()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .child(value),
        )
        .into_any_element()
}

fn resume_error_banner(message: String) -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(theme::SPACE_2))
        .p(px(theme::SPACE_3))
        .rounded(px(theme::CARD_RADIUS))
        .bg(theme::with_alpha(theme::danger(), 0.1))
        .text_color(theme::danger())
        .child(Icon::new(IconName::TriangleAlert).size(px(theme::ICON_SIZE_DEFAULT)))
        .child(div().flex_1().child(message))
        .into_any_element()
}
