//! The terminals on this computer that the app did not open.
//!
//! The Sessions library shows what the app owns: panes it opened and durable Sessions it started.
//! External terminals are Session Hosts started by `multiplex-cli shell`, either through global
//! shell routing or a Multiplex terminal profile. Only these CLI terminals are listed here.
//!
//! What cannot be listed: a terminal started by another app without the profile. There is no way
//! to attach to another program's pseudo-terminal, so a row for it could only ever be a claim we
//! could not honour.

use gpui::prelude::FluentBuilder as _;
use gpui_component::Disableable as _;
use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::{Icon, IconName, StyledExt as _, h_flex, v_flex};
use multiplex_domain::{HostedSessionId, OutputSequence};
use multiplex_ui_contract::MessageId;

use super::hosted_session::DurableSessionPaths;
use super::session_coordinator::SessionStartRequest;
use super::{MultiplexApp, theme};
use crate::ui::localization;

/// Polling interval for startup metadata, independent of UI animation timing.
pub(super) const CONSOLE_RENAME_RETRY_MILLIS: u64 = 50;

/// How a terminal outside the app is reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OtherTerminalKind {
    Pane {
        pane_id: u64,
    },
    /// A Session Host started by `multiplex-cli shell`: the same protocol a durable Session uses.
    Console {
        session_id: HostedSessionId,
        session_dir: PathBuf,
        runtime_root: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OtherTerminal {
    pub kind: OtherTerminalKind,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Default)]
pub(super) struct OtherTerminalsState {
    pub terminals: Vec<OtherTerminal>,
    pub(super) close_pending: Option<OtherTerminal>,
    pub(super) stopping: bool,
    pub(super) force_stopping: bool,
    pub(super) drawer_pane: Option<u64>,
}

impl MultiplexApp {
    /// Reads the live CLI Session Host records when Sessions opens or is refreshed.
    pub(super) fn refresh_other_terminals(&mut self) {
        self.other_terminals.terminals = read_other_terminals(crate::storage::app_dir().ok());
    }

