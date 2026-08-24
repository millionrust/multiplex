use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Styled, Window, div,
    px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{
    HostedSessionId, HostedSessionState, LaunchPreset, PermissionPolicy, PresetId, Project,
    ProjectId, ResolvedLaunch, Revision, WorkingDirectoryRule, resolve_launch,
};

use super::hosted_session::{
    DurableLaunch, DurableSessionPaths, DurableSessionSpec, spawn_durable_session,
};
use super::{AppAttachedPaneState, PendingPaste, TermiRustApp, theme};
use crate::agents::build_app_attached_launch_config;
use crate::models::{ConnectRequest, SavedAppAttachedSession, SavedDurableHost};
use crate::storage::{app_dir, save_saved_state};
use crate::ui::localization;
use crate::ui::util::current_unix_millis;

pub(super) struct NewSessionState {
    pub project_id: ProjectId,
    pub selected_preset_id: Option<PresetId>,
    pub phase: HostedSessionState,
    pub error: Option<String>,
    pub generation: u64,
    pub project_store_revision: Revision,
    pub preset_store_revision: Revision,
    pub hosted_session_id: Option<HostedSessionId>,
    pub spawned_pane_id: Option<u64>,
}

impl TermiRustApp {
    pub(super) fn open_new_session_with_preset(
        &mut self,
        project_id: ProjectId,
        preset_id: PresetId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_session(project_id, window, cx);
        if self.new_session.is_some() {
            self.select_new_session_preset(preset_id, cx);
        }
    }

    pub(super) fn open_new_session(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project_snapshot) = self.project_library.snapshot.as_ref() else {
            self.error_message = localization::project_store_unavailable();
            cx.notify();
            return;
        };
        let Some(project) = project_snapshot
            .projects
            .iter()
            .find(|summary| summary.project.id == project_id)
        else {
            self.error_message = localization::project_error_stale();
            cx.notify();
            return;
        };
        if project.status != termirust_domain::ProjectStatus::Available {
            self.error_message = localization::project_error_unavailable();
            cx.notify();
            return;
        }
        let Some(preset_snapshot) = self.preset_library.snapshot.as_ref() else {
            self.error_message = localization::preset_store_unavailable();
            cx.notify();
            return;
        };
        let selected_preset_id = preset_snapshot
            .presets
            .iter()
            .filter(|preset| preset.enabled)
            .find(|preset| preset.favorite)
            .or_else(|| preset_snapshot.presets.iter().find(|preset| preset.enabled))
            .map(|preset| preset.id);

