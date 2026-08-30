//! Connect-tab flow: opens a workspace tab with a Username dialog or a
//! Choose-Protocol dialog, validates DNS, and renders a Termius-style
//! failure screen if the connection can't even reach the host.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    ClipboardItem, Context, Div, Entity, InteractiveElement as _, IntoElement, ParentElement,
    Stateful, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::{AuthMode, HostProfile, WorkspaceLayoutMode};
use crate::ui::app::{
    CanvasWorkspaceState, ConnectDialogMode, ConnectFailure, ConnectProtocol, NavSection,
    SplitNode, TermiRustApp, WorkspaceTab, WorkspaceViewMode,
};
use crate::ui::localization;
use crate::ui::theme;

impl TermiRustApp {
    pub(super) fn open_connect_dialog_tab(
        &mut self,
        profile_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_connect_dialog_tab_mode(profile_id, ConnectDialogMode::Username, window, cx);
    }

    pub(super) fn open_choose_protocol_tab(
        &mut self,
        profile_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_connect_dialog_tab_mode(
            profile_id,
            ConnectDialogMode::ChooseProtocol,
            window,
            cx,
        );
    }

    pub(super) fn open_choose_protocol_tab_from_draft(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = self.inputs.host.read(cx).value().trim().to_string();
        if host.is_empty() {
            self.error_message = "Enter a host before choosing a protocol.".to_string();
            cx.notify();
            return;
        }
        let label = self.inputs.label.read(cx).value().trim().to_string();
        let display_label = if label.is_empty() {
            host.clone()
        } else {
            label
        };
        let port: u16 = self
            .inputs
            .port
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(22);
        let username = self.inputs.username.read(cx).value().trim().to_string();
        let mut profile = HostProfile {
            id: format!("draft-{}", self.next_session_id()),
            label: display_label,
            host,
            port,
            username,
            ..Default::default()
        };
        profile.normalize();
        let workspace_id = self.next_workspace_id();
        let title = profile.display_name();
        Self::set_input_value(
            &self.shell_inputs.connect_username,
            &profile.username,
            window,
            cx,
        );
        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title,
            project_directory: None,
            pane_ids: Vec::new(),
            active_pane_id: 0,
            unread_events: 0,
            layout: None,
            layout_mode: WorkspaceLayoutMode::Split,
            canvas: CanvasWorkspaceState::default(),
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: Some(profile),
            pending_connect_mode: ConnectDialogMode::ChooseProtocol,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });
        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
        cx.notify();
    }

    fn open_connect_dialog_tab_mode(
        &mut self,
        profile_id: &str,
        mode: ConnectDialogMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self
            .saved
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
        else {
            return;
        };
        let workspace_id = self.next_workspace_id();
        let title = profile.display_name();
        Self::set_input_value(
            &self.shell_inputs.connect_username,
            &profile.username,
            window,
            cx,
        );
        Self::set_input_value(
            &self.shell_inputs.protocol_ssh_port,
            profile.port.to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.shell_inputs.protocol_mosh_port,
            profile.port.to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.shell_inputs.protocol_mosh_command,
            format!("mosh --server=/path/server {}", profile.host),
            window,
            cx,
        );
        Self::set_input_value(
            &self.shell_inputs.protocol_telnet_port,
            "23".to_string(),
            window,
            cx,
        );
        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title,
            project_directory: None,
            pane_ids: Vec::new(),
            active_pane_id: 0,
            unread_events: 0,
            layout: None,
            layout_mode: WorkspaceLayoutMode::Split,
            canvas: CanvasWorkspaceState::default(),
            view_mode: WorkspaceViewMode::Terminal,
            sftp: None,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: Some(profile),
            pending_connect_mode: mode,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });
        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
        cx.notify();
    }

    pub(super) fn confirm_connect_dialog(
        &mut self,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.active_workspace_id else {
            return;
        };
        let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) else {
            return;
        };
        let Some(mut profile) = workspace.pending_connect.clone() else {
            return;
        };
        let username = self
            .shell_inputs
            .connect_username
            .read(cx)
            .value()
            .to_string();
        let username = username.trim().to_string();
        if !username.is_empty() {
            profile.username = username;
        }
        if save {
            self.saved.upsert_profile(profile.clone());
            self.persist_runtime_state();
        }
        self.load_profile_into_inputs(&profile.id, window, cx);
        self.show_editor_panel = false;
        // Remove the placeholder workspace so connect_current creates a fresh one.
        self.workspaces.retain(|w| w.id != workspace_id);
        if self.active_workspace_id == Some(workspace_id) {
            self.active_workspace_id = self.workspaces.last().map(|w| w.id);
        }
        self.connect_current(window, cx);
    }

    pub(super) fn close_connect_dialog_tab(
        &mut self,
        workspace_id: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspaces.retain(|w| w.id != workspace_id);
        if self.active_workspace_id == Some(workspace_id) {
            self.active_workspace_id = self.workspaces.last().map(|w| w.id);
        }
        cx.notify();
    }

    pub(super) fn render_connect_failure_dialog(
        &self,
        workspace_id: u64,
        failure: &ConnectFailure,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = failure.profile.display_name();
        let endpoint = format!(
            "{} {}:{}",
            match failure.protocol {
                ConnectProtocol::Ssh => "SSH",
                ConnectProtocol::Mosh => "Mosh",
                ConnectProtocol::Telnet => "Telnet",
            },
            failure.profile.host,
            failure.port
        );
        let log_lines: Vec<String> = failure.log.clone();
        let profile_id = failure.profile.id.clone();
        v_flex()
            .flex_1()
            .items_center()
            .pt(px(96.))
            .gap(px(20.))
            .child(
                h_flex()
                    .w(px(520.))
                    .gap(px(12.))
                    .items_center()
                    .child(
                        div()
                            .size(px(44.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::accent(), 0.18))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(20.))
                                    .text_color(theme::accent()),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(endpoint),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "connect-fail-copy".to_string())
                            .child(
                                Button::new("connect-fail-copy")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Copy logs")
                                    .on_click(cx.listener({
                                        let log = log_lines.clone();
                                        move |_, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                log.join("\n"),
                                            ));
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w(px(520.))
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::danger())
                            .child(
                                Icon::new(IconName::User)
                                    .size(px(14.))
                                    .text_color(theme::library_card()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(2.))
                            .bg(theme::with_alpha(theme::danger(), 0.7)),
                    )
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::danger())
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(14.))
                                    .text_color(theme::library_card()),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(520.))
                    .text_size(px(13.))
                    .font_semibold()
                    .text_color(theme::danger())
                    .child("Connection failed with connection log:"),
            )
            .child(
                v_flex()
                    .w(px(520.))
                    .p(px(14.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .bg(theme::with_alpha(theme::hover(), 0.4))
                    .border_1()
                    .border_color(theme::soft_border())
                    .children(log_lines.iter().map(|line| {
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_main())
                            .child(line.clone())
                            .into_any_element()
                    })),
            )
            .child(
                h_flex()
                    .w(px(520.))
                    .pt(px(8.))
                    .gap(px(10.))
                    .justify_end()
                    .items_center()
                    .child(
                        div()
                            .debug_selector(|| "connect-fail-close".to_string())
                            .child(
                                Button::new("connect-fail-close")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label(localization::common_close())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_connect_dialog_tab(workspace_id, window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "connect-fail-edit".to_string())
                            .child(
                                Button::new("connect-fail-edit")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Edit host")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_connect_dialog_tab(workspace_id, window, cx);
                                        this.activate_library_section(
                                            NavSection::Hosts,
                                            window,
                                            cx,
                                        );
                                        this.load_profile_into_inputs(&profile_id, window, cx);
                                        this.show_editor_panel = true;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "connect-fail-restart".to_string())
                            .child(
                                Button::new("connect-fail-restart")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Start over")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.restart_choose_protocol(workspace_id, cx);
                                    })),
                            ),
                    ),
            )
    }

    pub(super) fn render_choose_protocol_dialog(
        &self,
        workspace_id: u64,
        profile: &HostProfile,
        selected: ConnectProtocol,
        cx: &mut Context<Self>,
    ) -> Div {
        let host = profile.host.clone();
        let display = profile.display_name();
        v_flex()
            .flex_1()
            .items_center()
            .pt(px(96.))
            .gap(px(28.))
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(
                        div()
                            .size(px(44.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::accent(), 0.18))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(20.))
                                    .text_color(theme::accent()),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(display.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(host.clone()),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w(px(420.))
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::accent())
                            .child(
                                Icon::new(IconName::User)
                                    .size(px(14.))
                                    .text_color(theme::library_card()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(2.))
                            .bg(theme::with_alpha(theme::text_muted(), 0.4)),
                    )
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::text_muted(), 0.3))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(14.))
                                    .text_color(theme::text_main()),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(420.))
                    .text_size(px(13.))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child("Choose protocol"),
            )
            .child(self.protocol_card(
                "proto-ssh",
                ConnectProtocol::Ssh,
                "SSH",
                &format!("ssh {host}"),
                None,
                &self.shell_inputs.protocol_ssh_port.clone(),
                selected == ConnectProtocol::Ssh,
                cx,
            ))
            .child(self.protocol_card(
                "proto-mosh",
                ConnectProtocol::Mosh,
                "Mosh",
                &format!("mosh {host}"),
                Some(self.shell_inputs.protocol_mosh_command.clone()),
                &self.shell_inputs.protocol_mosh_port.clone(),
                selected == ConnectProtocol::Mosh,
                cx,
            ))
            .child(self.protocol_card(
                "proto-telnet",
                ConnectProtocol::Telnet,
                "Telnet",
                &format!("telnet {host}"),
                None,
                &self.shell_inputs.protocol_telnet_port.clone(),
                selected == ConnectProtocol::Telnet,
                cx,
            ))
            .when(
                profile.auth_mode == AuthMode::LocalAgent
                    && selected == ConnectProtocol::Ssh,
                |this| {
                    this.child(
                        v_flex()
                            .w(px(420.))
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme::with_alpha(theme::warning(), 0.45))
                            .bg(theme::with_alpha(theme::warning(), 0.08))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_main())
                                    .child(
                                        "Agent forwarding lets this server request signatures from your local SSH agent for this connection only.",
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector(|| {
                                        "choose-proto-forward-agent".to_string()
                                    })
                                    .child(
                                        Button::new("choose-proto-forward-agent")
                                            .w_full()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Danger,
                                                cx,
                                            ))
                                            .icon(IconName::TriangleAlert)
                                            .label("Connect + Forward Agent Once")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.confirm_choose_protocol_with_agent_forwarding(
                                                    workspace_id,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    ),
                            ),
                    )
                },
            )
            .child(
                h_flex()
                    .w(px(420.))
                    .pt(px(20.))
                    .justify_between()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .debug_selector(|| "choose-proto-close".to_string())
                            .child(
                                Button::new("choose-proto-close")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label(localization::common_close())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_connect_dialog_tab(workspace_id, window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "choose-proto-continue".to_string())
                            .child(
                                Button::new("choose-proto-continue")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Continue")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_choose_protocol(workspace_id, window, cx);
                                    })),
                            ),
                    ),
            )
    }

    fn protocol_card(
        &self,
        id: &'static str,
        protocol: ConnectProtocol,
        title: &str,
        subtitle: &str,
        hint_input: Option<Entity<InputState>>,
        port_input: &Entity<InputState>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let title = title.to_string();
        let subtitle = subtitle.to_string();
        h_flex()
            .id(id)
            .w(px(420.))
            .min_h(px(64.))
            .py(px(10.))
            .px(px(14.))
            .gap(px(12.))
            .items_center()
            .rounded(px(10.))
            .bg(theme::library_card())
            .border_2()
            .border_color(if selected {
                theme::accent()
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.6)))
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child(subtitle),
                    )
                    .when_some(hint_input, |this, input| {
                        this.child(Input::new(&input).xsmall())
                    }),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("port:"),
                    )
                    .child(div().w(px(60.)).child(Input::new(port_input).xsmall())),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(workspace) = this
                    .active_workspace_id
                    .and_then(|wid| this.workspaces.iter_mut().find(|w| w.id == wid))
                {
                    workspace.pending_connect_protocol = protocol;
                    cx.notify();
                }
            }))
    }

    pub(super) fn confirm_choose_protocol(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_choose_protocol_impl(workspace_id, false, window, cx);
    }

    fn confirm_choose_protocol_with_agent_forwarding(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_choose_protocol_impl(workspace_id, true, window, cx);
    }

    fn confirm_choose_protocol_impl(
        &mut self,
        workspace_id: u64,
        forward_agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspaces.iter().find(|w| w.id == workspace_id) else {
            return;
        };
        let Some(profile) = workspace.pending_connect.clone() else {
            return;
        };
        let protocol = workspace.pending_connect_protocol;
        if protocol != ConnectProtocol::Ssh {
            let label = match protocol {
                ConnectProtocol::Mosh => "Mosh",
                ConnectProtocol::Telnet => "Telnet",
                ConnectProtocol::Ssh => "SSH",
            };
            self.status_message =
                format!("{label} protocol isn't supported yet — only SSH for now.");
            cx.notify();
            return;
        }
        let port_str = self
            .shell_inputs
            .protocol_ssh_port
            .read(cx)
            .value()
            .trim()
            .to_string();
        let port: u16 = port_str.parse().unwrap_or(profile.port);

        let mut log: Vec<String> = Vec::new();
        log.push(format!(
            "👤 Starting a new connection to: \"{}\" port \"{}\"",
            profile.host, port
        ));
        log.push(format!(
            "⚙️ Starting address resolution of \"{}\"",
            profile.host
        ));
        let resolve_target = format!("{}:{port}", profile.host);
        match std::net::ToSocketAddrs::to_socket_addrs(&resolve_target) {
            Err(e) => {
                log.push(format!("😞 Address resolution finished with error: {e}"));
                if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
                    workspace.connect_failure = Some(ConnectFailure {
                        profile: profile.clone(),
                        protocol,
                        port,
                        log,
                    });
                    workspace.pending_connect = None;
                    workspace.title = format!("{} [failed]", profile.display_name());
                }
                cx.notify();
                return;
            }
            Ok(_) => {
                log.push("✅ Address resolved.".to_string());
            }
        }

        self.load_profile_into_inputs(&profile.id, window, cx);
        if !port_str.is_empty() {
            Self::set_input_value(&self.inputs.port, port_str, window, cx);
        }
        self.show_editor_panel = false;
        let mut request = match self.build_request_for_current_draft(cx) {
            Ok(r) => r,
            Err(e) => {
                log.push(format!("😞 {e}"));
                if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
                    workspace.connect_failure = Some(ConnectFailure {
                        profile: profile.clone(),
                        protocol,
                        port,
                        log,
                    });
                    workspace.pending_connect = None;
                }
                cx.notify();
                return;
            }
        };
        if forward_agent {
            let result = request
                .auth
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("SSH authentication is not configured"))
                .and_then(|auth| auth.enable_one_shot_agent_forwarding());
            if let Err(error) = result {
                log.push(format!("Connection approval failed: {error}"));
                if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
                    workspace.connect_failure = Some(ConnectFailure {
                        profile: profile.clone(),
                        protocol,
                        port,
                        log,
                    });
                    workspace.pending_connect = None;
                }
                cx.notify();
                return;
            }
        }
        let pane_id = self.spawn_pane(request.clone(), window, cx);
        if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            workspace.pane_ids = vec![pane_id];
            workspace.layout = Some(SplitNode::Leaf(pane_id));
            workspace.active_pane_id = pane_id;
            workspace.title = request.title.clone();
            workspace.pending_connect = None;
            workspace.connect_failure = None;
        }
        self.active_workspace_id = Some(workspace_id);
        self.status_message = localization::status_connecting(request.address());
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    pub(super) fn restart_choose_protocol(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            if let Some(failure) = workspace.connect_failure.take() {
                workspace.pending_connect = Some(failure.profile);
                workspace.pending_connect_protocol = failure.protocol;
                workspace.pending_connect_mode = ConnectDialogMode::ChooseProtocol;
                workspace.title = workspace
                    .pending_connect
                    .as_ref()
                    .map(|p| p.display_name())
                    .unwrap_or_default();
            }
        }
        cx.notify();
    }

    pub(super) fn render_connect_dialog(
        &self,
        workspace_id: u64,
        profile: &HostProfile,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = profile.display_name();
        let endpoint = format!("SSH {}:{}", profile.host, profile.port);
        v_flex()
            .flex_1()
            .items_center()
            .pt(px(96.))
            .gap(px(28.))
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(
                        div()
                            .size(px(44.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::accent(), 0.18))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(20.))
                                    .text_color(theme::accent()),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(endpoint),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w(px(420.))
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::accent())
                            .child(
                                Icon::new(IconName::User)
                                    .size(px(14.))
                                    .text_color(theme::library_card()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(2.))
                            .bg(theme::with_alpha(theme::text_muted(), 0.4)),
                    )
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(999.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::text_muted(), 0.3))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(14.))
                                    .text_color(theme::text_main()),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .w(px(420.))
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("Username"),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(40.))
                            .px(px(12.))
                            .items_center()
                            .rounded(px(8.))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::soft_border())
                            .text_size(px(13.))
                            .text_color(theme::text_main())
                            .child(
                                Input::new(&self.shell_inputs.connect_username)
                                    .appearance(false)
                                    .flex_1(),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w(px(420.))
                    .justify_between()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .debug_selector(|| "connect-dialog-close".to_string())
                            .child(
                                Button::new("connect-dialog-close")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label(localization::common_close())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_connect_dialog_tab(workspace_id, window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "connect-dialog-save".to_string())
                            .child(
                                Button::new("connect-dialog-save")
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Continue & Save")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_connect_dialog(true, window, cx);
                                    })),
                            ),
                    ),
            )
    }
}
