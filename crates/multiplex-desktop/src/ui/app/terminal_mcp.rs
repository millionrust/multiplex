//! This window's answers to the terminal MCP server (`crate::terminal_mcp`): the panes it
//! holds, read, typed into, scrolled, shown, opened and closed on the window's own loop.

use gpui::{Context, Window, point};
use multiplex_domain::HostedSessionId;
use serde_json::{Value, json};

use super::{
    AnyElement, ClipboardItem, IconName, MessageId, ParentElement as _, Styled as _, div, h_flex,
    localization, px, theme, v_flex,
};
use super::{MultiplexApp, SessionPane};
use crate::models::ConnectRequest;
use crate::terminal::MouseProtocolMode;
use crate::terminal_mcp::{
    InputChunk, OpError, OpResult, ReadRegion, ScrollRequest, TerminalOp, strip_terminal_controls,
};
use crate::ui::keys::{MouseEventKind, TerminalCellPos};
use gpui::prelude::FluentBuilder as _;
use gpui::{InteractiveElement as _, IntoElement as _};
use gpui_component::StyledExt as _;

/// The most wheel reports or arrow keys one scroll sends a full-screen program.
const MAX_PROGRAM_SCROLL_STEPS: i32 = 500;

impl MultiplexApp {
    /// Answers every call the server has queued for this window.
    pub(super) fn answer_terminal_mcp_calls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(calls) = self.terminal_mcp_calls.as_ref().map(|calls| calls.take()) else {
            return;
        };
        for call in calls {
            let result = self.answer_terminal_mcp(call.op().clone(), window, cx);
            call.respond(result);
        }
    }

    fn answer_terminal_mcp(
        &mut self,
        op: TerminalOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> OpResult {
        if let TerminalOp::List = op {
            let active = window.is_window_active();
            return Ok(Value::Array(
                self.panes
                    .iter()
                    .map(|pane| self.terminal_mcp_summary(pane, active))
                    .collect(),
            ));
        }
        if let TerminalOp::Open { folder, title } = op {
            return self.terminal_mcp_open(folder, title, window, cx);
        }
        let pane_id = op
            .terminal_id()
            .and_then(|id| self.terminal_mcp_pane_id(id))
            .ok_or(OpError::NotFound)?;
        match op {
            TerminalOp::Read {
                region,
                start_line,
                max_lines,
                ..
            } => Ok(self.terminal_mcp_read(pane_id, region, start_line, max_lines)),
            TerminalOp::Output {
                since, max_bytes, ..
            } => {
                let pane = self.pane(pane_id).ok_or(OpError::NotFound)?;
                let since = since.unwrap_or_else(|| pane.terminal_mcp_output.cursor());
                let output = pane.terminal_mcp_output.since(since, max_bytes);
                Ok(json!({
                    "output_cursor": pane.terminal_mcp_output.cursor(),
                    "text": strip_terminal_controls(&output.bytes),
                    "truncated": output.truncated,
                    "state": terminal_state(pane),
                }))
            }
            TerminalOp::Input { chunks, .. } => self.terminal_mcp_input(pane_id, chunks, cx),
            TerminalOp::Scroll { scroll, .. } => self.terminal_mcp_scroll(pane_id, scroll, cx),
            TerminalOp::Focus { .. } => {
                window.activate_window();
                if let Some(workspace_id) = self.pane_workspace_id(pane_id) {
                    self.activate_workspace(workspace_id, window, cx);
                }
                self.activate_pane(pane_id, window, cx);
                Ok(json!({ "terminal_id": self.terminal_mcp_id(pane_id), "focused": true }))
            }
            TerminalOp::Close { .. } => {
                let terminal_id = self.terminal_mcp_id(pane_id);
                self.close_pane(pane_id, cx);
                cx.notify();
                Ok(json!({ "terminal_id": terminal_id, "closed": true }))
            }
            TerminalOp::List | TerminalOp::Open { .. } => Err(OpError::NotFound),
        }
    }

    fn terminal_mcp_pane_id(&self, terminal_id: HostedSessionId) -> Option<u64> {
        self.panes
            .iter()
            .find(|pane| pane.controller_session_id == terminal_id)
            .map(|pane| pane.id)
    }

    fn terminal_mcp_id(&self, pane_id: u64) -> String {
        self.pane(pane_id)
            .map(|pane| pane.controller_session_id.to_string())
            .unwrap_or_default()
    }

    fn terminal_mcp_summary(&self, pane: &SessionPane, window_active: bool) -> Value {
        let workspace = self
            .pane_workspace_id(pane.id)
            .and_then(|id| self.workspace(id));
        let tab_active =
            workspace.is_some_and(|workspace| Some(workspace.id) == self.active_workspace_id);
        let focused = window_active
            && self
                .active_pane()
                .is_some_and(|active| active.id == pane.id);
        let size = pane.terminal.size();
        json!({
            "terminal_id": pane.controller_session_id.to_string(),
            "title": pane.title,
            "kind": terminal_kind(pane),
            "endpoint": pane.endpoint,
            "state": terminal_state(pane),
            "status": pane.status,
            "error": pane.last_error,
            "tab": workspace.map(|workspace| workspace.title.clone()),
            "tab_active": tab_active,
            "focused": focused,
            "columns": size.cols,
            "rows": size.rows,
            "scrollback_lines": pane.terminal.max_scrollback(),
            "scrolled_back": pane.terminal.scrollback(),
            "alternate_screen": pane.terminal.alternate_screen(),
            "accepts_input": pane.input_authorized(),
            "output_cursor": pane.terminal_mcp_output.cursor(),
        })
    }

    fn terminal_mcp_read(
        &self,
        pane_id: u64,
        region: ReadRegion,
        start_line: Option<usize>,
        max_lines: usize,
    ) -> Value {
        let Some(pane) = self.pane(pane_id) else {
            return Value::Null;
        };
        let terminal = &pane.terminal;
        let rows = terminal.all_rows_text();
        let total = rows.len();
        let screen = usize::from(terminal.size().rows).min(total);
        let (first, end, trim) = match region {
            ReadRegion::Screen => (total - screen, total, true),
            ReadRegion::Viewport => {
                let start = terminal.visible_row_start().min(total - screen);
                (start, start + screen, true)
            }
            ReadRegion::Buffer => {
                let start = start_line
                    .unwrap_or(total.saturating_sub(max_lines))
                    .min(total);
                (start, (start + max_lines).min(total), false)
            }
        };
        let mut lines = rows[first..end].to_vec();
        if trim {
            while lines.last().is_some_and(String::is_empty) {
                lines.pop();
            }
        }
        let (cursor_row, cursor_col) = terminal.cursor_position();
        let size = terminal.size();
        json!({
            "terminal_id": pane.controller_session_id.to_string(),
            "title": pane.title,
            "state": terminal_state(pane),
            "region": match region {
                ReadRegion::Screen => "screen",
                ReadRegion::Viewport => "viewport",
                ReadRegion::Buffer => "buffer",
            },
            "first_line": first,
            "total_lines": total,
            "lines": lines,
            "cursor": {
                "row": cursor_row,
                "col": cursor_col,
                "visible": terminal.cursor_visible(),
                "line": total - screen + usize::from(cursor_row),
            },
            "columns": size.cols,
            "rows": size.rows,
            "scrolled_back": terminal.scrollback(),
            "alternate_screen": terminal.alternate_screen(),
            "output_cursor": pane.terminal_mcp_output.cursor(),
        })
    }

    fn terminal_mcp_input(
        &mut self,
        pane_id: u64,
        chunks: Vec<InputChunk>,
        cx: &mut Context<Self>,
    ) -> OpResult {
        let pane = self.pane(pane_id).ok_or(OpError::NotFound)?;
        if !pane.connected {
            return Err(OpError::Failed(format!(
                "the terminal is {}; it takes input once connected",
                terminal_state(pane)
            )));
        }
        if !pane.input_authorized() {
            return Err(OpError::Failed(
                "another viewer is typing in this session; it takes input once they stop"
                    .to_owned(),
            ));
        }
        let application_cursor = pane.terminal.application_cursor();
        let bracketed = pane.terminal.bracketed_paste();
        let mut bytes = Vec::new();
        for chunk in chunks {
            match chunk {
                InputChunk::Text(text) => {
                    bytes.extend(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
                }
                InputChunk::Paste(text) => bytes.extend(crate::ui::keys::wrap_terminal_paste(
                    text.as_bytes(),
                    bracketed,
                )),
                InputChunk::Key(key) => bytes.extend(
                    key.encode(application_cursor)
                        .ok_or_else(|| OpError::Failed("a key could not be encoded".to_owned()))?,
                ),
            }
        }
        let output_cursor = pane.terminal_mcp_output.cursor();
        let sent = bytes.len();
        if !self.send_input_bytes(pane_id, bytes, cx) {
            return Err(OpError::Failed(
                "the terminal did not take the input".to_owned(),
            ));
        }
        Ok(json!({
            "terminal_id": self.terminal_mcp_id(pane_id),
            "bytes_sent": sent,
            "output_cursor": output_cursor,
        }))
    }

    fn terminal_mcp_scroll(
        &mut self,
        pane_id: u64,
        scroll: ScrollRequest,
        cx: &mut Context<Self>,
    ) -> OpResult {
        let pane = self.pane(pane_id).ok_or(OpError::NotFound)?;
        let size = pane.terminal.size();
        let page = i32::from(size.rows.max(1));
        let lines = match scroll {
            ScrollRequest::Lines(lines) => Some(lines),
            ScrollRequest::Pages(pages) => Some(pages.saturating_mul(page)),
            ScrollRequest::Top | ScrollRequest::Bottom => None,
        };
        let mouse_reporting = pane.terminal.mouse_protocol_mode() != MouseProtocolMode::None;
        let program_scrolls = pane.connected
            && pane.input_authorized()
            && (mouse_reporting || pane.terminal.alternate_screen());
        let method = if program_scrolls {
            // A full-screen program keeps its own history, so it is scrolled the way the
            // wheel scrolls it in `handle_pane_scroll`.
            let Some(lines) = lines else {
                return Err(OpError::Failed(
                    "a full-screen program has no top or bottom to jump to; scroll by lines or \
                     pages, or send it keys"
                        .to_owned(),
                ));
            };
            let lines = lines.clamp(-MAX_PROGRAM_SCROLL_STEPS, MAX_PROGRAM_SCROLL_STEPS);
            let bytes = if mouse_reporting {
                let report = crate::ui::keys::encode_mouse_report(
                    pane.terminal.mouse_protocol_mode(),
                    pane.terminal.mouse_protocol_encoding(),
                    MouseEventKind::Wheel {
                        delta: gpui::ScrollDelta::Lines(point(0.0, lines.signum() as f32)),
                    },
                    TerminalCellPos {
                        row: size.rows / 2,
                        col: size.cols / 2,
                    },
                    gpui::Modifiers::none(),
                )
                .unwrap_or_default();
                report.repeat(lines.unsigned_abs() as usize)
            } else {
                crate::ui::keys::alternate_scroll_keys(lines, pane.terminal.application_cursor())
            };
            if !bytes.is_empty() {
                self.send_input_bytes(pane_id, bytes, cx);
            }
            if mouse_reporting {
                "mouse wheel"
            } else {
                "arrow keys"
            }
        } else {
            if let Some(pane) = self.pane_mut(pane_id) {
                match (scroll, lines) {
                    (ScrollRequest::Top, _) => {
                        let top = pane.terminal.max_scrollback();
                        pane.terminal.set_scrollback(top);
                    }
                    (ScrollRequest::Bottom, _) => pane.terminal.reset_scrollback(),
                    (_, Some(lines)) => pane.terminal.scroll_scrollback(lines),
                    (_, None) => {}
                }
                pane.selection = None;
                pane.selection_snapshot = None;
                pane.dragging_selection = false;
            }
            self.sync_terminal_grid(pane_id, cx);
            cx.notify();
            "scrollback"
        };
        let mut value = self.terminal_mcp_read(pane_id, ReadRegion::Viewport, None, 0);
        value["scrolled_with"] = json!(method);
        Ok(value)
    }

    fn terminal_mcp_open(
        &mut self,
        folder: Option<String>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> OpResult {
        let mut shell = self.saved.settings.default_local_shell.clone();
        if let Some(folder) = folder {
            if !std::path::Path::new(&folder).is_dir() {
                return Err(OpError::Failed(format!("{folder} is not a folder")));
            }
            shell.cwd = Some(folder);
        }
        let mut request = ConnectRequest::local_shell_with_config(0, shell);
        if let Some(title) = title {
            request.title = title;
        }
        self.prepare_local_cli_terminal(&mut request);
        let (_, pane_id) = self
            .open_request_workspace(request, window, cx)
            .ok_or_else(|| OpError::Failed("the terminal could not be opened".to_owned()))?;
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
        let pane = self.pane(pane_id).ok_or(OpError::NotFound)?;
        Ok(self.terminal_mcp_summary(pane, window.is_window_active()))
    }
}

fn terminal_kind(pane: &SessionPane) -> &'static str {
    if pane.app_attached.is_some() {
        "session"
    } else if pane.request.persistent_session {
        "tmux"
    } else if pane.request.is_local_shell() {
        "local"
    } else {
        "ssh"
    }
}

fn terminal_state(pane: &SessionPane) -> &'static str {
    if pane.closed {
        "closed"
    } else if pane.connected {
        "connected"
    } else if pane.last_error.is_some() {
        "error"
    } else if pane.ever_connected {
        "disconnected"
    } else {
        "connecting"
    }
}

