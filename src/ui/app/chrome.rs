//! Top tab bar (chrome) + library sidebar. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Bounds, ClickEvent, Context, Div, ElementId, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Point, SharedString, Stateful, StatefulInteractiveElement as _, Styled, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, Root, Sizable as _, StyledExt as _, h_flex, v_flex};

use crate::ui::app::{
    NavSection, TermiRustApp, WorkspaceIndicators, WorkspaceTab, WorkspaceTabDrag,
    WorkspaceTabDragPreview, nav_section_key,
};
use crate::ui::theme;

impl TermiRustApp {
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

    fn render_workspace_indicators(&self, indicators: WorkspaceIndicators) -> AnyElement {
        let mut nodes = Vec::new();

        // Only flag notable states — a tab whose panes are all connected
        // shows no dot at all, keeping the tab name clean.
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
        } else if indicators.closed_panes > 0 && indicators.live_panes == 0 {
            nodes.push(
                div()
                    .size(px(9.))
                    .rounded(px(999.))
                    .bg(theme::with_alpha(theme::text_muted_dark(), 0.45))
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
            .flex_shrink_0()
            .gap(px(8.))
            .items_center()
            .pl(px(12.))
            .pr(if close_button.is_some() {
                px(6.)
            } else {
                px(12.)
            })
            .h(px(32.))
            .rounded(px(6.))
            .border_1()
            .border_color(if active {
                theme::with_alpha(theme::border(), 0.8)
            } else {
                gpui::transparent_black()
            })
            .bg(if active {
                theme::chrome_tab_active()
            } else {
                gpui::transparent_black()
            })
            .when(active, |this| {
                this.shadow(vec![gpui::BoxShadow {
                    color: theme::with_alpha(gpui::black(), 0.12),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(4.),
                    spread_radius: px(0.),
                }])
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
                    .max_w(px(160.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(if active {
                        theme::text_main()
                    } else {
                        theme::with_alpha(theme::text_on_dark(), 0.65)
                    })
                    .child(label),
            )
            .when_some(indicators, |this, indicators| {
                this.child(self.render_workspace_indicators(indicators))
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

    fn render_pane_context_menu(
        &self,
        pane_id: u64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let closed = self.pane(pane_id).map(|pane| pane.closed).unwrap_or(false);
        v_flex()
            .id(("pane-context-menu", pane_id))
            .absolute()
            .top(position.y)
            .left(position.x)
            .w(px(212.))
            .p(px(6.))
            .gap(px(2.))
            .rounded(px(10.))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.workspace_tab_menu_item(
                ("pane-menu-copy", pane_id),
                IconName::Copy,
                "Copy",
                move |this, _, cx| {
                    this.pane_context_menu = None;
                    this.copy_active_selection(cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("pane-menu-paste", pane_id),
                IconName::File,
                "Paste",
                move |this, _, cx| {
                    this.pane_context_menu = None;
                    this.paste_to_active_pane(cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("pane-menu-clear", pane_id),
                IconName::Replace,
                "Clear",
                move |this, _, cx| {
                    this.pane_context_menu = None;
                    this.clear_pane_screen(pane_id, cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("pane-menu-duplicate", pane_id),
                IconName::PanelRight,
                "Duplicate",
                move |this, window, cx| {
                    this.pane_context_menu = None;
                    this.duplicate_pane_into_split(
                        pane_id,
                        super::SplitAxis::Horizontal,
                        window,
                        cx,
                    );
                },
                cx,
            ))
            .child(div().h(px(1.)).my(px(3.)).bg(theme::soft_border()))
            .when(closed, |this| {
                this.child(self.workspace_tab_menu_item(
                    ("pane-menu-reconnect", pane_id),
                    IconName::Redo,
                    "Reconnect",
                    move |this, window, cx| {
                        this.pane_context_menu = None;
                        this.reconnect_pane(pane_id, window, cx);
                    },
                    cx,
                ))
            })
            .child(self.workspace_tab_menu_item(
                ("pane-menu-detach", pane_id),
                IconName::ExternalLink,
                "Detach to new tab",
                move |this, window, cx| {
                    this.pane_context_menu = None;
                    this.move_pane_to_new_workspace(pane_id, window, cx);
                },
                cx,
            ))
            .child(self.workspace_tab_menu_item(
                ("pane-menu-close", pane_id),
                IconName::Close,
                "Close pane",
                move |this, _, cx| {
                    this.pane_context_menu = None;
                    this.close_pane(pane_id, cx);
                },
                cx,
            ))
    }

    pub(super) fn render_pane_context_menu_layer(
        &self,
        pane_id: u64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("pane-context-menu-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.pane_context_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.pane_context_menu = None;
                    cx.notify();
                }),
            )
            .child(self.render_pane_context_menu(pane_id, position, cx))
    }

    pub(super) fn duplicate_workspace_in_new_window(
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
                    title: Some("TermiRust".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(-200.), px(8.))),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let state = initial_state.clone();
                let request = request_for_window.clone();
                let view = cx.new(|cx| {
                    let mut app = TermiRustApp::new(state, window, cx);
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

    /// One custom macOS-style window-control dot (close / minimize / zoom).
    fn window_control_button(
        &self,
        id: &'static str,
        color: Hsla,
        action: impl Fn(&mut Window) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(12.))
            .rounded(px(6.))
            .bg(color)
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, window, _| action(window)))
    }

    /// The three custom traffic lights, as an inline group for the chrome row.
    fn render_window_controls(&self, cx: &mut Context<Self>) -> Div {
        h_flex()
            .flex_shrink_0()
            .gap(px(8.))
            .items_center()
            .pr(px(6.))
            .child(self.window_control_button(
                "window-close",
                gpui::rgb(0xff5f57).into(),
                |window| window.remove_window(),
                cx,
            ))
            .child(self.window_control_button(
                "window-minimize",
                gpui::rgb(0xfebc2e).into(),
                |window| window.minimize_window(),
                cx,
            ))
            .child(self.window_control_button(
                "window-zoom",
                gpui::rgb(0x28c840).into(),
                |window| window.zoom_window(),
                cx,
            ))
    }

    /// A workspace tab in rename mode: an inline text field in place of the
    /// label. Enter commits and Esc cancels (handled in `handle_global_key`).
    fn render_workspace_tab_rename(&self, workspace_id: u64) -> Stateful<Div> {
        h_flex()
            .id(("chrome-workspace-rename", workspace_id))
            .flex_shrink_0()
            .gap(px(6.))
            .items_center()
            .pl(px(10.))
            .pr(px(6.))
            .h(px(32.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme::accent())
            .bg(theme::chrome_tab_active())
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(14.))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .w(px(150.))
                    .child(Input::new(&self.tab_rename_input).small()),
            )
    }

    pub(super) fn render_top_chrome(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library_active = self.active_workspace_id.is_none();

        h_flex()
            .h(px(theme::CHROME_HEIGHT))
            .w_full()
            .relative()
            .pl(px(13.))
            .pr(px(12.))
            .gap(px(6.))
            .items_center()
            .bg(theme::chrome_bg())
            .border_b_1()
            .border_color(theme::with_alpha(theme::border_dark(), 0.5))
            .child(self.render_window_controls(cx))
            .child(
                self.render_chrome_tab(
                    "chrome-hosts",
                    Icon::new(IconName::Globe),
                    "Hosts",
                    library_active && self.nav_section != NavSection::Sftp,
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
                        .flex_shrink_0()
                        .w(px(1.))
                        .h(px(20.))
                        .mx(px(4.))
                        .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                )
            })
            .child(
                h_flex()
                    .id("chrome-tab-scroll")
                    .track_scroll(&self.tab_strip_scroll)
                    .overflow_x_scroll()
                    .h_full()
                    .min_w(px(0.))
                    .gap(px(6.))
                    .children(self.workspaces.iter().map(|workspace| {
                        let workspace_id = workspace.id;
                        if self.tab_rename_workspace_id == Some(workspace_id) {
                            return self
                                .render_workspace_tab_rename(workspace_id)
                                .into_any_element();
                        }
                        let close_id = workspace.id;
                        let active = self.active_workspace_id == Some(workspace.id);
                        let drag_info = WorkspaceTabDrag {
                            workspace_id,
                            title: workspace.title.clone(),
                        };
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
                                    .size(px(20.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .text_color(theme::text_muted_dark())
                                    .hover(|style| {
                                        style
                                            .bg(theme::with_alpha(theme::text_muted_dark(), 0.15))
                                            .text_color(theme::text_main())
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_workspace_tab_menu = None;
                                        this.close_workspace(close_id, cx);
                                    }))
                                    .child(Icon::new(IconName::Close).size(px(12.)))
                                    .into_any_element(),
                            ),
                        )
                        .on_drag(drag_info, |drag: &WorkspaceTabDrag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| WorkspaceTabDragPreview {
                                title: drag.title.clone(),
                            })
                        })
                        .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                            if drag.workspace_id == workspace_id {
                                style
                            } else {
                                style
                                    .ml(px(2.))
                                    .border_l_2()
                                    .border_color(theme::accent())
                                    .bg(theme::with_alpha(theme::accent(), 0.12))
                            }
                        })
                        .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, _, cx| {
                            if drag.workspace_id != workspace_id {
                                this.reorder_workspace_tabs(
                                    drag.workspace_id,
                                    Some(workspace_id),
                                    false,
                                );
                                this.error_message.clear();
                                cx.notify();
                            }
                        }))
                        .cursor_grab()
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if event.is_right_click() {
                                this.open_workspace_tab_context_menu(workspace_id, window, cx);
                            } else if event.click_count() >= 2 {
                                this.open_workspace_tab_menu = None;
                                this.start_workspace_rename_for(workspace_id, window, cx);
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
                    })),
            )
            .child(
                div()
                    .id("chrome-workspace-drop-tail")
                    .debug_selector(|| "chrome-workspace-drop-tail".to_string())
                    .h_full()
                    .flex_1()
                    .min_w(px(80.))
                    .drag_over::<WorkspaceTabDrag>(|style, _, _, _| {
                        style.bg(theme::with_alpha(theme::accent(), 0.08))
                    })
                    .on_drop(cx.listener(|this, drag: &WorkspaceTabDrag, _, cx| {
                        this.reorder_workspace_tabs(drag.workspace_id, None, true);
                        this.error_message.clear();
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, event: &MouseDownEvent, window, _| {
                            // Only a plain single press starts a window drag;
                            // a double-click is handled on click-up below so
                            // the native drag loop doesn't swallow it.
                            if event.click_count == 1 {
                                crate::platform_mac::start_window_drag();
                                window.start_window_move();
                            }
                        }),
                    )
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.open_workspace_tab_menu = None;
                            this.open_local_terminal(window, cx);
                        }
                    })),
            )
            .child(
                div()
                    .id("chrome-local-btn")
                    .debug_selector(|| "chrome-local-btn".to_string())
                    .flex_shrink_0()
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
                    .debug_selector(|| "chrome-new-btn".to_string())
                    .flex_shrink_0()
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
