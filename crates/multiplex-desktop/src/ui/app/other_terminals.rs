//! The terminals on this computer that the app did not open.
//!
//! The Sessions library shows what the app owns: panes it opened and durable Sessions it started.
//! External terminals are Session Hosts started by `multiplex-cli shell`, either through global
//! shell routing or a Multiplex terminal profile. Only these CLI terminals are listed here.
//!
//! What cannot be listed: a terminal started by another app without the profile. There is no way
//! to attach to another program's pseudo-terminal, so a row for it could only ever be a claim we
//! could not honour.

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

/// How a terminal outside the app is reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OtherTerminalKind {
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
                            .tooltip(move |_, cx| {
                                cx.new(|cx| {
                                    TerminalHoverPreview::new(
                                        preview_terminal.clone(),
                                        preview_font.clone(),
                                        cx,
                                    )
                                })
                                .into()
                            })
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
                                .label(localization::other_terminals_open_action())
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_other_terminal(index, window, cx);
                                    },
                                )),
                            )
                    },
                ))
                .into_any_element(),
        )
    }

    /// Attaches a CLI terminal in a pane through the durable Session Host protocol.
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
                self.open_spawned_pane_workspace(&request, pane_id);
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
    } = kind;
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

#[cfg(test)]
mod tests {
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