    /// Poll while the Sessions destination is visible, with disk reads off the UI thread.
    #[cfg(not(test))]
    pub(super) fn start_other_terminals_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let request = this.update(cx, |app, _| {
                    (app.active_workspace_id.is_none()
                        && app.nav_section == super::NavSection::Sessions)
                        .then(|| crate::storage::app_dir().ok())
                });
                let Ok(request) = request else {
                    break;
                };
                let Some(app_root) = request else {
                    continue;
                };
                let terminals = cx
                    .background_executor()
                    .spawn(async move { read_other_terminals(app_root) })
                    .await;
                if this
                    .update(cx, |app, cx| {
                        if app.active_workspace_id.is_none()
                            && app.nav_section == super::NavSection::Sessions
                            && app.other_terminals.terminals != terminals
                        {
                            app.other_terminals.terminals = terminals;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn render_other_terminals(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.other_terminals.terminals.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .id("other-terminals")
                .debug_selector(|| "other-terminals".to_string())
                .flex_none()
                .gap(px(theme::SPACE_2))
                .px(px(theme::SPACE_4))
                .py(px(theme::SPACE_3))
                .child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .font_medium()
                        .text_color(theme::text_main())
                        .child(localization::other_terminals_heading()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::other_terminals_description()),
                )
                .children(self.other_terminals.terminals.iter().enumerate().map(
                    |(index, terminal)| {
                        let preview_terminal = terminal.clone();
                        let preview_font = self
                            .saved
                            .settings
                            .terminal_font_family
                            .clone()
                            .unwrap_or_else(|| "monospace".into());
                        h_flex()
                            .id(("other-terminal", index))
                            .debug_selector(move || format!("other-terminal-{index}"))
                            .items_center()
                            .gap(px(theme::SPACE_3))
                            .p(px(theme::SPACE_3))
                            .rounded(px(theme::CONTROL_RADIUS))
                            .border_1()
                            .border_color(theme::soft_border())
                            .bg(theme::library_card())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::chrome_tab()))
                            .when(
                                self.other_terminals.close_pending.is_none()
                                    && self.other_terminals.drawer_pane.is_none(),
                                |this| {
                                    this.tooltip(move |_, cx| {
                                        cx.new(|cx| {
                                            TerminalHoverPreview::new(
                                                preview_terminal.clone(),
                                                preview_font.clone(),
                                                cx,
                                            )
                                        })
                                        .into()
                                    })
                                },
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_other_terminal(index, window, cx);
                            }))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(theme::ICON_SIZE_DEFAULT))
                                    .text_color(theme::text_muted()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::text_main())
                                            .child(terminal.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(terminal.detail.clone()),
                                    ),
                            )
                            .child(
                                Self::design_button(
                                    ("other-terminal-open", index),
                                    theme::ActionTone::Neutral,
                                    cx,
                                )
                                .debug_selector(move || format!("other-terminal-open-{index}"))
                                .icon(IconName::PanelRightOpen)
                                .tooltip(localization::other_terminals_open_action())
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_other_terminal(index, window, cx);
                                    },
                                )),
                            )
                            .child(
                                Self::design_button(
                                    ("other-terminal-close", index),
                                    theme::ActionTone::Danger,
                                    cx,
                                )
                                .debug_selector(move || format!("other-terminal-close-{index}"))
                                .icon(IconName::CircleX)
                                .tooltip(localization::static_message(
                                    MessageId::OtherTerminalsCloseAction,
                                ))
                                .disabled(self.other_terminals.stopping)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.other_terminals.close_pending =
                                            this.other_terminals.terminals.get(index).cloned();
                                        this.error_message.clear();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Self::design_button(
                                    ("other-terminal-kill", index),
                                    theme::ActionTone::Danger,
                                    cx,
                                )
                                .debug_selector(move || format!("other-terminal-kill-{index}"))
                                .icon(IconName::Delete)
                                .tooltip(localization::static_message(
                                    MessageId::ChromeKillTerminal,
                                ))
                                .disabled(self.other_terminals.stopping)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if this.other_terminals.stopping {
                                            return;
                                        }
                                        this.other_terminals.close_pending =
                                            this.other_terminals.terminals.get(index).cloned();
                                        this.error_message.clear();
                                        this.confirm_other_terminal_close(
                                            multiplex_host_protocol::wire::StopMode::Force,
                                            cx,
                                        );
                                    },
                                )),
                            )
                    },
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_other_terminal_close_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.other_terminals.force_stopping {
            return None;
        }
        let pending = self.other_terminals.close_pending.as_ref()?;
        Some(
            div()
                .id("other-terminal-close-overlay")
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(theme::SPACE_4))
                .bg(theme::modal_scrim())
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .id("other-terminal-close-dialog")
                        .debug_selector(|| "other-terminal-close-dialog".into())
                        .w(px(theme::DIALOG_MAX_WIDTH))
                        .max_w_full()
                        .max_h_full()
                        // The scrollbar wrapper forces full height, stretching this confirmation.
                        // Native overflow keeps the dialog sized to its contents.
                        .overflow_y_scroll()
                        .p(px(theme::SPACE_5))
                        .gap(px(theme::SPACE_4))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::library_card())
                        .shadow(theme::popover_shadow())
                        .child(div().font_medium().child(localization::static_message(
                            MessageId::OtherTerminalsCloseHeading,
                        )))
                        .child(div().truncate().child(pending.title.clone()))
                        .child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::static_message(
                                    MessageId::OtherTerminalsCloseDescription,
                                )),
                        )
                        .when(!self.error_message.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_color(theme::danger())
                                    .child(self.error_message.clone()),
                            )
                        })
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(theme::SPACE_3))
                                .child(
                                    Self::design_button(
                                        "other-terminal-close-cancel",
                                        theme::ActionTone::Neutral,
                                        cx,
                                    )
                                    .debug_selector(|| "other-terminal-close-cancel".into())
                                    .icon(IconName::Close)
                                    .tooltip(localization::common_cancel())
                                    .disabled(self.other_terminals.stopping)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.other_terminals.close_pending = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    Self::design_button(
                                        "other-terminal-close-confirm",
                                        theme::ActionTone::Danger,
                                        cx,
                                    )
                                    .debug_selector(|| "other-terminal-close-confirm".into())
                                    .icon(IconName::CircleX)
                                    .tooltip(localization::static_message(
                                        MessageId::OtherTerminalsCloseAction,
                                    ))
                                    .loading(self.other_terminals.stopping)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.confirm_other_terminal_close(
                                                multiplex_host_protocol::wire::StopMode::Graceful,
                                                cx,
                                            )
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// A just-started CLI can announce its terminal before writing its session record.
    /// Read the pane's current title on every attempt so overlapping renames never save an old name.
    pub(super) fn defer_console_title_rename(
        &mut self,
        pane_id: u64,
        session_id: HostedSessionId,
        session_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            for _ in 0..160 {
                let retry = this.update(cx, |app, cx| {
                    let Some(title) = app
                        .pane(pane_id)
                        .filter(|pane| {
                            crate::models::local_console_session_id(&pane.request)
                                == Some(session_id)
                        })
                        .map(|pane| pane.request.title.clone())
                    else {
                        return false;
                    };
                    match multiplex_store::rename_console_session(&session_dir, session_id, &title)
                    {
                        Ok(()) => {
                            app.refresh_other_terminals();
                            cx.notify();
                            false
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                        Err(_) => {
                            app.error_message = localization::session_library_operation_failed();
                            cx.notify();
                            false
                        }
                    }
                });
                if !matches!(retry, Ok(true)) {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(
                        CONSOLE_RENAME_RETRY_MILLIS,
                    ))
                    .await;
            }
            let _ = this.update(cx, |app, cx| {
                app.error_message = localization::session_library_operation_failed();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn request_tab_terminal_kill(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        self.open_workspace_tab_menu = None;
        let Some(pane_id) = self
            .workspace(workspace_id)
            .map(|workspace| workspace.active_pane_id)
        else {
            return;
        };
        self.request_pane_terminal_kill(pane_id, cx);
    }

    fn request_pane_terminal_kill(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        if self.other_terminals.stopping {
            return;
        }
        let Some(pane) = self.pane(pane_id) else {
            return;
        };
        let kind = self
            .terminal_kind_for_pane(pane.id)
            .unwrap_or(OtherTerminalKind::Pane { pane_id: pane.id });
        self.other_terminals.close_pending = Some(OtherTerminal {
            kind,
            title: pane.title.clone(),
            detail: pane.endpoint.clone(),
        });
        self.error_message.clear();
        self.confirm_other_terminal_close(multiplex_host_protocol::wire::StopMode::Force, cx);
    }

    fn terminal_kind_for_pane(&self, pane_id: u64) -> Option<OtherTerminalKind> {
        let pane = self.pane(pane_id)?;
        if let Some(id) = crate::models::local_console_session_id(&pane.request) {
            let app_root = crate::storage::app_dir().ok()?;
            return Some(OtherTerminalKind::Console {
                session_id: id,
                session_dir: app_root.join("console-sessions").join(id.to_string()),
                runtime_root: crate::controller_runtime_parent(&app_root).join(id.to_string()),
            });
        }
        let id = pane.app_attached.as_ref()?.hosted_session_id;
        let host = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|saved| saved.id == id)?
            .durable_host
            .as_ref()?;
        Some(OtherTerminalKind::Console {
            session_id: id,
            session_dir: host.session_dir.clone().into(),
            runtime_root: host.runtime_root.clone().into(),
        })
    }

    fn confirm_other_terminal_close(
        &mut self,
        mode: multiplex_host_protocol::wire::StopMode,
        cx: &mut Context<Self>,
    ) {
        if self.other_terminals.stopping {
            return;
        }
        let Some(pending) = self.other_terminals.close_pending.clone() else {
            return;
        };
        if let OtherTerminalKind::Pane { pane_id } = pending.kind {
            self.other_terminals.close_pending = None;
            self.close_pane(pane_id, cx);
            self.active_workspace_id = None;
            self.nav_section = super::NavSection::Sessions;
            cx.notify();
            return;
        }
        self.other_terminals.stopping = true;
        self.other_terminals.force_stopping =
            mode == multiplex_host_protocol::wire::StopMode::Force;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let kind = pending.kind.clone();
            let stopped = cx
                .background_executor()
                .spawn(async move { stop_other_terminal(&kind, mode) })
                .await;
            let refreshed = cx
                .background_executor()
                .spawn(async { read_other_terminals(crate::storage::app_dir().ok()) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.other_terminals.stopping = false;
                app.other_terminals.force_stopping = false;
                if stopped.is_ok() {
                    let OtherTerminalKind::Console { session_id, .. } = pending.kind else {
                        return;
                    };
                    let pane_ids: Vec<_> =
                        app.panes
                            .iter()
                            .filter(|pane| {
                                crate::models::local_console_session_id(&pane.request)
                                    == Some(session_id)
                                    || pane.app_attached.as_ref().is_some_and(|session| {
                                        session.hosted_session_id == session_id
                                    })
                            })
                            .map(|pane| pane.id)
                            .collect();
                    app.other_terminals.drawer_pane = None;
                    for id in pane_ids {
                        app.close_pane(id, cx);
                    }
                    app.active_workspace_id = None;
                    app.nav_section = super::NavSection::Sessions;
                    app.other_terminals.close_pending = None;
                    app.other_terminals.terminals = refreshed;
                    app.error_message.clear();
                    app.status_message =
                        localization::static_message(MessageId::OtherTerminalsClosed);
                } else {
                    app.other_terminals.terminals = refreshed;
                    if mode == multiplex_host_protocol::wire::StopMode::Force {
                        app.other_terminals.close_pending = None;
                    }
                    app.error_message =
                        localization::static_message(MessageId::OtherTerminalsCloseFailed);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Attaches a CLI terminal in a pane through the durable Session Host protocol.
    pub(super) fn close_session_drawer(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.other_terminals.drawer_pane.take()
            && self.pane_workspace_id(id).is_none()
        {
            self.close_pane(id, cx);
            self.unpublish_desktop_pane(id);
            self.panes.retain(|pane| pane.id != id);
        }
        cx.notify();
    }

    pub(super) fn preview_library_session(
        &mut self,
        id: HostedSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_session_drawer(cx);
        let existing = self
            .panes
            .iter()
            .find(|pane| {
                pane.app_attached
                    .as_ref()
                    .is_some_and(|session| session.hosted_session_id == id)
            })
            .map(|pane| pane.id);
        let pane_id = existing.or_else(|| {
            let saved = self
                .saved
                .app_attached_sessions
                .iter()
                .find(|session| session.id == id && session.archived_at.is_none())
                .cloned()?;
            if saved.route != multiplex_domain::SessionLaunchRoute::DurableHost {
                return None;
            }
            let mut request = crate::models::ConnectRequest::local_shell_with_config(
                0,
                self.saved.settings.default_local_shell.clone(),
            );
            request.title = saved.title.clone();
            self.spawn_saved_durable_pane(request, &saved, window, cx)
        });
        self.other_terminals.drawer_pane = pane_id;
        if let Some(pane_id) = pane_id {
            self.activate_pane(pane_id, window, cx);
        }
        self.active_workspace_id = None;
        cx.notify();
    }

    pub(super) fn render_session_drawer(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pane = self.pane(self.other_terminals.drawer_pane?)?;
        Some(
            v_flex()
                .id("session-terminal-drawer")
                .debug_selector(|| "session-terminal-drawer".into())
                .w_full()
                .max_w(px(theme::DIALOG_WIDE_WIDTH))
                .flex_shrink_0()
                .h_full()
                .bg(theme::terminal_bg())
                .border_l_1()
                .border_color(theme::border())
                .child(
                    h_flex()
                        .p(px(theme::SPACE_3))
                        .gap(px(theme::SPACE_3))
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(pane.title.clone()),
                        )
                        .child(
                            Self::design_button(
                                "session-drawer-open-tab",
                                theme::ActionTone::Neutral,
                                cx,
                            )
                            .debug_selector(|| "session-drawer-open-tab".into())
                            .icon(IconName::ExternalLink)
                            .tooltip(localization::static_message(
                                MessageId::SessionDrawerOpenTab,
                            ))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    let Some(id) = this.other_terminals.drawer_pane.take() else {
                                        return;
                                    };
                                    if this.pane_workspace_id(id).is_some() {
                                        this.move_pane_to_new_workspace(id, window, cx);
                                    } else if let Some(request) =
                                        this.pane(id).map(|pane| pane.request.clone())
                                    {
                                        this.open_spawned_pane_workspace(&request, id);
                                        this.activate_pane(id, window, cx);
                                    }
                                    this.sync_terminal_layout(window, cx);
                                    this.persist_runtime_state();
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Self::design_button(
                                "session-drawer-kill",
                                theme::ActionTone::Danger,
                                cx,
                            )
                            .debug_selector(|| "session-drawer-kill".into())
                            .icon(IconName::Delete)
                            .tooltip(localization::static_message(MessageId::ChromeKillTerminal))
                            .loading(self.other_terminals.force_stopping)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(id) = this.other_terminals.drawer_pane {
                                    this.request_pane_terminal_kill(id, cx);
                                }
                            })),
                        )
                        .child(
                            Self::design_button(
                                "session-drawer-close",
                                theme::ActionTone::Neutral,
                                cx,
                            )
                            .debug_selector(|| "session-drawer-close".into())
                            .icon(IconName::PanelRightClose)
                            .tooltip(localization::common_close())
                            .on_click(cx.listener(|this, _, _, cx| this.close_session_drawer(cx))),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_terminal_pane(pane, window, cx)),
                )
                .into_any_element(),
        )
    }

    fn open_other_terminal(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.other_terminals.terminals.get(index).cloned() else {
            return;
        };
        match terminal.kind {
            OtherTerminalKind::Pane { .. } => return,
            OtherTerminalKind::Console {
                session_id,
                session_dir,
                runtime_root,
            } => {
                let mut request = crate::models::ConnectRequest::local_shell_with_config(
                    self.next_session_id(),
                    self.saved.settings.default_local_shell.clone(),
                );
                request.persistent_session_name = Some(format!("multiplex-cli:{session_id}"));
                request.title = terminal.title.clone();
                let pane_id = request.session_id;
                let runtime = self.session_coordinator.start(SessionStartRequest::attach(
                    pane_id,
                    session_id,
                    DurableSessionPaths {
                        runtime_root,
                        session_dir,
                    },
                    multiplex_domain::OutputSequence::ZERO,
                ));
                self.register_pane(
                    request.clone(),
                    runtime,
                    cx.focus_handle().tab_stop(true),
                    cx.focus_handle().tab_stop(true),
                    cx,
                );
                self.close_session_drawer(cx);
                self.other_terminals.drawer_pane = Some(pane_id);
                self.sync_terminal_layout(window, cx);
                if let Some(pane) = self.pane(pane_id) {
                    pane.terminal_focus.focus(window);
                }
                self.persist_runtime_state();
                self.status_message = localization::other_terminals_attaching();
            }
        }
        self.error_message.clear();
        cx.notify();
    }
}

fn read_other_terminals(app_root: Option<PathBuf>) -> Vec<OtherTerminal> {
    let mut terminals = Vec::new();

    if let Some(app_root) = app_root {
        let data_root = app_root.join("durable-sessions");
        let console_root = multiplex_store::console_sessions_root(&data_root);
        let runtime_parent = crate::controller_runtime_parent(&app_root);
        for live in multiplex_store::live_console_sessions(&console_root, &runtime_parent) {
            let session_id = live.record.session_id;
            terminals.push(OtherTerminal {
                title: live.record.title(),
                detail: localization::other_terminals_profile_origin(
                    live.record.working_directory.display().to_string(),
                ),
                kind: OtherTerminalKind::Console {
                    session_id,
                    session_dir: console_root.join(session_id.to_string()),
                    runtime_root: runtime_parent.join(session_id.to_string()),
                },
            });
        }
    }

    terminals
}

/// A transient read-only viewer. It requests neither a writer lease nor a PTY resize,
/// and its output stays in memory only while the tooltip is visible.
struct TerminalHoverPreview {
    terminal: OtherTerminal,
    font: String,
    lines: Option<Result<Vec<String>, ()>>,
}

impl TerminalHoverPreview {
    fn new(terminal: OtherTerminal, font: String, cx: &mut Context<Self>) -> Self {
        let kind = terminal.kind.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let kind = kind.clone();
                let lines = cx
                    .background_executor()
                    .spawn(async move { read_terminal_preview(&kind) })
                    .await;
                if this
                    .update(cx, |view, cx| {
                        view.lines = Some(lines);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
            }
        })
        .detach();
        Self {
            terminal,
            font,
            lines: None,
        }
    }
}

impl Render for TerminalHoverPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let lines = match &self.lines {
            None => vec![localization::static_message(
                MessageId::OtherTerminalsPreviewLoading,
            )],
            Some(Err(())) => vec![localization::static_message(
                MessageId::OtherTerminalsPreviewUnavailable,
            )],
            Some(Ok(lines)) if lines.is_empty() => vec![localization::static_message(
                MessageId::OtherTerminalsPreviewEmpty,
            )],
            Some(Ok(lines)) => lines.clone(),
        };
        v_flex()
            .id("other-terminal-preview")
            .debug_selector(|| "other-terminal-preview".into())
            .w(px(theme::DIALOG_MAX_WIDTH))
            .max_w_full()
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .rounded(px(theme::CONTROL_RADIUS))
            .bg(theme::library_card())
            .shadow(theme::popover_shadow())
            .child(
                div()
                    .font_medium()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .truncate()
                    .child(self.terminal.title.clone()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::static_message(
                        MessageId::OtherTerminalsPreviewHeading,
                    )),
            )
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .p(px(theme::SPACE_3))
                    .bg(theme::terminal_default_bg())
                    .rounded(px(theme::CONTROL_RADIUS))
                    .font_family(self.font.clone())
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::terminal_default_fg())
                    .children(
                        lines
                            .into_iter()
                            .map(|line| div().w_full().truncate().child(line)),
                    ),
            )
    }
}

