//! Top tab bar (chrome) + library sidebar. All methods are part of `TShellApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Bounds, ClickEvent, Context, Div, ElementId,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions,
    div, point, px, size,
};
use gpui_component::{Icon, IconName, Root, StyledExt as _, h_flex, v_flex};

use crate::ui::app::{
    ICON_PANEL_COLLAPSE_RIGHT, NavSection, TShellApp, WorkspaceIndicators, WorkspaceTab,
    WorkspaceTabDrag, WorkspaceTabDragPreview, WorkspaceViewMode, app_icon, nav_section_key,
};
use crate::ui::theme;

impl TShellApp {
    pub(super) fn workspace_indicators(&self, workspace: &WorkspaceTab) -> WorkspaceIndicators {
        let mut indicators = WorkspaceIndicators {
            split_count: workspace.pane_ids.len(),
            unread_events: workspace.unread_events,
            ..WorkspaceIndicators::default()
        };

        for pane_id in &workspace.pane_ids {
            if let Some(pane) = self.pane(*pane_id) {
                if pane.connected {
                    indicators.live_panes += 1;
                } else if pane.closed {
                    if pane.status == "Error" {
                        indicators.error_panes += 1;
                    } else {
                        indicators.closed_panes += 1;
                    }
                } else {
                    indicators.connecting_panes += 1;
                }
            }
        }

        indicators
    }

    fn render_workspace_indicators(
        &self,
        indicators: WorkspaceIndicators,
        active: bool,
    ) -> AnyElement {
        let mut nodes = Vec::new();

        if indicators.unread_events > 0 {
            nodes.push(
                div()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(6.))
                    .rounded(px(999.))
                    .bg(theme::accent())
                    .text_size(px(11.))
                    .font_semibold()
                    .text_color(theme::library_card())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(indicators.unread_events.min(99).to_string())
                    .into_any_element(),
            );
        }

