//! The terminals on this computer that the app did not open.
//!
//! The Sessions library shows what the app owns: panes it opened and durable Sessions it started.
//! A paired phone has always been shown more than that — the Controller listener also publishes
//! terminals started by `multiplex-cli shell` through the Multiplex terminal profile, and tmux
//! sessions nobody here created. This reads the same two sources so the computer's own window is
//! not the last to know.
//!
//! What cannot be listed: a terminal started by another app without the profile. There is no way
//! to attach to another program's pseudo-terminal, so a row for it could only ever be a claim we
//! could not honour.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled,
    Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::{Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use multiplex_domain::HostedSessionId;

use super::hosted_session::DurableSessionPaths;
use super::session_coordinator::SessionStartRequest;
use super::{MultiplexApp, theme};
use crate::ui::localization;

/// How a terminal outside the app is reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OtherTerminalKind {
    /// A Session Host started by `multiplex-cli shell`: the same protocol a durable Session uses.
    Console {
        session_id: HostedSessionId,
        session_dir: PathBuf,
        runtime_root: PathBuf,
    },
    /// A tmux session, reached by attaching a local shell to it.
    Tmux { name: String },
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
}

impl MultiplexApp {
    /// Reads both sources. Cheap enough to run when Sessions is opened and after a refresh: the
    /// console listing is a directory walk, and tmux is asked once with a bounded listing.
    pub(super) fn refresh_other_terminals(&mut self) {
        let mut terminals = Vec::new();

        if let Ok(app_root) = crate::storage::app_dir() {
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

        if let Ok(tmux) = multiplex_tmux::Tmux::discover()
            && let Ok(listing) = tmux.list_sessions()
        {
            for session in listing.sessions {
                // A session this app started is already in the library above it.
                if session.name.starts_with("multiplex-") || session.name.starts_with("termirust-")
                {
                    continue;
                }
                terminals.push(OtherTerminal {
                    title: session.name.clone(),
                    detail: localization::other_terminals_tmux_origin(session.windows as u64),
                    kind: OtherTerminalKind::Tmux { name: session.name },
                });
            }
        }

        self.other_terminals.terminals = terminals;
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
                                Button::new(("other-terminal-open", index))
                                    .debug_selector(move || format!("other-terminal-open-{index}"))
                                    .small()
                                    .label(localization::other_terminals_open_action())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_other_terminal(index, window, cx);
                                    })),
                            )
                    },
                ))
                .into_any_element(),
        )
    }

    /// Opens one in a pane: a console session is attached the way a durable Session is, and a
    /// tmux session gets a local shell that attaches to it.
    fn open_other_terminal(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.other_terminals.terminals.get(index).cloned() else {
            return;
        };
        match terminal.kind {
            OtherTerminalKind::Console {
                session_id,
                session_dir,
                runtime_root,
            } => {
                let request = crate::models::ConnectRequest::local_shell_with_config(
                    self.next_session_id(),
                    self.saved.settings.default_local_shell.clone(),
                );
                let Some((_, pane_id)) = self.open_request_workspace(request, window, cx) else {
                    return;
                };
                let runtime = self.session_coordinator.start(SessionStartRequest::attach(
                    pane_id,
                    session_id,
                    DurableSessionPaths {
                        runtime_root,
                        session_dir,
                    },
                    multiplex_domain::OutputSequence::ZERO,
                ));
                if let Some(pane) = self.pane_mut(pane_id) {
                    pane.runtime = runtime;
                    pane.request.title = terminal.title.clone();
                    pane.status = localization::other_terminals_attaching();
                }
                self.status_message = localization::other_terminals_attaching();
            }
            OtherTerminalKind::Tmux { name } => {
                let mut request = crate::models::ConnectRequest::local_shell_with_config(
                    self.next_session_id(),
                    self.saved.settings.default_local_shell.clone(),
                );
                request.title = name.clone();
                request.persistent_session = true;
                request.persistent_session_name = Some(name);
                request.persistent_session_detach_others = false;
                if self.open_request_workspace(request, window, cx).is_none() {
                    return;
                }
                self.status_message = localization::other_terminals_attaching();
            }
        }
        self.error_message.clear();
        cx.notify();
    }
}
