//! Top tab bar (chrome) + library sidebar. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, point, px, AnyElement, AppContext as _, ClickEvent, Context, Div, DragMoveEvent,
    ElementId, InteractiveElement as _, IntoElement, MouseButton, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{h_flex, v_flex, Disableable, Icon, IconName, Sizable, StyledExt as _};

use crate::ui::app::{
    app_icon, nav_section_key, NavSection, TermiRustApp, WorkspaceIndicators, WorkspaceTab,
    WorkspaceTabDrag, WorkspaceTabDragPreview, ICON_PANEL_COLLAPSE_RIGHT,
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

    pub(super) fn render_top_chrome(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library_active = self.active_workspace_id.is_none();

        h_flex()
            .h(px(theme::CHROME_HEIGHT))
            .w_full()
            .pl(px(theme::CHROME_INSET_LEFT))
            .pr(px(12.))
            .gap(px(4.))
            .items_center()
            .bg(theme::chrome_bg())
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
                        this.reorder_workspace_tabs(drag.workspace_id, Some(workspace_id), false);
                        this.error_message.clear();
                        cx.notify();
                    }
                }))
                .cursor_grab()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_workspace(workspace_id, window, cx);
                }))
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
                        this.activate_library(window, cx);
                        this.open_editor_for_new_host(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(14.))
                            .text_color(theme::text_muted_dark()),
                    ),
            )
            .child(
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