/// Settings: the switch, and what a client needs to connect.
impl MultiplexApp {
    /// Starts or stops the server to match Settings and `MULTIPLEX_TERMINAL_MCP`.
    pub(super) fn apply_terminal_mcp_setting(&mut self) {
        let enabled = self.saved.settings.terminal_mcp_enabled
            || crate::terminal_mcp::enabled_by_environment();
        let Ok(app_dir) = crate::storage::app_dir() else {
            return;
        };
        if let Err(error) = crate::terminal_mcp::hub().set_enabled(
            enabled,
            &app_dir,
            crate::terminal_mcp::configured_port(),
        ) {
            eprintln!("[terminal-mcp] {error}");
        }
    }

    fn set_terminal_mcp_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.saved.settings.terminal_mcp_enabled = enabled;
        self.save_settings();
        self.apply_terminal_mcp_setting();
        if enabled && crate::terminal_mcp::hub().info().is_none() {
            self.error_message = localization::static_message(MessageId::TerminalMcpStartFailed);
        } else {
            self.error_message.clear();
        }
        cx.notify();
    }

    fn rotate_terminal_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Ok(app_dir) = crate::storage::app_dir() else {
            return;
        };
        match crate::terminal_mcp::hub()
            .rotate_token(&app_dir, crate::terminal_mcp::configured_port())
        {
            Ok(()) => {
                self.status_message =
                    localization::static_message(MessageId::TerminalMcpTokenRotated);
                self.error_message.clear();
            }
            Err(error) => {
                eprintln!("[terminal-mcp] {error}");
                self.error_message =
                    localization::static_message(MessageId::TerminalMcpStartFailed);
            }
        }
        cx.notify();
    }

    pub(super) fn render_terminal_mcp_controls(&self, cx: &Context<Self>) -> AnyElement {
        let message = |id| localization::static_message(id);
        let hub = crate::terminal_mcp::hub();
        let info = hub.info();
        let enabled = info.is_some();
        let copy_row = |id: &'static str,
                        value: String,
                        shown: String,
                        label: MessageId,
                        copied: MessageId| {
            h_flex()
                .gap(px(theme::SPACE_3))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::current_design_tokens().font_mono_family().0)
                        .child(shown),
                )
                .child(
                    Self::design_button(id, theme::ActionTone::Neutral, cx)
                        .icon(IconName::Copy)
                        .label(message(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                            this.status_message = localization::static_message(copied);
                            cx.notify();
                        })),
                )
        };
        v_flex()
            .id("terminal-mcp-access")
            .debug_selector(|| "terminal-mcp-access".into())
            .w_full()
            .gap(px(theme::SPACE_3))
            .p(px(theme::SPACE_4))
            .rounded(px(theme::CONTROL_RADIUS))
            .border_1()
            .border_color(theme::soft_border())
            .child(self.settings_choice_row(
                message(MessageId::TerminalMcpTitle),
                message(MessageId::TerminalMcpDescription),
                self.segmented_control(
                    "terminal-mcp-enabled",
                    [
                        (false, message(MessageId::TerminalMcpOff)),
                        (true, message(MessageId::TerminalMcpOn)),
                    ],
                    enabled,
                    false,
                    cx,
                    |this, enabled, _, cx| this.set_terminal_mcp_enabled(enabled, cx),
                ),
            ))
            .when_some(hub.last_error().filter(|_| !enabled), |this, error| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::danger())
                        .child(error),
                )
            })
            .when_some(info, |this, info| {
                let masked = format!("{}…", &info.token[..6.min(info.token.len())]);
                let config = client_configuration(&info);
                this.child(
                    v_flex()
                        .gap(px(theme::SPACE_2))
                        .child(
                            div()
                                .font_medium()
                                .child(message(MessageId::TerminalMcpUrlLabel)),
                        )
                        .child(copy_row(
                            "terminal-mcp-copy-url",
                            info.url.clone(),
                            info.url.clone(),
                            MessageId::TerminalMcpCopyUrl,
                            MessageId::TerminalMcpUrlCopied,
                        )),
                )
                .child(
                    v_flex()
                        .gap(px(theme::SPACE_2))
                        .child(
                            div()
                                .font_medium()
                                .child(message(MessageId::TerminalMcpTokenLabel)),
                        )
                        .child(copy_row(
                            "terminal-mcp-copy-token",
                            info.token.clone(),
                            masked,
                            MessageId::TerminalMcpCopyToken,
                            MessageId::TerminalMcpTokenCopied,
                        )),
                )
                .child(
                    h_flex()
                        .gap(px(theme::SPACE_3))
                        .flex_wrap()
                        .child(
                            Self::design_button(
                                "terminal-mcp-copy-config",
                                theme::ActionTone::Accent,
                                cx,
                            )
                            .icon(IconName::Copy)
                            .label(message(MessageId::TerminalMcpCopyConfig))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        config.clone(),
                                    ));
                                    this.status_message = localization::static_message(
                                        MessageId::TerminalMcpConfigCopied,
                                    );
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Self::design_button(
                                "terminal-mcp-rotate-token",
                                theme::ActionTone::Neutral,
                                cx,
                            )
                            .label(message(MessageId::TerminalMcpRotateToken))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.rotate_terminal_mcp_token(cx)),
                            ),
                        ),
                )
                .child(self.render_terminal_mcp_agent_commands(&info, cx))
                .child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(message(MessageId::TerminalMcpHint)),
                )
            })
            .into_any_element()
    }

    /// The command that adds this server to the chosen coding agent, through the app's bridge
    /// and straight to the URL.
    fn render_terminal_mcp_agent_commands(
        &self,
        info: &crate::terminal_mcp::ServerInfo,
        cx: &Context<Self>,
    ) -> AnyElement {
        let message = |id| localization::static_message(id);
        let client = self.terminal_mcp_agent;
        let commands = agent_commands(client, &bridge_executable(), info);
        let http_note = match client {
            AgentClient::ClaudeCode => MessageId::TerminalMcpAgentClaudeHttpNote,
            AgentClient::Codex => MessageId::TerminalMcpAgentCodexHttpNote,
        };
        let command_block = |id: &'static str,
                             label: MessageId,
                             note: MessageId,
                             command: AgentCommand| {
            let AgentCommand { text, shown } = command;
            v_flex()
                .gap(px(theme::SPACE_2))
                .child(div().font_medium().child(message(label)))
                .child(
                    h_flex()
                        .gap(px(theme::SPACE_3))
                        .items_start()
                        .child(
                            div()
                                .id(id)
                                .debug_selector(move || id.to_string())
                                .flex_1()
                                .min_w_0()
                                .p(px(theme::SPACE_2))
                                .rounded(px(theme::CONTROL_RADIUS))
                                .bg(theme::control_bg())
                                .font_family(theme::current_design_tokens().font_mono_family().0)
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .whitespace_normal()
                                .child(shown),
                        )
                        .child(
                            Self::design_button(
                                match id {
                                    "terminal-mcp-agent-bridge" => "terminal-mcp-copy-agent-bridge",
                                    _ => "terminal-mcp-copy-agent-http",
                                },
                                theme::ActionTone::Neutral,
                                cx,
                            )
                            .icon(IconName::Copy)
                            .label(message(MessageId::TerminalMcpCopyCommand))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    this.status_message = localization::static_message(
                                        MessageId::TerminalMcpCommandCopied,
                                    );
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(message(note)),
                )
        };
        let [bridge, http] = commands;
        v_flex()
            .id("terminal-mcp-agents")
            .debug_selector(|| "terminal-mcp-agents".into())
            .gap(px(theme::SPACE_3))
            .pt(px(theme::SPACE_3))
            .border_t_1()
            .border_color(theme::soft_border())
            .child(self.settings_choice_row(
                message(MessageId::TerminalMcpAgentsTitle),
                message(MessageId::TerminalMcpAgentsDescription),
                self.segmented_control(
                    "terminal-mcp-agent",
                    [
                        (
                            AgentClient::ClaudeCode,
                            message(MessageId::TerminalMcpAgentClaude),
                        ),
                        (
                            AgentClient::Codex,
                            message(MessageId::TerminalMcpAgentCodex),
                        ),
                    ],
                    client,
                    false,
                    cx,
                    |this, client, _, cx| {
                        this.terminal_mcp_agent = client;
                        cx.notify();
                    },
                ),
            ))
            .child(command_block(
                "terminal-mcp-agent-bridge",
                MessageId::TerminalMcpAgentBridgeLabel,
                MessageId::TerminalMcpAgentBridgeNote,
                bridge,
            ))
            .child(command_block(
                "terminal-mcp-agent-http",
                MessageId::TerminalMcpAgentHttpLabel,
                http_note,
                http,
            ))
            .into_any_element()
    }
}