        self.new_session = Some(NewSessionState {
            project_id,
            selected_preset_id,
            phase: HostedSessionState::Draft,
            error: selected_preset_id
                .is_none()
                .then(localization::new_session_preset_required),
            generation: 1,
            project_store_revision: project_snapshot.revision,
            preset_store_revision: preset_snapshot.revision,
            hosted_session_id: None,
            spawned_pane_id: None,
        });
        Self::set_input_value(&self.new_session_initial_input, String::new(), window, cx);
        self.new_session_initial_input
            .update(cx, |input, cx| input.focus(window, cx));
        self.error_message.clear();
        cx.notify();
    }

    fn select_new_session_preset(&mut self, preset_id: PresetId, cx: &mut Context<Self>) {
        let Some(state) = self.new_session.as_mut() else {
            return;
        };
        if new_session_busy(state.phase) {
            return;
        }
        state.selected_preset_id = Some(preset_id);
        state.phase = HostedSessionState::Draft;
        state.error = None;
        state.hosted_session_id = None;
        state.spawned_pane_id = None;
        cx.notify();
    }

    pub(super) fn cancel_new_session(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.new_session.as_mut() else {
            return;
        };
        state.generation = state.generation.wrapping_add(1).max(1);
        let pane_id = state.spawned_pane_id;
        let hosted_session_id = state.hosted_session_id;
        let validating = state.phase == HostedSessionState::Validating;
        if let Some(pane_id) = pane_id {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.user_closed = true;
                if let Some(app_attached) = pane.app_attached.as_mut() {
                    app_attached.cancel_requested = true;
                }
                pane.status = "Cancelling".to_string();
                let _ = pane
                    .runtime
                    .command_tx
                    .send(crate::ssh::SessionCommand::Disconnect);
            }
            if let Some(id) = hosted_session_id {
                self.mutate_session(
                    id,
                    termirust_domain::SessionMutation::SetLifecycle(HostedSessionState::Cancelled),
                );
            }
            if let Some(state) = self.new_session.as_mut() {
                state.phase = HostedSessionState::Cancelled;
                state.error = Some(localization::new_session_cancelled_clean());
            }
        } else if validating {
            if let Some(state) = self.new_session.as_mut() {
                state.phase = HostedSessionState::Cancelled;
                state.error = Some(localization::new_session_validation_cancelled());
            }
        } else {
            self.new_session = None;
        }
        cx.notify();
    }

    pub(super) fn close_new_session(&mut self, cx: &mut Context<Self>) {
        if self
            .new_session
            .as_ref()
            .is_some_and(|state| new_session_busy(state.phase))
        {
            self.cancel_new_session(cx);
        } else {
            self.new_session = None;
            cx.notify();
        }
    }

    fn start_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.new_session.as_ref() else {
            return;
        };
        let Some(preset_id) = state.selected_preset_id else {
            if let Some(state) = self.new_session.as_mut() {
                state.error = Some(localization::new_session_preset_required());
            }
            cx.notify();
            return;
        };
        let Some(project_snapshot) = self.project_library.snapshot.as_ref() else {
            return self.fail_new_session(&localization::project_store_unavailable(), cx);
        };
        let Some(preset_snapshot) = self.preset_library.snapshot.as_ref() else {
            return self.fail_new_session(&localization::preset_store_unavailable(), cx);
        };
        if project_snapshot.revision != state.project_store_revision
            || preset_snapshot.revision != state.preset_store_revision
        {
            return self.fail_new_session(&localization::new_session_review_stale(), cx);
        }
        let Some(project) = project_snapshot
            .projects
            .iter()
            .find(|summary| summary.project.id == state.project_id)
            .map(|summary| summary.project.clone())
        else {
            return self.fail_new_session(&localization::new_session_project_missing(), cx);
        };
        let Some(preset) = preset_snapshot
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned()
        else {
            return self.fail_new_session(&localization::new_session_preset_missing(), cx);
        };

        let state = self
            .new_session
            .as_mut()
            .expect("new session checked above");
        let generation = state.generation.wrapping_add(1).max(1);
        let hosted_session_id = HostedSessionId::new();
        state.generation = generation;
        state.hosted_session_id = Some(hosted_session_id);
        state.spawned_pane_id = None;
        state.phase = HostedSessionState::Validating;
        state.error = None;
        let path_snapshot = explicit_path_snapshot();
        let home = dirs::home_dir();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    resolve_launch(
                        hosted_session_id,
                        &project,
                        &preset,
                        &path_snapshot,
                        home.as_deref(),
                    )
                    .map(|resolved| (resolved, project, preset))
                })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.finish_new_session_validation(generation, result, window, cx);
                });
            });
        })
        .detach();
    }

    fn finish_new_session_validation(
        &mut self,
        generation: u64,
        result: Result<
            (ResolvedLaunch, Project, LaunchPreset),
            termirust_domain::LaunchResolutionError,
        >,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.new_session.as_ref() else {
            return;
        };
        if state.generation != generation || state.phase != HostedSessionState::Validating {
            return;
        }
        let (resolved, project, preset) = match result {
            Ok(value) => value,
            Err(error) => {
                return self.fail_new_session(
                    &localization::new_session_start_error(error.to_string()),
                    cx,
                );
            }
        };
        if self
            .project_library
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.revision != state.project_store_revision)
            || self
                .preset_library
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.revision != state.preset_store_revision)
        {
            return self.fail_new_session(&localization::new_session_review_stale(), cx);
        }

        let config = match build_app_attached_launch_config(&resolved) {
            Ok(config) => config,
            Err(error) => {
                return self.fail_new_session(
                    &localization::new_session_start_error(error.to_string()),
                    cx,
                );
            }
        };
        let paths = match app_dir().and_then(|directory| {
            DurableSessionPaths::create(&directory, resolved.session_id).map_err(Into::into)
        }) {
            Ok(paths) => paths,
            Err(error) => {
                return self.fail_new_session(
                    &localization::new_session_start_error(error.to_string()),
                    cx,
                );
            }
        };
        let launch = DurableLaunch {
            executable: PathBuf::from(&config.program),
            arguments: config.args.clone(),
            cwd: config
                .cwd
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| resolved.working_directory().to_path_buf()),
        };
        let initial_input = self.new_session_initial_input.read(cx).value().to_string();
        let (title, title_source) = if initial_input.trim().is_empty() {
            (
                preset.label.as_str().to_string(),
                termirust_domain::TitleSource::Default,
            )
        } else {
            (
                termirust_domain::automatic_title_from_explicit_input(
                    &initial_input,
                    resolved.session_id,
                )
                .as_str()
                .to_string(),
                termirust_domain::TitleSource::Automatic,
            )
        };
        let now = current_unix_millis();
        let position = self
            .saved
            .next_app_attached_session_position(project.id, None);
        let record = SavedAppAttachedSession {
            id: resolved.session_id,
            route: resolved.route,
            origin: resolved.origin,
            state: HostedSessionState::Provisioning,
            project_label: project.display_name.as_str().to_string(),
            preset_label: preset.label.as_str().to_string(),
            title,
            title_source,
            activity: termirust_domain::ActivityState::Unknown,
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: termirust_domain::Revision::ZERO,
            durable_host: Some(SavedDurableHost {
                runtime_root: paths.runtime_root.to_string_lossy().to_string(),
                session_dir: paths.session_dir.to_string_lossy().to_string(),
                working_directory: config.cwd.clone(),
                last_sequence: 0,
                durable_sequence: 0,
            }),
            group_id: None,
            position,
            started_at: now,
            updated_at: now,
        };
        if let Err(error) = self
            .session_library
            .create_from_saved(&mut self.saved, record)
        {
            return self.fail_new_session(
                &localization::new_session_start_error(error.to_string()),
                cx,
            );
        }
        if let Err(error) = save_saved_state(&self.saved) {
            eprintln!("[session-library] compatibility projection save failed: {error:#}");
        }

        let pane_id = self.next_session_id();
        let mut request = ConnectRequest::local_shell_with_config(pane_id, config);
        request.title = localization::new_session_workspace_title(
            project.display_name.as_str(),
            preset.label.as_str(),
        );
        let runtime = spawn_durable_session(
            DurableSessionSpec {
                pane_id,
                session_id: resolved.session_id,
                paths,
                launch: Some(launch),
                from_sequence: termirust_domain::OutputSequence::ZERO,
            },
            self.event_tx.clone(),
        );
        let terminal_focus = cx.focus_handle().tab_stop(true);
        self.register_pane(request.clone(), runtime, terminal_focus);
        self.open_spawned_pane_workspace(&request, pane_id);
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.app_attached = Some(AppAttachedPaneState {
                hosted_session_id: resolved.session_id,
                route: resolved.route,
                origin: resolved.origin,
                pending_initial_input: (!initial_input.is_empty()).then_some(initial_input),
                cancel_requested: false,
                last_sequence: 0,
                has_writer_lease: false,
            });
            pane.status = "Provisioning".to_string();
            pane.terminal_focus.focus(window);
        }
        if let Some(state) = self.new_session.as_mut() {
            state.phase = HostedSessionState::Provisioning;
            state.spawned_pane_id = Some(pane_id);
        }
        self.status_message = localization::new_session_status_starting();
        self.error_message.clear();
        self.persist_runtime_state();
        cx.notify();
    }

    fn fail_new_session(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.new_session.as_mut() {
            state.phase = HostedSessionState::Failed;
            state.error = Some(message.to_string());
        }
        cx.notify();
    }

    pub(super) fn stop_app_attached_session(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some((hosted_session_id, durable)) = self.pane(pane_id).and_then(|pane| {
            (!pane.closed).then(|| {
                pane.app_attached.as_ref().map(|attached| {
                    (
                        attached.hosted_session_id,
                        attached.route == termirust_domain::SessionLaunchRoute::DurableHost,
                    )
                })
            })?
        }) else {
            return;
        };
        if !self
            .session_library
            .session(hosted_session_id)
            .is_some_and(|session| session.lifecycle.can_stop())
        {
            return;
        }
        if !self.mutate_session(
            hosted_session_id,
            termirust_domain::SessionMutation::SetLifecycle(HostedSessionState::Stopping),
        ) {
            return;
        }
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        pane.user_closed = true;
        pane.status = "Stopping".to_string();
        let _ = pane.runtime.command_tx.send(if durable {
            crate::ssh::SessionCommand::StopDurable
        } else {
            crate::ssh::SessionCommand::Disconnect
        });
        self.status_message = localization::new_session_status_stopping();
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn app_attached_ready(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some((session_id, initial_input)) = self.pane_mut(pane_id).and_then(|pane| {
            pane.app_attached.as_mut().map(|session| {
                (
                    session.hosted_session_id,
                    session.pending_initial_input.take(),
                )
            })
        }) else {
            return;
        };
        self.mutate_session(
            session_id,
            termirust_domain::SessionMutation::SetLifecycle(
                if self
                    .pane(pane_id)
                    .and_then(|pane| pane.app_attached.as_ref())
                    .is_some_and(|session| {
                        session.route == termirust_domain::SessionLaunchRoute::DurableHost
                    })
                {
                    HostedSessionState::Live
                } else {
                    HostedSessionState::RunningAppAttached
                },
            ),
        );

        if let Some(input) = initial_input {
            if self.saved.settings.confirm_multiline_paste
                && (input.contains('\n') || input.contains('\r'))
            {
                self.pending_paste = Some(PendingPaste {
                    pane_id,
                    text: input,
                });
                self.status_message = localization::new_session_status_review_input();
            } else {
                let mut bytes = input.into_bytes();
                bytes.push(b'\r');
                if let Some(pane) = self.pane(pane_id) {
                    let _ = pane
                        .runtime
                        .command_tx
                        .send(crate::ssh::SessionCommand::Input(bytes));
                }
                self.status_message = localization::new_session_status_ready_input();
            }
        } else {
            self.status_message = localization::new_session_status_ready();
        }
        if self
            .new_session
            .as_ref()
            .is_some_and(|state| state.hosted_session_id == Some(session_id))
        {
            self.new_session = None;
        }
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn set_app_attached_terminal_state(
        &mut self,
        pane_id: u64,
        failed: bool,
        message: &str,
    ) {
        let Some((session_id, cancelled)) = self.pane(pane_id).and_then(|pane| {
            pane.app_attached
                .as_ref()
                .map(|session| (session.hosted_session_id, session.cancel_requested))
        }) else {
            return;
        };
        let durable = self.pane(pane_id).is_some_and(|pane| {
            pane.app_attached.as_ref().is_some_and(|session| {
                session.route == termirust_domain::SessionLaunchRoute::DurableHost
            })
        });
        let preserved_durable_state = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.state)
            .filter(|state| {
                matches!(
                    state,
                    HostedSessionState::Exited
                        | HostedSessionState::Orphaned
                        | HostedSessionState::Gap
                        | HostedSessionState::PermissionDenied
                        | HostedSessionState::Incompatible
                )
            });
        let state = if cancelled {
            HostedSessionState::Cancelled
        } else if let Some(state) = preserved_durable_state {
            state
        } else if durable {
            HostedSessionState::Offline
        } else if failed {
            HostedSessionState::Failed
        } else {
            HostedSessionState::Exited
        };
        self.mutate_session(
            session_id,
            termirust_domain::SessionMutation::SetLifecycle(state),
        );
        if let Some(sheet) = self.new_session.as_mut()
            && sheet.hosted_session_id == Some(session_id)
        {
            sheet.phase = state;
            sheet.error = Some(if message.trim().is_empty() {
                localization::new_session_exited_before_ready()
            } else {
                message.to_string()
            });
        }
    }

    pub(super) fn render_new_session_sheet(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.new_session.as_ref() else {
            return div().into_any_element();
        };
        let project = self.new_session_project(state.project_id);
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
        let selected = state
            .selected_preset_id
            .and_then(|id| presets.iter().copied().find(|preset| preset.id == id));
        let busy = new_session_busy(state.phase);
        let phase_label = session_phase_label(state.phase);

        div()
            .id("new-session-overlay")
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
                    .id("new-session-sheet")
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
                                            .child(localization::new_session_title()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(phase_label),
                                    ),
                            )
                            .child(
                                Button::new("new-session-close")
                                    .icon(IconName::Close)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_new_session(cx);
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_5))
                            .p(px(theme::SPACE_5))
                            .child(app_attached_warning())
                            .child(review_row(
                                localization::new_session_project_field(),
                                project
                                    .map(|project| project.display_name.as_str().to_string())
                                    .unwrap_or_else(localization::new_session_unavailable_value),
                            ))
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div()
                                            .font_semibold()
                                            .child(localization::new_session_preset_field()),
                                    )
                                    .child(h_flex().flex_wrap().gap(px(theme::SPACE_2)).children(
                                        presets.iter().map(|preset| {
                                            let preset_id = preset.id;
                                            Button::new((
                                                "new-session-preset",
                                                preset_id.as_uuid().as_u128() as u64,
                                            ))
                                            .label(preset.label.as_str().to_string())
                                            .selected(state.selected_preset_id == Some(preset_id))
                                            .disabled(busy)
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.select_new_session_preset(preset_id, cx);
                                                }),
                                            )
                                        }),
                                    )),
                            )
                            .when_some(selected, |this, preset| {
                                this.child(review_row(
                                    localization::new_session_working_directory_field(),
                                    working_directory_preview(project, preset),
                                ))
                                .child(review_row(
                                    localization::preset_permission_field(),
                                    permission_policy_label(preset.permission_policy),
                                ))
                                .when(
                                    preset.risk.is_risky(),
                                    |this| {
                                        this.child(
                                            h_flex()
                                                .gap(px(theme::SPACE_2))
                                                .text_color(theme::warning())
                                                .child(
                                                    Icon::new(IconName::TriangleAlert)
                                                        .size(px(theme::ICON_SIZE_DEFAULT)),
                                                )
                                                .child(localization::new_session_risk_warning()),
                                        )
                                    },
                                )
                            })
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div()
                                            .font_semibold()
                                            .child(localization::new_session_initial_input_field()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::new_session_initial_input_hint()),
                                    )
                                    .child(
                                        Input::new(&self.new_session_initial_input).disabled(busy),
                                    ),
                            )
                            .when_some(state.error.as_ref(), |this, error| {
                                this.child(error_banner(error.clone()))
                            })
                            .child(
                                h_flex()
                                    .justify_end()
                                    .flex_wrap()
                                    .gap(px(theme::SPACE_3))
                                    .child(
                                        Button::new("new-session-cancel")
                                            .label(if busy {
                                                localization::new_session_cancel_start()
                                            } else {
                                                localization::common_cancel()
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_new_session(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("new-session-start")
                                            .primary()
                                            .icon(if busy {
                                                IconName::LoaderCircle
                                            } else {
                                                IconName::SquareTerminal
                                            })
                                            .label(if busy {
                                                localization::new_session_starting_action()
                                            } else {
                                                localization::common_run()
                                            })
                                            .disabled(busy || selected.is_none())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.start_new_session(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn new_session_project(&self, id: ProjectId) -> Option<&Project> {
        self.project_library
            .snapshot
            .as_ref()?
            .projects
            .iter()
            .find(|summary| summary.project.id == id)
            .map(|summary| &summary.project)
    }
}

fn explicit_path_snapshot() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .take(termirust_domain::MAX_PATH_SEARCH_DIRECTORIES)
                .collect()
        })
        .unwrap_or_default()
}

fn new_session_busy(state: HostedSessionState) -> bool {
    matches!(
        state,
        HostedSessionState::Validating
            | HostedSessionState::Starting
            | HostedSessionState::Provisioning
            | HostedSessionState::Attaching
            | HostedSessionState::Replaying
    )
}

fn session_phase_label(state: HostedSessionState) -> String {
    match state {
        HostedSessionState::Draft => localization::new_session_phase_draft(),
        HostedSessionState::Validating => localization::new_session_phase_validating(),
        HostedSessionState::Starting => localization::new_session_phase_starting(),
        HostedSessionState::Provisioning => localization::new_session_phase_provisioning(),
        HostedSessionState::Attaching => localization::new_session_phase_attaching(),
        HostedSessionState::Replaying => localization::new_session_phase_replaying(),
        HostedSessionState::Live => localization::new_session_phase_live(),
        HostedSessionState::RecordingPaused => localization::new_session_phase_recording_paused(),
        HostedSessionState::Stopping => localization::new_session_status_stopping(),
        HostedSessionState::Offline => localization::new_session_phase_offline(),
        HostedSessionState::Orphaned => localization::new_session_phase_orphaned(),
        HostedSessionState::Gap => localization::new_session_phase_gap(),
        HostedSessionState::PermissionDenied => localization::new_session_phase_permission_denied(),
        HostedSessionState::Incompatible => localization::new_session_phase_incompatible(),
        HostedSessionState::RunningAppAttached => localization::new_session_phase_running(),
        HostedSessionState::Failed => localization::new_session_phase_failed(),
        HostedSessionState::Cancelled => localization::new_session_phase_cancelled(),
        HostedSessionState::Exited => localization::new_session_phase_exited(),
    }
}

fn app_attached_warning() -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(theme::SPACE_3))
        .p(px(theme::SPACE_4))
        .rounded(px(theme::CARD_RADIUS))
        .bg(theme::accent_soft())
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(px(theme::SPACE_5))
                .text_color(theme::warning()),
        )
        .child(
            v_flex()
                .gap(px(theme::SPACE_2))
                .child(
                    div()
                        .font_semibold()
                        .child(localization::new_session_warning()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::new_session_durable_copy()),
                ),
        )
        .into_any_element()
}