        if indicators.error_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::danger())
                    .into_any_element(),
            );
        } else if indicators.connecting_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::warning())
                    .into_any_element(),
            );
        } else if indicators.live_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::success())
                    .opacity(if active { 1.0 } else { 0.86 })
                    .into_any_element(),
            );
        } else if indicators.closed_panes > 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::with_alpha(theme::text_muted_dark(), 0.45))
                    .into_any_element(),
            );
        }

        if indicators.split_count > 1 {
            nodes.push(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(999.))
                    .bg(theme::with_alpha(theme::text_muted_dark(), 0.12))
                    .text_size(px(11.))
                    .font_medium()
                    .text_color(theme::text_muted_dark())
                    .child(format!("{}p", indicators.split_count))
                    .into_any_element(),
            );
        }

        h_flex()
            .gap_1()
            .items_center()
            .children(nodes)
            .into_any_element()
    }

    fn render_chrome_tab(
        &self,
        id: impl Into<ElementId>,
        icon: Icon,
        label: impl Into<SharedString>,
        active: bool,
        indicators: Option<WorkspaceIndicators>,
        close_button: Option<AnyElement>,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        h_flex()
            .id(id)
            .gap(px(7.))
            .items_center()
            .pl(px(12.))
            .pr(if close_button.is_some() {
                px(6.)
            } else {
                px(14.)
            })
            .h(px(34.))
            .rounded(px(8.))
            .bg(if active {
                theme::chrome_tab_active()
            } else {
                gpui::transparent_black()
            })
            .when(!active, |this| {
                this.hover(|style| style.bg(theme::chrome_tab()))
            })
            .child(icon.size(px(14.)).text_color(if active {
                theme::accent()
            } else {
                theme::text_muted_dark()
            }))
            .child(
                div()
                    .max_w(px(140.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(if active {
                        theme::text_on_dark()
                    } else {
                        theme::with_alpha(theme::text_on_dark(), 0.72)
                    })
                    .child(label),
            )
            .when_some(indicators, |this, indicators| {
                this.child(self.render_workspace_indicators(indicators, active))
            })
            .when_some(close_button, |this, button| this.child(button))
    }

    fn workspace_tab_menu_item(
        &self,
        id: impl Into<ElementId>,
        icon: IconName,
        label: &'static str,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        h_flex()
            .id(id)
            .w_full()
            .h(px(32.))
            .px(px(10.))
            .gap(px(9.))
            .items_center()
            .rounded(px(7.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.72)))
            .child(
                Icon::new(icon)
                    .size(px(14.))
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                action(this, window, cx);
            }))
    }

    fn render_workspace_tab_context_menu(
        &self,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
            .unwrap_or(0);
        let left = 230.0 + (index as f32 * 188.0);
        let can_split = self
            .workspace(workspace_id)
            .map(|workspace| workspace.pane_ids.len() < super::MAX_SPLIT_PANES)
            .unwrap_or(false);

        v_flex()
            .id(("workspace-tab-menu", workspace_id))
            .absolute()
            .top(px(theme::CHROME_HEIGHT - 2.))
            .left(px(left))
            .w(px(238.))
            .p(px(6.))
            .gap(px(2.))
            .rounded(px(10.))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.workspace_tab_menu_item(
                ("workspace-tab-menu-duplicate", workspace_id),
                IconName::Copy,
                "Duplicate",
                move |this, window, cx| {
                    this.duplicate_workspace_in_new_tab(workspace_id, window, cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("workspace-tab-menu-duplicate-window", workspace_id),
                IconName::ExternalLink,
                "Duplicate in a new window",
                move |this, window, cx| {
                    this.duplicate_workspace_in_new_window(workspace_id, window, cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("workspace-tab-menu-multiplayer", workspace_id),
                IconName::User,
                "Start multiplayer",
                move |this, _, cx| {
                    this.start_multiplayer_for_workspace(workspace_id, cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("workspace-tab-menu-rename", workspace_id),
                IconName::ALargeSmall,
                "Rename",
                move |this, window, cx| {
                    this.start_workspace_rename_for(workspace_id, window, cx);
                },
                cx,
            ))
            .when(can_split, |this| {
                this.child(self.workspace_tab_menu_item(
                    ("workspace-tab-menu-split-horizontal", workspace_id),
                    IconName::PanelRight,
                    "Split horizontally",
                    move |this, window, cx| {
                        this.split_workspace_horizontally(workspace_id, window, cx);
                    },
                    cx,
                ))
            })
            .child(div().h(px(1.)).my(px(3.)).bg(theme::soft_border()))
            .child(self.workspace_tab_menu_item(
                ("workspace-tab-menu-close", workspace_id),
                IconName::Close,
                "Close",
                move |this, _, cx| {
                    this.open_workspace_tab_menu = None;
                    this.close_workspace(workspace_id, cx);
                },
                cx,
            ))
    }

    pub(super) fn render_workspace_tab_context_menu_layer(
        &self,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("workspace-tab-menu-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open_workspace_tab_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.open_workspace_tab_menu = None;
                    cx.notify();
                }),
            )
            .child(self.render_workspace_tab_context_menu(workspace_id, cx))
    }

    fn open_workspace_tab_context_menu(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_tab_menu = Some(workspace_id);
        self.activate_workspace(workspace_id, window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn duplicate_workspace_in_new_window(
        &mut self,
        workspace_id: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .workspace(workspace_id)
            .and_then(|workspace| self.pane(workspace.active_pane_id))
            .map(|pane| pane.request.clone())
        else {
            return;
        };
        let mut initial_state = self.saved.clone();
        initial_state.settings.restore_workspaces_on_launch = false;
        let request_for_window = request.clone();
        let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);

        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("TShell".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(14.))),
                }),
                is_movable: false,
                ..Default::default()
            },
            move |window, cx| {
                let state = initial_state.clone();
                let request = request_for_window.clone();
                let view = cx.new(|cx| {
                    let mut app = TShellApp::new(state, window, cx);
                    if let Some((_, pane_id)) =
                        app.open_request_workspace(request.clone(), window, cx)
                    {
                        app.status_message = format!("Connecting to {}...", request.address());
                        app.error_message.clear();
                        app.sync_terminal_layout(window, cx);
                        if let Some(pane) = app.pane(pane_id) {
                            pane.terminal_focus.focus(window);
                        }
                    }
                    app
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        ) {
            Ok(_) => {
                self.open_workspace_tab_menu = None;
                self.status_message =
                    format!("Duplicating {} in a new window...", request.address());
                self.error_message.clear();
                cx.notify();
            }
            Err(error) => {
                self.error_message = format!("Unable to open duplicate window: {error}");
                cx.notify();
            }
        }
    }

    pub(super) fn render_top_chrome(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library_active = self.active_workspace_id.is_none();
        let terminal_workspace_open = self
            .active_workspace()
            .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Terminal);
        let app_entity = cx.weak_entity();

        h_flex()
            .h(px(theme::CHROME_HEIGHT))
            .w_full()
            .relative()
            .pl(px(theme::CHROME_INSET_LEFT))
            .pr(px(12.))
            .gap(px(4.))
            .items_center()
            .bg(theme::chrome_bg())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.chrome_window_drag_pending = true;
                    window.prevent_default();
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.chrome_window_drag_pending = false;
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.chrome_window_drag_pending = false;
            }))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.chrome_window_drag_pending {
                    this.chrome_window_drag_pending = false;
                    window.start_window_move();
                }
            }))
            .child(
                self.render_chrome_tab(
                    "chrome-hosts",
                    Icon::new(IconName::Globe),
                    "Hosts",
                    library_active,
                    None,
                    None,
                )
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_workspace_tab_menu = None;
                    this.activate_library(window, cx);
                })),
            )
            .child(
                self.render_chrome_tab(
                    "chrome-sftp",
                    Icon::new(IconName::Folder),
                    "SFTP",
                    library_active && self.nav_section == NavSection::Sftp,
                    None,
                    None,
                )
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_workspace_tab_menu = None;
                    let active = this
                        .active_workspace_id
                        .and_then(|wid| this.workspaces.iter().find(|w| w.id == wid))
                        .and_then(|w| Some((w.id, w.active_pane_id)));
                    if let Some((wid, pid)) = active {
                        this.open_workspace_files_for_pane(wid, pid, cx);
                    } else {
                        this.activate_library_section(NavSection::Sftp, window, cx);
                    }
                })),
            )
            .when(!self.workspaces.is_empty(), |this| {
                this.child(
                    div()
                        .w(px(1.))
                        .h(px(20.))
                        .mx(px(4.))
                        .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                )
            })
            .children(self.workspaces.iter().map(|workspace| {
                let workspace_id = workspace.id;
                let close_id = workspace.id;
                let active = self.active_workspace_id == Some(workspace.id);
                let drag_info = WorkspaceTabDrag {
                    workspace_id,
                    title: workspace.title.clone(),
                };
                let tab_hover_entity = app_entity.clone();
                let tab_drag_entity = app_entity.clone();
                let indicators = self.workspace_indicators(workspace);
                self.render_chrome_tab(
                    ("chrome-workspace", workspace.id),
                    Icon::new(IconName::SquareTerminal),
                    workspace.title.clone(),
                    active,
                    Some(indicators),
                    Some(
                        div()
                            .id(("chrome-close-wrap", workspace.id))
                            .size(px(18.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| {
                                style.bg(theme::with_alpha(theme::text_muted_dark(), 0.2))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_workspace_tab_menu = None;
                                this.close_workspace(close_id, cx);
                            }))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(12.))
                                    .text_color(theme::text_muted_dark()),
                            )
                            .into_any_element(),
                    ),
                )
                .on_drag(drag_info, move |drag: &WorkspaceTabDrag, _, _, app| {
                    let entity = tab_drag_entity.clone();
                    let dragged_workspace_id = drag.workspace_id;
                    app.defer(move |app| {
                        let _ = entity.update(app, |this, cx| {
                            this.activate_workspace_for_incoming_tab_drag(dragged_workspace_id, cx);
                        });
                    });
                    app.stop_propagation();
                    app.new(|_| WorkspaceTabDragPreview {
                        title: drag.title.clone(),
                    })
                })
                .drag_over::<WorkspaceTabDrag>(move |style, drag, _, app| {
                    if drag.workspace_id == workspace_id {
                        style
                    } else {
                        let entity = tab_hover_entity.clone();
                        app.defer(move |app| {
                            let _ = entity.update(app, |this, cx| {
                                if this.active_workspace_id != Some(workspace_id) {
                                    this.active_workspace_id = Some(workspace_id);
                                    this.reset_workspace_activity(workspace_id);
                                    this.open_workspace_tab_menu = None;
                                    this.error_message.clear();
                                    this.status_message =
                                        "Drop the tab onto a pane edge to split it here."
                                            .to_string();
                                    cx.notify();
                                }
                            });
                        });
                        style
                            .ml(px(2.))
                            .border_l_2()
                            .border_color(theme::accent())
                            .bg(theme::with_alpha(theme::accent(), 0.12))
                    }
                })
                .on_drop(
                    cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                        if drag.workspace_id != workspace_id {
                            this.drop_workspace_on_workspace(
                                drag.workspace_id,
                                workspace_id,
                                window,
                                cx,
                            );
                        }
                    }),
                )
                .cursor_grab()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.is_right_click() {
                        this.open_workspace_tab_context_menu(workspace_id, window, cx);
                    } else {
                        this.open_workspace_tab_menu = None;
                        this.activate_workspace(workspace_id, window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, window, cx| {
                        this.open_workspace_tab_context_menu(workspace_id, window, cx);
                    }),
                )
                .into_any_element()
            }))
            .child(
                div()
                    .id("chrome-workspace-drop-tail")
                    .h_full()
                    .flex_1()
                    .min_w(px(24.))
                    .drag_over::<WorkspaceTabDrag>(|style, _, _, _| {
                        style.bg(theme::with_alpha(theme::accent(), 0.08))
                    })
                    .on_drop(cx.listener(|this, drag: &WorkspaceTabDrag, _, cx| {
                        this.reorder_workspace_tabs(drag.workspace_id, None, true);
                        this.error_message.clear();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("chrome-local-btn")
                    .size(px(30.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::with_alpha(theme::text_muted_dark(), 0.15))
                    .hover(|style| {
                        style
                            .bg(theme::chrome_tab())
                            .border_color(theme::with_alpha(theme::text_muted_dark(), 0.3))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tab_menu = None;
                        this.open_local_terminal(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(14.))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
            .child(
                div()
                    .id("chrome-new-btn")
                    .size(px(30.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::with_alpha(theme::text_muted_dark(), 0.15))
                    .hover(|style| {
                        style
                            .bg(theme::chrome_tab())
                            .border_color(theme::with_alpha(theme::text_muted_dark(), 0.3))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tab_menu = None;
                        this.activate_library(window, cx);
                        this.open_editor_for_new_host(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(14.))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
            .when(terminal_workspace_open, |this| {
                this.child(
                    div()
                        .id("chrome-toggle-side-panel")
                        .ml(px(8.))
                        .size(px(28.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(if self.terminal_panel_visible {
                            theme::with_alpha(theme::accent(), 0.18)
                        } else {
                            gpui::transparent_black()
                        })
                        .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.6)))
                        .child(
                            app_icon(ICON_PANEL_COLLAPSE_RIGHT)
                                .size(px(14.))
                                .text_color(if self.terminal_panel_visible {
                                    theme::accent()
                                } else {
                                    theme::text_muted_dark()
                                }),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.terminal_panel_visible = !this.terminal_panel_visible;
                            cx.notify();
                        })),
                )
            })
    }

    fn nav_card(
        &self,
        id: impl Into<ElementId>,
        section: NavSection,
        active: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let _ = cx;
        h_flex()
            .id(id)
            .w_full()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .h(px(36.))
            .rounded(px(8.))
            .bg(if active {
                theme::accent_soft()
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|style| {
                style.bg(if active {
                    theme::accent_soft()
                } else {
                    theme::hover()
                })
            })
            .child(section.icon().size(px(16.)).text_color(if active {
                theme::accent()
            } else {
                theme::text_muted()
            }))
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(if active {
                        theme::accent()
                    } else {
                        theme::text_main()
                    })
                    .child(section.label()),
            )
    }

    pub(super) fn render_library_sidebar(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .w(px(theme::HOST_SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .px(px(12.))
            .pt(px(16.))
            .pb(px(16.))
            .bg(theme::library_sidebar())
            .child(
                v_flex().gap(px(2.)).children(
                    [
                        NavSection::Hosts,
                        NavSection::Vaults,
                        NavSection::Keychain,
                        NavSection::Snippets,
                        NavSection::Settings,
                    ]
                    .into_iter()
                    .map(|section| {
                        let active = self.nav_section == section;
                        self.nav_card(("nav-card", nav_section_key(section)), section, active, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.nav_section != section {
                                    this.show_editor_panel = false;
                                }
                                this.nav_section = section;
                                this.error_message.clear();
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
                ),
            )
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .my(px(8.))
                    .bg(theme::with_alpha(theme::border(), 0.6)),
            )
            .child(
                v_flex().gap(px(2.)).children(
                    [NavSection::KnownHosts, NavSection::Logs]
                        .into_iter()
                        .map(|section| {
                            let active = self.nav_section == section;
                            self.nav_card(
                                ("nav-card", nav_section_key(section)),
                                section,
                                active,
                                cx,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.nav_section != section {
                                    this.show_editor_panel = false;
                                }
                                this.nav_section = section;
                                this.error_message.clear();
                                cx.notify();
                            }))
                            .into_any_element()
                        }),
                ),
            )
    }
}