fn read_terminal_preview(kind: &OtherTerminalKind) -> Result<Vec<String>, ()> {
    use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
    use rand::RngCore as _;
    use tokio_util::sync::CancellationToken;
    let OtherTerminalKind::Console {
        session_id,
        runtime_root,
        ..
    } = kind
    else {
        return Err(());
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ())?;
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let cancel = CancellationToken::new();
            let mut nonce = [0; 32];
            rand::rngs::OsRng.fill_bytes(&mut nonce);
            let mut client = HostClient::connect(
                LocalEndpoint::new(runtime_root, *session_id),
                ConnectOptions::local_read_only(*session_id, nonce),
                &cancel,
            )
            .await
            .map_err(|_| ())?;
            let state = client.get_state(&cancel).await.map_err(|_| ())?;
            let from = OutputSequence::new(state.latest_sequence.saturating_sub(64));
            let outputs = client
                .attach(from, 160, 48, &cancel)
                .await
                .map_err(|_| ())?;
            let mut terminal =
                crate::terminal::TerminalState::new(crate::terminal::TerminalSize::default(), 0);
            if let Some(snapshot) = client.take_last_snapshot() {
                terminal.process_bytes(&snapshot.terminal_bytes);
            }
            for output in outputs {
                terminal.process_bytes(&output.bytes);
            }
            client.disconnect();
            Ok(preview_lines(&terminal))
        })
        .await
        .map_err(|_| ())?
    })
}