/// The name the server is added under in every client.
const CLIENT_SERVER_NAME: &str = "multiplex-terminals";

/// The variable Codex reads the bearer token from: it takes no token on its command line or in
/// its configuration for a streamable HTTP server, only the name of a variable that holds one.
const CODEX_TOKEN_VARIABLE: &str = "MULTIPLEX_TERMINAL_MCP_BEARER";

/// The coding agent the Settings card shows the command for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AgentClient {
    #[default]
    ClaudeCode,
    Codex,
}

/// A command to copy, and the same command as shown, with the token cut short.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AgentCommand {
    pub(super) text: String,
    pub(super) shown: String,
}

/// This executable, which also runs the stdio bridge.
fn bridge_executable() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "multiplex".to_owned())
}

/// The bridge command and the URL command that add this server to `client`.
///
/// Both are added for the user rather than one project (`--scope user` for Claude Code; Codex
/// has only its user configuration), since the terminals belong to the whole computer.
pub(super) fn agent_commands(
    client: AgentClient,
    executable: &str,
    info: &crate::terminal_mcp::ServerInfo,
) -> [AgentCommand; 2] {
    use crate::terminal_mcp::stdio::{COMMAND, STDIO_FLAG};
    let executable = shell_quote(executable);
    let bridge = match client {
        AgentClient::ClaudeCode => format!(
            "claude mcp add --scope user {CLIENT_SERVER_NAME} -- {executable} {COMMAND} {STDIO_FLAG}"
        ),
        AgentClient::Codex => {
            format!("codex mcp add {CLIENT_SERVER_NAME} -- {executable} {COMMAND} {STDIO_FLAG}")
        }
    };
    let url = shell_quote(&info.url);
    // `--header` takes several values, so it follows the name and URL rather than coming before
    // them, where it would swallow both.
    let http = |token: &str| match client {
        AgentClient::ClaudeCode => format!(
            "claude mcp add --scope user --transport http {CLIENT_SERVER_NAME} {url} --header {}",
            shell_quote(&format!("Authorization: Bearer {token}"))
        ),
        AgentClient::Codex => format!(
            "{}{} codex mcp add {CLIENT_SERVER_NAME} --url {url} --bearer-token-env-var {CODEX_TOKEN_VARIABLE}",
            set_variable(CODEX_TOKEN_VARIABLE, token),
            // Windows PowerShell has no `&&`.
            if cfg!(windows) { ";" } else { " &&" },
        ),
    };
    let masked = format!("{}…", &info.token[..6.min(info.token.len())]);
    [
        AgentCommand {
            shown: bridge.clone(),
            text: bridge,
        },
        AgentCommand {
            text: http(&info.token),
            shown: http(&masked),
        },
    ]
}