fn review_row(label: String, value: String) -> AnyElement {
    v_flex()
        .gap(px(theme::SPACE_2))
        .child(div().font_semibold().child(label))
        .child(
            div()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .text_color(theme::text_muted())
                .child(value),
        )
        .into_any_element()
}

fn error_banner(error: String) -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(theme::SPACE_2))
        .p(px(theme::SPACE_3))
        .rounded(px(theme::CARD_RADIUS))
        .border_1()
        .border_color(theme::danger())
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(px(theme::ICON_SIZE_DEFAULT))
                .text_color(theme::danger()),
        )
        .child(error)
        .into_any_element()
}

fn working_directory_preview(project: Option<&Project>, preset: &LaunchPreset) -> String {
    match &preset.working_directory {
        WorkingDirectoryRule::ProjectRoot => project
            .map(|project| project.canonical_root.as_path().display().to_string())
            .unwrap_or_else(localization::new_session_project_missing),
        WorkingDirectoryRule::ContainedSubdirectory(relative) => project
            .map(|project| {
                project
                    .canonical_root
                    .as_path()
                    .join(relative)
                    .display()
                    .to_string()
            })
            .unwrap_or_else(|| relative.clone()),
        WorkingDirectoryRule::PlatformHome => localization::new_session_platform_home(),
    }
}

