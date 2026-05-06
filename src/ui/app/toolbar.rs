//! Workspace toolbar for open SSH/local terminal sessions. Methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, InteractiveElement as _, ParentElement, StatefulInteractiveElement as _, Styled,
    Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Disableable, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::ui::app::{
    SplitAxis, TermiRustApp, WorkspaceRuntimeTone, WorkspaceViewMode, workspace_runtime_summary,
};
use crate::ui::theme;

impl TermiRustApp {
    pub(super) fn render_workspace_toolbar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let Some(pane) = self.active_pane() else {
            return v_flex();
        };
        let indicators = self.workspace_indicators(workspace);
        let files_mode = workspace.view_mode == WorkspaceViewMode::Files;
        let can_browse_files = !pane.request.is_local_shell();
        let selected_remote_entry = self.selected_workspace_sftp_entry(workspace.id);
        let _focused = pane.terminal_focus.is_focused(window);
        let workspace_id = workspace.id;
        let broadcast_active = workspace.broadcast_input;
        let broadcast_available = workspace.pane_ids.len() > 1;
        let renaming = self.tab_rename_workspace_id == Some(workspace_id);

        h_flex()
            .h(px(theme::WORKSPACE_HEADER_HEIGHT))
            .w_full()
            .px(px(18.))
            .gap_2()
            .items_center()
            .justify_between()
            .bg(theme::chrome_bg())
            .border_b_1()
            .border_color(theme::border_dark())
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(16.))
                            .text_color(theme::accent()),
                    )
                    .when(renaming, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .w(px(220.))
                                        .child(Input::new(&self.tab_rename_input).small()),
                                )
                                .child(
                                    Button::new("workspace-rename-save")
                                        .xsmall()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label("Save")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.commit_workspace_rename(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("workspace-rename-cancel")
                                        .xsmall()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_workspace_rename(window, cx);
                                        })),
                                ),
                        )
                    })
                    .when(!renaming, |this| {
                        this.child(
                            div()
                                .id(("workspace-title", workspace_id))
                                .text_size(px(15.))
                                .font_semibold()
                                .text_color(theme::text_on_dark())
                                .cursor_pointer()
                                .hover(|style| {
                                    style.text_color(theme::with_alpha(theme::accent(), 0.95))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_workspace_rename(window, cx);
                                }))
                                .child(workspace.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted_dark())
                                .child(pane.endpoint.clone()),
                        )
                    })
                    .when_some(
                        workspace_runtime_summary(indicators),
                        |this, (label, tone)| {
                            this.child(self.status_badge(
                                label,
                                theme::terminal_panel(),
                                match tone {
                                    WorkspaceRuntimeTone::Live => theme::success(),
                                    WorkspaceRuntimeTone::Connecting => theme::warning(),
                                    WorkspaceRuntimeTone::Error => theme::danger(),
                                    WorkspaceRuntimeTone::Closed => theme::text_muted_dark(),
                                },
                            ))
                        },
                    )
                    .when(indicators.split_count > 1, |this| {
                        this.child(self.status_badge(
                            format!("{} Panes", indicators.split_count),
                            theme::terminal_panel(),
                            theme::slate(),
                        ))
                    })
                    .when(files_mode, |this| {
                        this.child(self.status_badge(
                            "Files",
                            theme::terminal_panel(),
                            theme::accent(),
                        ))
                    })
                    .when(pane.request.is_local_shell(), |this| {
                        this.child(self.status_badge(
                            "Local",
                            theme::terminal_panel(),
                            theme::success(),
                        ))
                    })
                    .when_some(pane.request.jump_host.as_ref(), |this, jump_host| {
                        this.child(self.status_badge(
                            format!("Via {}", jump_host.title),
                            theme::terminal_panel(),
                            theme::accent(),
                        ))
                    })
                    .when(!pane.request.port_forward_rules.is_empty(), |this| {
                        let forward_label = if pane.request.port_forward_rules.len() == 1 {
                            pane.request.port_forward_rules[0].display_name()
                        } else {
                            format!("{} Forwards", pane.request.port_forward_rules.len())
                        };
                        this.child(self.status_badge(
                            forward_label,
                            theme::terminal_panel(),
                            theme::warning(),
                        ))
                    }),
            )
            .child(
                h_flex()
                    .gap(px(3.))
                    .items_center()
                    .child(self.status_badge(
                        pane.status.clone(),
                        theme::terminal_panel(),
                        if pane.connected {
                            theme::success()
                        } else if pane.closed {
                            theme::warning()
                        } else {
                            theme::accent()
                        },
                    ))
                    .child(
                        Button::new("workspace-scroll-top")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scroll_active_pane_top(cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-scroll-bottom")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowDown)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scroll_active_pane_bottom(cx);
                            })),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                    )
                    .child(
                        Button::new("workspace-search")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_workspace_search(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-commands")
                            .ghost()
                            .small()
                            .icon(IconName::SquareTerminal)
                            .label("Commands")
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_command_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files")
                            .ghost()
                            .small()
                            .icon(if files_mode {
                                IconName::SquareTerminal
                            } else {
                                IconName::FolderOpen
                            })
                            .label(if files_mode { "Terminal" } else { "Files" })
                            .disabled(!files_mode && !can_browse_files)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.active_workspace().is_some_and(|workspace| {
                                    workspace.view_mode == WorkspaceViewMode::Files
                                }) {
                                    this.show_active_workspace_terminal(cx);
                                } else {
                                    this.open_active_workspace_files(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("workspace-split-right")
                            .ghost()
                            .small()
                            .icon(IconName::PanelRight)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Horizontal, window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-split-down")
                            .ghost()
                            .small()
                            .icon(IconName::PanelBottom)
                            .disabled(files_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.split_active_workspace(SplitAxis::Vertical, window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-broadcast")
                            .small()
                            .custom(Self::segmented_button_style(broadcast_active, cx))
                            .label(if broadcast_active {
                                "Broadcast On"
                            } else {
                                "Broadcast"
                            })
                            .disabled(files_mode || !broadcast_available)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_workspace_broadcast(workspace_id, cx);
                            })),
                    )
                    .when(files_mode, |this| {
                        this.child(
                            Button::new("workspace-files-up")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowLeft)
                                .label("Up")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.navigate_workspace_files_up(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-refresh")
                                .ghost()
                                .small()
                                .icon(IconName::Redo)
                                .label("Refresh")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(workspace_id) = this.active_workspace_id {
                                        this.refresh_workspace_files(workspace_id);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new("workspace-files-upload")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowUp)
                                .label("Upload")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.upload_workspace_file(window, cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-open")
                                .ghost()
                                .small()
                                .icon(IconName::FolderOpen)
                                .label("Open")
                                .disabled(
                                    !selected_remote_entry
                                        .as_ref()
                                        .is_some_and(|entry| entry.is_dir),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_selected_workspace_file_entry(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-download")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowDown)
                                .label("Download")
                                .disabled(
                                    !selected_remote_entry
                                        .as_ref()
                                        .is_some_and(|entry| !entry.is_dir),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.download_workspace_file(window, cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-delete")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .icon(IconName::Delete)
                                .label("Delete")
                                .disabled(selected_remote_entry.is_none())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_workspace_file(cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                    )
                    .when(pane.closed, |this| {
                        this.child(
                            Button::new("workspace-reconnect")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Success, cx))
                                .icon(IconName::Redo)
                                .label("Reconnect")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(pane_id) = this.active_pane().map(|p| p.id) {
                                        this.reconnect_pane(pane_id, window, cx);
                                    }
                                })),
                        )
                    })
                    .when(indicators.closed_panes > 1, |this| {
                        this.child(
                            Button::new("workspace-reconnect-all")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Success, cx))
                                .icon(IconName::Redo)
                                .label(format!("Reconnect All ({})", indicators.closed_panes))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reconnect_all(window, cx);
                                })),
                        )
                    })
                    .when(pane.connected, |this| {
                        this.child(
                            Button::new("workspace-disconnect")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .label("Disconnect")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(workspace_id) = this.active_workspace_id {
                                        this.disconnect_workspace(workspace_id, cx);
                                    }
                                })),
                        )
                    }),
            )
    }
}