/// `value` as one word for the shell the user is likely to paste into: a POSIX shell, or
/// PowerShell on Windows. Single quotes work in both; only the quote itself is escaped
/// differently.
fn shell_quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if plain {
        return value.to_owned();
    }
    if cfg!(windows) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// Sets an environment variable for the rest of the shell session.
fn set_variable(name: &str, value: &str) -> String {
    if cfg!(windows) {
        format!("$env:{name} = {}", shell_quote(value))
    } else {
        format!("export {name}={}", shell_quote(value))
    }
}

/// An `mcpServers` block for clients configured with JSON: the URL with its token, and the
/// stdio bridge for clients that only start commands.
fn client_configuration(info: &crate::terminal_mcp::ServerInfo) -> String {
    let command = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "multiplex".to_owned());
    serde_json::to_string_pretty(&json!({
        "mcpServers": {
            "multiplex-terminals": {
                "type": "http",
                "url": info.url,
                "headers": { "Authorization": format!("Bearer {}", info.token) },
            },
            "multiplex-terminals-stdio": {
                "command": command,
                "args": [crate::terminal_mcp::stdio::COMMAND, crate::terminal_mcp::stdio::STDIO_FLAG],
            },
        }
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, TestAppContext, WindowHandle};
    use gpui_component::Root;

    use super::*;
    use crate::models::{LocalShellConfig, SavedState};
    use crate::terminal_mcp::ParsedKey;
    use crate::test_support::{TEST_POLL_INTERVAL, TestIsolation};

    fn server_info() -> crate::terminal_mcp::ServerInfo {
        crate::terminal_mcp::ServerInfo {
            url: "http://127.0.0.1:47913/mcp".to_owned(),
            token: "abcdef0123456789".to_owned(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn claude_code_is_given_the_bridge_and_the_url_with_its_token() {
        let [bridge, http] = agent_commands(
            AgentClient::ClaudeCode,
            "/Applications/Multiplex.app/Contents/MacOS/multiplex",
            &server_info(),
        );
        assert_eq!(
            bridge.text,
            "claude mcp add --scope user multiplex-terminals -- \
             /Applications/Multiplex.app/Contents/MacOS/multiplex terminal-mcp --stdio"
        );
        assert_eq!(bridge.shown, bridge.text);
        assert_eq!(
            http.text,
            "claude mcp add --scope user --transport http multiplex-terminals \
             http://127.0.0.1:47913/mcp --header 'Authorization: Bearer abcdef0123456789'"
        );
        assert!(!http.shown.contains("abcdef0123456789"));
        assert!(http.shown.ends_with("'Authorization: Bearer abcdef…'"));
    }

    #[cfg(unix)]
    #[test]
    fn codex_is_given_the_bridge_and_a_variable_holding_the_token() {
        let [bridge, http] = agent_commands(
            AgentClient::Codex,
            "/Users/someone/My Apps/multiplex",
            &server_info(),
        );
        assert_eq!(
            bridge.text,
            "codex mcp add multiplex-terminals -- '/Users/someone/My Apps/multiplex' \
             terminal-mcp --stdio"
        );
        assert_eq!(
            http.text,
            "export MULTIPLEX_TERMINAL_MCP_BEARER=abcdef0123456789 && codex mcp add \
             multiplex-terminals --url http://127.0.0.1:47913/mcp \
             --bearer-token-env-var MULTIPLEX_TERMINAL_MCP_BEARER"
        );
        assert!(!http.shown.contains("abcdef0123456789"));
    }

    #[cfg(unix)]
    #[test]
    fn a_quote_in_a_path_survives_the_shell() {
        assert_eq!(
            shell_quote("/tmp/it's/multiplex"),
            r"'/tmp/it'\''s/multiplex'"
        );
        assert_eq!(shell_quote(""), "''");
    }

    fn open_test_app(cx: &mut TestAppContext) -> (gpui::Entity<MultiplexApp>, WindowHandle<Root>) {
        let mut app_entity = None;
        let window = cx.update(|cx| {
            gpui_component::init(cx);
            cx.open_window(Default::default(), |window, cx| {
                let app = cx.new(|cx| MultiplexApp::new(SavedState::default(), window, cx));
                app_entity = Some(app.clone());
                cx.new(|cx| Root::new(app, window, cx))
            })
            .expect("test window should open")
        });
        (app_entity.expect("test app should exist"), window)
    }

    fn answer(
        cx: &mut TestAppContext,
        app: &gpui::Entity<MultiplexApp>,
        window: WindowHandle<Root>,
        op: TerminalOp,
    ) -> OpResult {
        window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| app.answer_terminal_mcp(op, window, cx))
            })
            .expect("window update should succeed")
    }

    fn wait_until(
        cx: &mut TestAppContext,
        app: &gpui::Entity<MultiplexApp>,
        what: &str,
        mut check: impl FnMut(&mut MultiplexApp) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            cx.run_until_parked();
            if app.update(cx, |app, cx| {
                app.process_events(cx);
                check(app)
            }) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(TEST_POLL_INTERVAL);
        }
    }

    #[gpui::test]
    fn an_agent_lists_types_into_reads_scrolls_and_closes_a_local_terminal(
        cx: &mut TestAppContext,
    ) {
        let _isolation = TestIsolation::acquire();
        crate::terminal_mcp::hub().set_running_for_tests(true);
        let (app, window) = open_test_app(cx);
        let request = ConnectRequest::local_shell_with_config(
            0,
            LocalShellConfig {
                program: crate::test_support::test_shell_program(),
                args: Vec::new(),
                cwd: Some(std::env::temp_dir().display().to_string()),
            },
        );
        let (_, pane_id) = window
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.open_request_workspace(request, window, cx)
                        .expect("local workspace should open")
                })
            })
            .expect("window update should succeed");
        wait_until(cx, &app, "the shell to connect", |app| {
            app.pane(pane_id).is_some_and(|pane| pane.connected)
        });

        let listed = answer(cx, &app, window, TerminalOp::List).unwrap();
        let terminals = listed.as_array().unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["kind"], "local");
        assert_eq!(terminals[0]["state"], "connected");
        assert_eq!(terminals[0]["accepts_input"], true);
        let terminal_id: HostedSessionId = terminals[0]["terminal_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let sent = answer(
            cx,
            &app,
            window,
            TerminalOp::Input {
                terminal_id,
                chunks: vec![
                    InputChunk::Text("echo mcp-$((40+2))".to_owned()),
                    InputChunk::Key(ParsedKey::parse("Enter").unwrap()),
                ],
            },
        )
        .unwrap();
        let since = sent["output_cursor"].as_u64().unwrap();
        let mut printed = String::new();
        wait_until(cx, &app, "the command's output", |app| {
            let pane = app.pane(pane_id).unwrap();
            let output = pane.terminal_mcp_output.since(since, usize::MAX);
            printed = strip_terminal_controls(&output.bytes);
            printed.contains("mcp-42")
        });
        assert!(printed.contains("mcp-42"), "{printed}");

        let screen = answer(
            cx,
            &app,
            window,
            TerminalOp::Read {
                terminal_id,
                region: ReadRegion::Screen,
                start_line: None,
                max_lines: 100,
            },
        )
        .unwrap();
        let lines: Vec<&str> = screen["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap())
            .collect();
        assert!(lines.contains(&"mcp-42"), "{lines:?}");

        // History to scroll through, written straight into the emulator.
        app.update(cx, |app, _| {
            let pane = app.pane_mut(pane_id).unwrap();
            for line in 0..200 {
                pane.terminal
                    .process_bytes(format!("history {line}\r\n").as_bytes());
            }
        });
        let scrolled = answer(
            cx,
            &app,
            window,
            TerminalOp::Scroll {
                terminal_id,
                scroll: ScrollRequest::Top,
            },
        )
        .unwrap();
        assert_eq!(scrolled["scrolled_with"], "scrollback");
        assert!(scrolled["scrolled_back"].as_u64().unwrap() > 0);
        let buffer = answer(
            cx,
            &app,
            window,
            TerminalOp::Read {
                terminal_id,
                region: ReadRegion::Buffer,
                start_line: None,
                max_lines: 5,
            },
        )
        .unwrap();
        assert_eq!(buffer["lines"].as_array().unwrap().len(), 5);
        answer(
            cx,
            &app,
            window,
            TerminalOp::Scroll {
                terminal_id,
                scroll: ScrollRequest::Bottom,
            },
        )
        .unwrap();
        assert_eq!(
            app.read_with(cx, |app, _| app
                .pane(pane_id)
                .unwrap()
                .terminal
                .scrollback()),
            0
        );

        assert_eq!(
            answer(
                cx,
                &app,
                window,
                TerminalOp::Focus {
                    terminal_id: HostedSessionId::new()
                }
            ),
            Err(OpError::NotFound)
        );
        let closed = answer(cx, &app, window, TerminalOp::Close { terminal_id }).unwrap();
        assert_eq!(closed["closed"], true);
        crate::terminal_mcp::hub().set_running_for_tests(false);
    }
}