fn preview_lines(terminal: &crate::terminal::TerminalState) -> Vec<String> {
    let mut lines: Vec<String> = terminal
        .snapshot()
        .rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            for cell in &row.cells {
                cell.push_text(&mut line);
            }
            line.trim_end().to_owned()
        })
        .collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.drain(..lines.len().saturating_sub(14));
    lines
}

/// A stop acknowledgement can race with the Host exiting and closing its endpoint.
/// Only a matching terminal's recorded exit counts as success; an unreachable live
/// Host or a replacement Host must remain an error.
fn terminal_has_exited(session_dir: &std::path::Path, session_id: HostedSessionId) -> bool {
    multiplex_store::read_host_metadata(session_dir).is_ok_and(|metadata| {
        metadata.session_id == session_id
            && matches!(
                metadata.lifecycle,
                multiplex_domain::HostLifecycle::Exited | multiplex_domain::HostLifecycle::Failed
            )
    })
}

fn stop_other_terminal(
    kind: &OtherTerminalKind,
    mode: multiplex_host_protocol::wire::StopMode,
) -> Result<(), ()> {
    use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
    use multiplex_domain::CommandId;
    use rand::RngCore as _;
    use tokio_util::sync::CancellationToken;
    let OtherTerminalKind::Console {
        session_id,
        session_dir,
        runtime_root,
    } = kind
    else {
        return Err(());
    };
    if terminal_has_exited(session_dir, *session_id) {
        return Ok(());
    }
    let expected = multiplex_store::read_host_metadata(session_dir)
        .map_err(|_| ())?
        .host_instance_id;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ())?;
    let result = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            let cancel = CancellationToken::new();
            let mut nonce = [0; 32];
            rand::rngs::OsRng.fill_bytes(&mut nonce);
            let mut client = HostClient::connect(
                LocalEndpoint::new(runtime_root, *session_id),
                ConnectOptions::local_read_only(*session_id, nonce),
                &cancel,
            )
            .await
            .map_err(|_| ())?;
            if client.host_instance_id() != Some(expected) {
                return Err(());
            }
            client
                .stop(CommandId::new(), mode, &cancel)
                .await
                .map_err(|_| ())?;
            client.disconnect();
            Ok(())
        })
        .await
        .map_err(|_| ())?
    });
    if result.is_err() && terminal_has_exited(session_dir, *session_id) {
        Ok(())
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_preserves_writer_control_and_close_stops_the_owned_host() {
        preview_and_stop(multiplex_host_protocol::wire::StopMode::Graceful).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn force_kill_skips_graceful_deadlines_for_a_signal_resistant_shell() {
        preview_and_stop(multiplex_host_protocol::wire::StopMode::Force).await;
    }

    #[cfg(unix)]
    async fn preview_and_stop(mode: multiplex_host_protocol::wire::StopMode) {
        let force = mode == multiplex_host_protocol::wire::StopMode::Force;
        use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
        use multiplex_domain::{HostInstanceId, HostedSessionId};
        use multiplex_session_host::{LaunchDescriptor, StopDeadlines};
        use std::collections::BTreeMap;
        use tokio_util::sync::CancellationToken;
        let fixture = tempfile::Builder::new()
            .prefix("multiplex-preview-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(fixture.path()).unwrap();
        let session_id = HostedSessionId::new();
        let session_dir = root.join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        let runtime_root = root.join("runtime");
        let descriptor = LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: runtime_root.clone(),
            session_dir: session_dir.clone(),
            executable: std::fs::canonicalize("/bin/sh").unwrap(),
            runtime_detection: None,
            arguments: vec![
                "-c".into(),
                if force {
                    "trap '' INT TERM; printf 'preview-job-ready\\r\\n'; sleep 30".into()
                } else {
                    "printf 'preview-job-ready\\r\\n'; sleep 30".into()
                },
            ],
            environment: BTreeMap::new(),
            cwd: Some(root),
            columns: 93,
            rows: 27,
            journal_limits: multiplex_store::JournalLimits::default(),
            stop_deadlines: if force {
                StopDeadlines {
                    interrupt_millis: 5_000,
                    terminate_millis: 5_000,
                    total_millis: 5_000,
                }
            } else {
                StopDeadlines::default()
            },
        };
        let host = multiplex_session_host::start(descriptor).await.unwrap();
        let cancel = CancellationToken::new();
        let mut writer = HostClient::connect(
            LocalEndpoint::new(&runtime_root, session_id),
            ConnectOptions::local(session_id, [11; 32]),
            &cancel,
        )
        .await
        .unwrap();
        let kind = super::OtherTerminalKind::Console {
            session_id,
            session_dir,
            runtime_root,
        };
        let mut found = false;
        for _ in 0..20 {
            let request = kind.clone();
            let lines = tokio::task::spawn_blocking(move || super::read_terminal_preview(&request))
                .await
                .unwrap()
                .unwrap();
            if lines.iter().any(|line| line.contains("preview-job-ready")) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(
                super::theme::current_design_tokens()
                    .motion_hosted_connect_poll(false)
                    .0 as u64,
            ))
            .await;
        }
        assert!(found);
        assert!(writer.get_state(&cancel).await.unwrap().has_writer_lease);
        let request = kind.clone();
        let started = std::time::Instant::now();
        tokio::task::spawn_blocking(move || super::stop_other_terminal(&request, mode))
            .await
            .unwrap()
            .unwrap();
        host.wait().await.unwrap();
        let already_closed = kind.clone();
        tokio::task::spawn_blocking(move || super::stop_other_terminal(&already_closed, mode))
            .await
            .unwrap()
            .expect("closing an already-exited terminal must succeed");
        if force {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "force kill must not wait through the five-second graceful deadline"
            );
        }
    }

    #[test]
    fn preview_emulates_terminal_control_sequences_and_keeps_recent_lines() {
        let mut terminal =
            crate::terminal::TerminalState::new(crate::terminal::TerminalSize::default(), 0);
        terminal.process_bytes(b"\x1b[31mjob running\x1b[0m\r\nprogress 1\rprogress 2");
        assert_eq!(
            super::preview_lines(&terminal),
            ["job running", "progress 2"]
        );
        for _ in 0..30 {
            terminal.process_bytes(b"\r\nnext");
        }
        assert_eq!(super::preview_lines(&terminal).len(), 14);
    }
}