fn permission_policy_label(policy: PermissionPolicy) -> String {
    match policy {
        PermissionPolicy::AskAsNeeded => localization::preset_permission_ask(),
        PermissionPolicy::ReadOnly => localization::preset_permission_read_only(),
        PermissionPolicy::WorkspaceWrite => localization::preset_permission_workspace_write(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SavedState;
    use crate::test_support::{TEST_POLL_INTERVAL, TestIsolation};
    use gpui::{
        AppContext as _, Focusable as _, KeyDownEvent, Keystroke, TestAppContext, WindowHandle,
    };
    use gpui_component::Root;
    use termirust_domain::{
        CanonicalPath, LocalizedUserText, PositionKey, PresetDraft, PresetOrigin, Revision,
    };
    use termirust_store::{Durability, PresetSnapshot, ProjectSnapshot, StoreHealth};

    fn wait_for_app_state<R>(
        cx: &mut TestAppContext,
        app: &gpui::Entity<TermiRustApp>,
        mut check: impl FnMut(&mut TermiRustApp) -> Option<R>,
    ) -> R {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if let Some(result) = app.update(cx, |app, cx| {
                app.process_events(cx);
                check(app)
            }) {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the synthetic durable session"
            );
            std::thread::sleep(TEST_POLL_INTERVAL);
        }
    }

    fn open_test_app(cx: &mut TestAppContext) -> (gpui::Entity<TermiRustApp>, WindowHandle<Root>) {
        let mut app_entity = None;
        let window = cx.update(|cx| {
            gpui_component::init(cx);
            cx.open_window(Default::default(), |window, cx| {
                let app = cx.new(|cx| TermiRustApp::new(SavedState::default(), window, cx));
                app_entity = Some(app.clone());
                cx.new(|cx| Root::new(app, window, cx))
            })
            .expect("test window should open")
        });
        (app_entity.expect("test app should exist"), window)
    }

    #[test]
    fn initial_input_is_blank_by_default() {
        assert!(String::new().is_empty());
    }

    #[test]
    fn policy_labels_do_not_claim_an_os_sandbox() {
        for policy in [
            PermissionPolicy::AskAsNeeded,
            PermissionPolicy::ReadOnly,
            PermissionPolicy::WorkspaceWrite,
        ] {
            assert!(!permission_policy_label(policy).contains("sandboxed"));
        }
    }

    #[test]
    fn path_snapshot_is_bounded() {
        assert!(explicit_path_snapshot().len() <= termirust_domain::MAX_PATH_SEARCH_DIRECTORIES);
    }

    #[gpui::test]
    fn projects_mod_n_opens_focused_sheet_and_escape_cancels_without_launch(
        cx: &mut TestAppContext,
    ) {
        let _isolation = TestIsolation::acquire();
        let project_root = tempfile::tempdir().unwrap();
        let project = Project {
            id: ProjectId::new(),
            display_name: LocalizedUserText::new("Synthetic project").unwrap(),
            canonical_root: CanonicalPath::resolve(project_root.path()).unwrap(),
            position: PositionKey::FIRST,
            revision: Revision::new(2),
        };
        let preset = PresetDraft {
            id: PresetId::new(),
            label: "fixture-shell".to_string(),
            executable: std::env::current_exe().unwrap().display().to_string(),
            args: Vec::new(),
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: None,
            enabled: true,
            favorite: true,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::User,
            confirm_risky_favorite: false,
        }
        .validate(PositionKey::FIRST, Revision::new(3))
        .unwrap();
        let (app, window) = open_test_app(cx);

        window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.nav_section = super::super::NavSection::Projects;
                    app.project_library.selected_id = Some(project.id);
                    app.project_library.snapshot = Some(ProjectSnapshot {
                        revision: project.revision,
                        projects: vec![project.clone().into()],
                        groups: Vec::new(),
                        health: StoreHealth::Healthy,
                        read_only: false,
                        durability: Durability::Full,
                    });
                    app.preset_library.snapshot = Some(PresetSnapshot {
                        revision: preset.revision,
                        presets: vec![preset.clone()],
                        health: StoreHealth::Healthy,
                        read_only: false,
                        durability: Durability::Full,
                    });
                    let event = KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-n").unwrap(),
                        is_held: false,
                    };
                    assert!(app.handle_global_key(&event, window, cx));
                    assert_eq!(
                        app.new_session.as_ref().map(|state| state.project_id),
                        Some(project.id)
                    );
                    assert!(
                        app.new_session_initial_input
                            .read(cx)
                            .focus_handle(cx)
                            .is_focused(window)
                    );

                    let escape = KeyDownEvent {
                        keystroke: Keystroke::parse("escape").unwrap(),
                        is_held: false,
                    };
                    assert!(app.handle_global_key(&escape, window, cx));
                    assert!(app.new_session.is_none());
                    assert!(app.saved.app_attached_sessions.is_empty());
                    assert!(app.workspaces.is_empty());
                });
            })
            .expect("window update should succeed");
    }

    #[cfg(unix)]
    #[gpui::test]
    fn reviewed_launch_sends_literal_input_and_stop_keeps_the_tab(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;

        let _isolation = TestIsolation::acquire();
        let project_root = tempfile::tempdir().unwrap();
        let executable = project_root.path().join("synthetic-agent");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'SYNTHETIC READY\\n'\nIFS= read -r line\nprintf 'INPUT=[%s]\\n' \"$line\"\nwhile :; do sleep 1; done\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let injection_marker = project_root.path().join("must-not-exist");
        let initial_input = [
            "$(",
            "touch",
            " ",
            &injection_marker.display().to_string(),
            ")",
        ]
        .concat();
        let project = Project {
            id: ProjectId::new(),
            display_name: LocalizedUserText::new("Launch fixture").unwrap(),
            canonical_root: CanonicalPath::resolve(project_root.path()).unwrap(),
            position: PositionKey::FIRST,
            revision: Revision::new(4),
        };
        let preset = PresetDraft {
            id: PresetId::new(),
            label: "fixture-agent".to_string(),
            executable: executable.display().to_string(),
            args: Vec::new(),
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: None,
            enabled: true,
            favorite: true,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::User,
            confirm_risky_favorite: false,
        }
        .validate(PositionKey::FIRST, Revision::new(5))
        .unwrap();
        let (app, window) = open_test_app(cx);

        window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.project_library.selected_id = Some(project.id);
                    app.project_library.snapshot = Some(ProjectSnapshot {
                        revision: project.revision,
                        projects: vec![project.clone().into()],
                        groups: Vec::new(),
                        health: StoreHealth::Healthy,
                        read_only: false,
                        durability: Durability::Full,
                    });
                    app.preset_library.snapshot = Some(PresetSnapshot {
                        revision: preset.revision,
                        presets: vec![preset.clone()],
                        health: StoreHealth::Healthy,
                        read_only: false,
                        durability: Durability::Full,
                    });
                    app.open_new_session(project.id, window, cx);
                    TermiRustApp::set_input_value(
                        &app.new_session_initial_input,
                        initial_input.clone(),
                        window,
                        cx,
                    );
                    app.start_new_session(window, cx);
                });
            })
            .unwrap();

        let (pane_id, hosted_session_id) = wait_for_app_state(cx, &app, |app| {
            let pane = app.panes.iter().find(|pane| pane.app_attached.is_some())?;
            let output = pane.terminal.all_rows_text().join("\n");
            (pane.connected && output.contains(&initial_input)).then(|| {
                (
                    pane.id,
                    pane.app_attached.as_ref().unwrap().hosted_session_id,
                )
            })
        });
        app.read_with(cx, |app, _| {
            assert!(app.new_session.is_none());
            assert!(!injection_marker.exists());
            assert_eq!(
                app.saved
                    .app_attached_sessions
                    .iter()
                    .find(|session| session.id == hosted_session_id)
                    .map(|session| session.state),
                Some(HostedSessionState::Live)
            );
            assert_eq!(app.saved.restored_workspaces.len(), 1);
            assert_eq!(
                app.saved.restored_workspaces[0].panes[0].durable_session_id,
                Some(hosted_session_id)
            );
        });

        app.update(cx, |app, cx| {
            app.stop_app_attached_session(pane_id, cx);
        });
        wait_for_app_state(cx, &app, |app| {
            app.pane(pane_id)
                .is_some_and(|pane| pane.closed && !pane.connected)
                .then_some(())
        });
        app.read_with(cx, |app, _| {
            assert!(
                app.pane(pane_id).is_some(),
                "Stop must retain the terminal tab"
            );
            assert_eq!(
                app.saved
                    .app_attached_sessions
                    .iter()
                    .find(|session| session.id == hosted_session_id)
                    .map(|session| session.state),
                Some(HostedSessionState::Exited)
            );
        });
        app.update(cx, |app, cx| app.close_pane(pane_id, cx));
        app.read_with(cx, |app, _| assert!(app.pane(pane_id).is_none()));
    }
}
