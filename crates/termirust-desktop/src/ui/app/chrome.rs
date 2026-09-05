//! Top tab bar (chrome) + library sidebar. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Bounds, ClickEvent, Context, Div, ElementId, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Point, SharedString, Stateful, StatefulInteractiveElement as _, Styled, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, Root, Sizable as _, StyledExt as _, h_flex, v_flex};

use crate::models::WorkspaceLayoutMode;
use crate::ui::app::{
    NavSection, TermiRustApp, WorkspaceIndicators, WorkspaceTab, WorkspaceTabDrag,
    WorkspaceTabDragPreview, nav_section_key,
};
use crate::ui::localization;
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
                    .size(px(theme::SHELL_TRAFFIC_LIGHT_SIZE))
                    .rounded(px(theme::PILL_RADIUS))
                    .bg(theme::danger())
                    .into_any_element(),
            );
        } else if indicators.connecting_panes > 0 {
            nodes.push(
                div()
                    .size(px(theme::SHELL_TRAFFIC_LIGHT_SIZE))
                    .rounded(px(theme::PILL_RADIUS))
                    .bg(theme::warning())
                    .into_any_element(),
            );
        } else if indicators.closed_panes > 0 && indicators.live_panes == 0 {
            nodes.push(
                div()
                    .size(px(theme::SHELL_TRAFFIC_LIGHT_SIZE))
                    .rounded(px(theme::PILL_RADIUS))
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
            .gap(px(theme::SPACE_3))
            .items_center()
            .pl(px(theme::SPACE_4))
            .pr(if close_button.is_some() {
                px(theme::SHELL_SPACE_DENSE)
            } else {
                px(theme::SPACE_4)
            })
            .h(px(theme::SHELL_COMPACT_CONTROL_HEIGHT))
            .rounded(px(theme::SHELL_SPACE_DENSE))
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
            .when(active, |this| this.shadow(theme::popover_shadow()))
            .when(!active, |this| {
                this.hover(|style| style.bg(theme::chrome_tab()))
            })
            .child(
                icon.size(px(theme::ICON_SIZE_DEFAULT))
                    .text_color(if active {
                        theme::accent()
                    } else {
                        theme::text_muted_dark()
                    }),
            )
            .child(
                div()
                    .max_w(px(theme::SHELL_TAB_LABEL_MAXIMUM))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
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
            .h(px(theme::SHELL_COMPACT_CONTROL_HEIGHT))
            .px(px(theme::SHELL_SPACE_COMPACT))
            .gap(px(theme::SHELL_SPACE_TIGHT))
            .items_center()
            .rounded(px(theme::CARD_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.72)))
            .child(
                Icon::new(icon)
                    .size(px(theme::ICON_SIZE_DEFAULT))
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
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
            .w(px(theme::SHELL_WORKSPACE_MENU_WIDTH))
            .p(px(theme::SHELL_SPACE_DENSE))
            .gap(px(theme::SPACE_1))
            .rounded(px(theme::SHELL_SPACE_COMPACT))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                self.workspace_tab_menu_item(
                    ("workspace-tab-menu-duplicate", workspace_id),
                    IconName::Copy,
                    "Duplicate",
                    move |this, window, cx| {
                        this.duplicate_workspace_in_new_tab(workspace_id, window, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("workspace-tab-menu-duplicate-{}", workspace_id)),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("workspace-tab-menu-duplicate-window", workspace_id),
                    IconName::ExternalLink,
                    "Duplicate in a new window",
                    move |this, window, cx| {
                        this.duplicate_workspace_in_new_window(workspace_id, window, cx);
                    },
                    cx,
                )
                .debug_selector(move || {
                    format!("workspace-tab-menu-duplicate-window-{}", workspace_id)
                }),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("workspace-tab-menu-rename", workspace_id),
                    IconName::ALargeSmall,
                    "Rename",
                    move |this, window, cx| {
                        this.start_workspace_rename_for(workspace_id, window, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("workspace-tab-menu-rename-{}", workspace_id)),
            )
            .when(can_split, |this| {
                this.child(
                    self.workspace_tab_menu_item(
                        ("workspace-tab-menu-split-horizontal", workspace_id),
                        IconName::PanelRight,
                        "Split horizontally",
                        move |this, window, cx| {
                            this.split_workspace_horizontally(workspace_id, window, cx);
                        },
                        cx,
                    )
                    .debug_selector(move || {
                        format!("workspace-tab-menu-split-horizontal-{}", workspace_id)
                    }),
                )
            })
            .child(
                div()
                    .h(px(theme::BORDER_HAIRLINE))
                    .my(px(theme::SPACE_1))
                    .bg(theme::soft_border()),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("workspace-tab-menu-close", workspace_id),
                    IconName::Close,
                    "Close",
                    move |this, _, cx| {
                        this.open_workspace_tab_menu = None;
                        this.close_workspace(workspace_id, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("workspace-tab-menu-close-{}", workspace_id)),
            )
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
            .w(px(theme::SHELL_PANE_MENU_WIDTH))
            .p(px(theme::SHELL_SPACE_DENSE))
            .gap(px(theme::SPACE_1))
            .rounded(px(theme::SHELL_SPACE_COMPACT))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                self.workspace_tab_menu_item(
                    ("pane-menu-copy", pane_id),
                    IconName::Copy,
                    "Copy",
                    move |this, _, cx| {
                        this.pane_context_menu = None;
                        this.copy_active_selection(cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("pane-menu-copy-{}", pane_id)),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("pane-menu-paste", pane_id),
                    IconName::File,
                    "Paste",
                    move |this, _, cx| {
                        this.pane_context_menu = None;
                        this.paste_to_active_pane(cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("pane-menu-paste-{}", pane_id)),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("pane-menu-clear", pane_id),
                    IconName::Replace,
                    "Clear",
                    move |this, _, cx| {
                        this.pane_context_menu = None;
                        this.clear_pane_screen(pane_id, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("pane-menu-clear-{}", pane_id)),
            )
            .child(
                self.workspace_tab_menu_item(
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
                )
                .debug_selector(move || format!("pane-menu-duplicate-{}", pane_id)),
            )
            .child(
                div()
                    .h(px(theme::BORDER_HAIRLINE))
                    .my(px(theme::SPACE_1))
                    .bg(theme::soft_border()),
            )
            .when(closed, |this| {
                this.child(
                    self.workspace_tab_menu_item(
                        ("pane-menu-reconnect", pane_id),
                        IconName::Redo,
                        "Reconnect",
                        move |this, window, cx| {
                            this.pane_context_menu = None;
                            this.reconnect_pane(pane_id, window, cx);
                        },
                        cx,
                    )
                    .debug_selector(move || format!("pane-menu-reconnect-{}", pane_id)),
                )
            })
            .child(
                self.workspace_tab_menu_item(
                    ("pane-menu-detach", pane_id),
                    IconName::ExternalLink,
                    "Detach to new tab",
                    move |this, window, cx| {
                        this.pane_context_menu = None;
                        this.move_pane_to_new_workspace(pane_id, window, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("pane-menu-detach-{}", pane_id)),
            )
            .child(
                self.workspace_tab_menu_item(
                    ("pane-menu-close", pane_id),
                    IconName::Close,
                    "Close pane",
                    move |this, _, cx| {
                        this.pane_context_menu = None;
                        this.close_pane(pane_id, cx);
                    },
                    cx,
                )
                .debug_selector(move || format!("pane-menu-close-{}", pane_id)),
            )
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
        let bounds = Bounds::centered(
            None,
            size(
                px(theme::WINDOW_DEFAULT_WIDTH),
                px(theme::WINDOW_DEFAULT_HEIGHT),
            ),
            cx,
        );

        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(localization::shell_app_title().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        px(theme::native_control_offset_x()),
                        px(theme::SPACE_3),
                    )),
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
                        app.status_message = localization::status_connecting(request.address());
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
                    localization::shell_duplicate_window_progress(request.address());
                self.error_message.clear();
                cx.notify();
            }
            Err(error) => {
                self.error_message = localization::shell_duplicate_window_error(error.to_string());
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
            .size(px(theme::ICON_SIZE_SMALL))
            .rounded(px(theme::SHELL_SPACE_DENSE))
            .bg(color)
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, window, _| action(window)))
    }

    /// The three custom traffic lights, as an inline group for the chrome row.
    fn render_window_controls(&self, cx: &mut Context<Self>) -> Div {
        h_flex()
            .flex_shrink_0()
            .gap(px(theme::SPACE_3))
            .items_center()
            .pr(px(theme::SHELL_SPACE_DENSE))
            .child(self.window_control_button(
                "window-close",
                theme::window_close(),
                |window| window.remove_window(),
                cx,
            ))
            .child(self.window_control_button(
                "window-minimize",
                theme::window_minimize(),
                |window| window.minimize_window(),
                cx,
            ))
            .child(self.window_control_button(
                "window-zoom",
                theme::window_zoom(),
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
            .gap(px(theme::SHELL_SPACE_DENSE))
            .items_center()
            .pl(px(theme::SHELL_SPACE_COMPACT))
            .pr(px(theme::SHELL_SPACE_DENSE))
            .h(px(theme::SHELL_COMPACT_CONTROL_HEIGHT))
            .rounded(px(theme::SHELL_SPACE_DENSE))
            .border_1()
            .border_color(theme::accent())
            .bg(theme::chrome_tab_active())
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(theme::ICON_SIZE_DEFAULT))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .w(px(theme::SHELL_RENAME_FIELD_WIDTH))
                    .child(Input::new(&self.tab_rename_input).small()),
            )
    }

    pub(super) fn render_top_chrome(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library_active = self.active_workspace_id.is_none();
        let active_layout_mode = self
            .active_workspace()
            .map(|workspace| workspace.layout_mode);

        h_flex()
            .h(px(theme::CHROME_HEIGHT))
            .w_full()
            .relative()
            .pl(px(theme::TYPE_BODY_SMALL_SIZE))
            .pr(px(theme::SPACE_4))
            .gap(px(theme::SHELL_SPACE_DENSE))
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
                .debug_selector(|| "chrome-hosts".to_string())
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
                .debug_selector(|| "chrome-sftp".to_string())
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
                        this.open_files_library(
                            super::artifact_gallery::FilesLibraryTab::Sftp,
                            window,
                            cx,
                        );
                    }
                })),
            )
            .when(!self.workspaces.is_empty(), |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .w(px(theme::BORDER_HAIRLINE))
                        .h(px(theme::SHELL_NAV_BADGE_HEIGHT))
                        .mx(px(theme::SPACE_2))
                        .bg(theme::with_alpha(theme::text_muted_dark(), 0.2)),
                )
            })
            .child(
                h_flex()
                    .id("chrome-tab-scroll")
                    .track_scroll(&self.tab_strip_scroll)
                    .overflow_x_scroll()
                    .h_full()
                    .min_w(px(theme::SPACE_0))
                    .gap(px(theme::SHELL_SPACE_DENSE))
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
                                    .debug_selector(move || {
                                        format!("chrome-workspace-close-{}", workspace.id)
                                    })
                                    .size(px(theme::ICON_SIZE_MEDIUM))
                                    .rounded(px(theme::SPACE_2))
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
                                    .child(
                                        Icon::new(IconName::Close).size(px(theme::ICON_SIZE_SMALL)),
                                    )
                                    .into_any_element(),
                            ),
                        )
                        .debug_selector(move || format!("chrome-workspace-{}", workspace_id))
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
                                    .ml(px(theme::SPACE_1))
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
                    .min_w(px(theme::SHELL_TAB_DROP_MINIMUM))
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
            .when_some(active_layout_mode, |this, layout_mode| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap(px(theme::SPACE_1))
                        .p(px(theme::SPACE_1))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme::with_alpha(theme::border_dark(), 0.7))
                        .bg(theme::terminal_panel())
                        .child(
                            Button::new("chrome-layout-split")
                                .xsmall()
                                .custom(Self::segmented_button_style(
                                    layout_mode == WorkspaceLayoutMode::Split,
                                    cx,
                                ))
                                .icon(IconName::LayoutDashboard)
                                .label(localization::shell_layout_split_label())
                                .tooltip(localization::shell_layout_split_tooltip())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_workspace_layout_mode(
                                        WorkspaceLayoutMode::Split,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new("chrome-layout-canvas")
                                .xsmall()
                                .custom(Self::segmented_button_style(
                                    layout_mode == WorkspaceLayoutMode::Canvas,
                                    cx,
                                ))
                                .icon(IconName::Map)
                                .label(localization::shell_layout_canvas_label())
                                .tooltip(localization::shell_layout_canvas_tooltip())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_workspace_layout_mode(
                                        WorkspaceLayoutMode::Canvas,
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("chrome-local-btn")
                    .debug_selector(|| "chrome-local-btn".to_string())
                    .flex_shrink_0()
                    .size(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
                    .rounded(px(theme::CARD_RADIUS))
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
                            .size(px(theme::ICON_SIZE_DEFAULT))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
            .child(
                div()
                    .id("chrome-new-btn")
                    .debug_selector(|| "chrome-new-btn".to_string())
                    .flex_shrink_0()
                    .size(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
                    .rounded(px(theme::CARD_RADIUS))
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
                            .size(px(theme::ICON_SIZE_DEFAULT))
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
            .debug_selector(|| format!("nav-card-{}", nav_section_key(section)))
            .w_full()
            .items_center()
            .gap(px(theme::SHELL_SPACE_COMPACT))
            .px(px(theme::SPACE_4))
            .h(px(theme::SHELL_NAVIGATION_ROW_HEIGHT))
            .rounded(px(theme::SPACE_3))
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
            .child(
                section
                    .icon()
                    .size(px(theme::ICON_SIZE_DEFAULT))
                    .text_color(if active {
                        theme::accent()
                    } else {
                        theme::text_muted()
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_medium()
                    .text_color(if active {
                        theme::accent()
                    } else {
                        theme::text_main()
                    })
                    .child(section.label()),
            )
            .when(
                section == NavSection::Activity && self.activity_center.visible_count() > 0,
                |this| {
                    this.child(
                        div()
                            .min_w(px(theme::SHELL_NAV_BADGE_WIDTH))
                            .h(px(theme::SHELL_NAV_BADGE_HEIGHT))
                            .px_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::SPACE_3))
                            .bg(theme::accent())
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .font_semibold()
                            .text_color(theme::library_bg())
                            .child(self.activity_center.visible_count().min(99).to_string()),
                    )
                },
            )
    }

    pub(super) fn render_library_sidebar(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("library-sidebar")
            .debug_selector(|| "library-sidebar".to_string())
            .w(px(theme::HOST_SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .min_h_0()
            .px(px(theme::SPACE_4))
            .pt(px(theme::SPACE_5))
            .pb(px(theme::SPACE_5))
            .bg(theme::library_sidebar())
            .overflow_y_scroll()
            .child(
                v_flex().gap(px(theme::SPACE_1)).children(
                    [
                        NavSection::Activity,
                        NavSection::Projects,
                        NavSection::Hosts,
                        NavSection::Sessions,
                        NavSection::Sftp,
                        NavSection::Devices,
                        NavSection::Settings,
                    ]
                    .into_iter()
                    .map(|section| {
                        let active = self.nav_section == section;
                        self.nav_card(("nav-card", nav_section_key(section)), section, active, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if section == NavSection::Sftp {
                                    this.open_files_library(
                                        super::artifact_gallery::FilesLibraryTab::Artifacts,
                                        window,
                                        cx,
                                    );
                                } else {
                                    this.activate_library_section(section, window, cx);
                                }
                            }))
                            .into_any_element()
                    }),
                ),
            )
            .child(
                div()
                    .h(px(theme::BORDER_HAIRLINE))
                    .w_full()
                    .my(px(theme::SPACE_3))
                    .bg(theme::with_alpha(theme::border(), 0.6)),
            )
            .child(
                v_flex().gap(px(theme::SPACE_1)).children(
                    [
                        NavSection::Presets,
                        NavSection::Vaults,
                        NavSection::Keychain,
                        NavSection::Snippets,
                        NavSection::KnownHosts,
                        NavSection::Logs,
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
            .into_any_element()
    }
}
