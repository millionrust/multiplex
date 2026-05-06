//! Right-side terminal companion panel: Quick / Snippets / History / Themes
//! tabs and the panel-tab button helper. Methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, Hsla, InteractiveElement as _, IntoElement, ParentElement, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, StyledExt as _, h_flex, v_flex};

use crate::models::ThemePreset;
use crate::ui::app::{ICON_PALETTE, NavSection, TermiRustApp, TerminalPanelTab, app_icon};
use crate::ui::theme;

impl TermiRustApp {
    pub(super) fn render_terminal_side_panel(&self, cx: &mut Context<Self>) -> Div {
        let active = self.terminal_panel_tab;
        v_flex()
            .flex_none()
            .w(px(300.))
            .h_full()
            .bg(theme::library_bg())
            .border_l_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .h(px(48.))
                    .px(px(14.))
                    .gap(px(12.))
                    .items_center()
                    .child(self.terminal_panel_tab_button(
                        "term-panel-quick",
                        TerminalPanelTab::Quick,
                        Icon::new(IconName::SquareTerminal),
                        active,
                        cx,
                    ))
                    .child(self.terminal_panel_tab_button(
                        "term-panel-snippets",
                        TerminalPanelTab::Snippets,
                        Icon::new(IconName::BookOpen),
                        active,
                        cx,
                    ))
                    .child(self.terminal_panel_tab_button(
                        "term-panel-history",
                        TerminalPanelTab::History,
                        Icon::new(IconName::Calendar),
                        active,
                        cx,
                    ))
                    .child(self.terminal_panel_tab_button(
                        "term-panel-themes",
                        TerminalPanelTab::Themes,
                        app_icon(ICON_PALETTE),
                        active,
                        cx,
                    )),
            )
            .child(v_flex().flex_1().min_h_0().child(match active {
                TerminalPanelTab::Quick => self.render_terminal_panel_quick(cx).into_any_element(),
                TerminalPanelTab::Snippets => {
                    self.render_terminal_panel_snippets(cx).into_any_element()
                }
                TerminalPanelTab::History => {
                    self.render_terminal_panel_history(cx).into_any_element()
                }
                TerminalPanelTab::Themes => {
                    self.render_terminal_panel_themes(cx).into_any_element()
                }
            }))
    }

    fn terminal_panel_tab_button(
        &self,
        id: &'static str,
        tab: TerminalPanelTab,
        icon: Icon,
        active: TerminalPanelTab,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let is_active = active == tab;
        div()
            .id(id)
            .size(px(32.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if is_active {
                theme::with_alpha(theme::accent(), 0.2)
            } else {
                gpui::transparent_black()
            })
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.6)))
            .child(icon.size(px(15.)).text_color(if is_active {
                theme::accent()
            } else {
                theme::text_muted()
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.terminal_panel_tab = tab;
                cx.notify();
            }))
    }

    fn render_terminal_panel_quick(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .px(px(14.))
            .pt(px(8.))
            .gap(px(10.))
            .child(
                h_flex()
                    .h(px(36.))
                    .px(px(10.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(13.))
                            .text_color(theme::text_muted()),
                    )
                    .child(
                        Input::new(&self.shell_inputs.terminal_search)
                            .appearance(false)
                            .flex_1(),
                    )
                    .child(
                        h_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .id("term-panel-search-up")
                                    .size(px(20.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.6)))
                                    .child(
                                        Icon::new(IconName::ChevronUp)
                                            .size(px(11.))
                                            .text_color(theme::text_muted()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.jump_workspace_search(-1, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("term-panel-search-down")
                                    .size(px(20.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.6)))
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(px(11.))
                                            .text_color(theme::text_muted()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.jump_workspace_search(1, cx);
                                    })),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(theme::soft_border()))
            .child(
                h_flex()
                    .px(px(2.))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child("Autocomplete"),
                            )
                            .child(
                                div()
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(theme::with_alpha(theme::accent(), 0.18))
                                    .text_size(px(9.))
                                    .font_semibold()
                                    .text_color(theme::accent())
                                    .child("BETA"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(2.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child("Disabled"),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(11.))
                                    .text_color(theme::text_muted()),
                            ),
                    ),
            )
    }

    fn render_terminal_panel_snippets(&self, cx: &mut Context<Self>) -> Div {
        let snippets = self.saved.snippets.clone();
        v_flex()
            .px(px(14.))
            .pt(px(8.))
            .gap(px(10.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .id("term-panel-new-snippet")
                            .h(px(28.))
                            .px(px(10.))
                            .gap(px(6.))
                            .items_center()
                            .rounded(px(6.))
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_main())
                                    .child("{}"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_main())
                                    .child("New Snippet"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.activate_library_section(NavSection::Snippets, window, cx);
                            })),
                    )
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(13.))
                            .text_color(theme::text_muted()),
                    ),
            )
            .child(div().h(px(1.)).bg(theme::soft_border()))
            .child(if snippets.is_empty() {
                v_flex()
                    .pt(px(80.))
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(48.))
                            .rounded(px(10.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(theme::text_main())
                                    .child("{}"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Create snippets from your commands"),
                    )
                    .child(
                        div()
                            .max_w(px(220.))
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("Store your most used commands to reuse them in one click."),
                    )
                    .into_any_element()
            } else {
                v_flex()
                    .gap(px(4.))
                    .children(snippets.iter().enumerate().map(|(idx, snippet)| {
                        let cmd = snippet.command.clone();
                        h_flex()
                            .id(("term-snippet", idx))
                            .h(px(36.))
                            .px(px(10.))
                            .gap(px(8.))
                            .items_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(theme::text_main())
                                    .child(if snippet.label.is_empty() {
                                        cmd.clone()
                                    } else {
                                        snippet.label.clone()
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.send_command_to_active_pane(cmd.clone(), window, cx);
                            }))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
    }

    fn render_terminal_panel_history(&self, cx: &mut Context<Self>) -> Div {
        let mut history: Vec<String> = self.saved.command_history.clone();
        history.dedup();
        v_flex()
            .px(px(14.))
            .pt(px(8.))
            .gap(px(8.))
            .child(
                h_flex()
                    .h(px(36.))
                    .px(px(10.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(13.))
                            .text_color(theme::text_muted()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child("Search"),
                    ),
            )
            .child(if history.is_empty() {
                v_flex()
                    .pt(px(80.))
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("No commands yet — they'll appear here as you type."),
                    )
                    .into_any_element()
            } else {
                v_flex()
                    .gap(px(2.))
                    .children(history.iter().enumerate().take(50).map(|(idx, cmd)| {
                        let cmd_owned = cmd.clone();
                        h_flex()
                            .id(("term-history", idx))
                            .h(px(28.))
                            .px(px(10.))
                            .items_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_main())
                                    .child(cmd.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.send_command_to_active_pane(cmd_owned.clone(), window, cx);
                            }))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
    }

    fn render_terminal_panel_themes(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.saved.settings.theme_preset;
        let mut wrapper = v_flex()
            .id("term-panel-themes-scroll")
            .px(px(14.))
            .pt(px(8.))
            .pb(px(20.))
            .gap(px(8.))
            .overflow_y_scroll()
            .child(
                h_flex()
                    .py(px(6.))
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_main())
                            .child("Font"),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(12.))
                            .text_color(theme::text_muted()),
                    ),
            )
            .child(div().h(px(1.)).bg(theme::soft_border()))
            .child(
                div()
                    .text_size(px(11.))
                    .font_semibold()
                    .text_color(theme::text_muted())
                    .pt(px(2.))
                    .child("Themes"),
            );
        for preset in ThemePreset::all() {
            wrapper = wrapper.child(self.theme_card(preset, active, cx));
        }
        wrapper
    }

    fn theme_card(
        &self,
        preset: ThemePreset,
        active: ThemePreset,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let is_active = preset == active;
        let label = preset.label().to_string();
        let bg_color: Hsla = gpui::rgb(preset.preview_bg()).into();
        let accent_color: Hsla = gpui::rgb(preset.preview_accent()).into();
        h_flex()
            .id(("theme-card", preset as usize))
            .h(px(56.))
            .px(px(8.))
            .gap(px(10.))
            .items_center()
            .rounded(px(8.))
            .bg(if is_active {
                theme::with_alpha(theme::accent(), 0.12)
            } else {
                gpui::transparent_black()
            })
            .border_1()
            .border_color(if is_active {
                theme::with_alpha(theme::accent(), 0.5)
            } else {
                theme::soft_border()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
            .child(
                v_flex()
                    .w(px(72.))
                    .h(px(40.))
                    .rounded(px(6.))
                    .pl(px(8.))
                    .pt(px(8.))
                    .gap(px(4.))
                    .bg(bg_color)
                    .border_1()
                    .border_color(if is_active {
                        theme::accent()
                    } else {
                        theme::soft_border()
                    })
                    .child(div().w(px(40.)).h(px(2.)).rounded(px(2.)).bg(accent_color))
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(2.))
                            .rounded(px(2.))
                            .bg(theme::with_alpha(accent_color, 0.5)),
                    )
                    .child(
                        div()
                            .w(px(34.))
                            .h(px(2.))
                            .rounded(px(2.))
                            .bg(theme::with_alpha(accent_color, 0.7)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_semibold()
                            .text_color(if is_active {
                                theme::accent()
                            } else {
                                theme::text_main()
                            })
                            .child(label),
                    )
                    .when(is_active, |this| {
                        this.child(
                            div()
                                .text_size(px(10.))
                                .text_color(theme::text_muted())
                                .child("Selected"),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.saved.settings.theme_preset = preset;
                theme::set_theme_preset(preset);
                this.persist_runtime_state();
                cx.notify();
            }))
    }

    pub(super) fn send_command_to_active_pane(
        &mut self,
        command: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.active_pane().map(|p| p.id) else {
            return;
        };
        let mut bytes = command.into_bytes();
        bytes.push(b'\n');
        self.send_input_bytes(pane_id, bytes, cx);
    }
}
