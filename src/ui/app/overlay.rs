//! Overlay panels rendered on top of the workspace: snippet-prompts banner,
//! multi-line paste confirmation, and the command palette modal.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::ui::app::{TermiRustApp, primary_shortcut_label};
use crate::ui::autocomplete::AutocompleteSource;
use crate::ui::theme;

impl TermiRustApp {
    pub(super) fn render_snippet_prompts_panel(&self, cx: &Context<Self>) -> Option<Div> {
        let prompts = self.pending_snippet_prompts.as_ref()?;
        let preview: SharedString = prompts
            .command
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>()
            .into();
        Some(
            v_flex()
                .w_full()
                .px(px(18.))
                .py(px(10.))
                .gap_2()
                .bg(theme::with_alpha(theme::accent(), 0.16))
                .border_b_1()
                .border_color(theme::with_alpha(theme::accent(), 0.45))
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_semibold()
                                        .text_color(theme::text_on_dark())
                                        .child("Snippet prompts"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted_dark())
                                        .child(format!("Command: {preview}")),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("snippet-prompts-run")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label("Run")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_snippet_prompts(cx);
                                        })),
                                )
                                .child(
                                    Button::new("snippet-prompts-cancel")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_snippet_prompts(cx);
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .children(prompts.fields.iter().map(|field| {
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(theme::text_on_dark())
                                        .child(field.name.clone()),
                                )
                                .child(Input::new(&field.input).small())
                                .into_any_element()
                        })),
                ),
        )
    }

    pub(super) fn render_paste_confirmation(&self, cx: &Context<Self>) -> Option<Div> {
        let pending = self.pending_paste.as_ref()?;
        let line_count = pending.text.matches('\n').count() + 1;
        let preview = pending
            .text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        Some(
            h_flex()
                .w_full()
                .px(px(18.))
                .py(px(8.))
                .gap_2()
                .items_center()
                .justify_between()
                .bg(theme::with_alpha(theme::warning(), 0.16))
                .border_b_1()
                .border_color(theme::with_alpha(theme::warning(), 0.45))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_medium()
                                .text_color(theme::text_on_dark())
                                .child(format!("Paste {line_count} lines into the active pane?")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted_dark())
                                .child(format!("First line: {preview}…")),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("paste-confirm")
                                .debug_selector(|| "paste-confirm".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label("Paste")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_pending_paste(cx);
                                })),
                        )
                        .child(
                            Button::new("paste-cancel")
                                .debug_selector(|| "paste-cancel".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label("Cancel")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_pending_paste(cx);
                                })),
                        ),
                ),
        )
    }

    pub(super) fn render_command_palette(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let candidates = self.command_palette_candidates(cx);
        let selected_index = self.selected_command_palette_index(candidates.len());
        let query = self.command_palette_query(cx);

        div()
            .id("command-palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(88.))
            .bg(theme::modal_scrim())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_command_palette(window, cx);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_command_palette_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                v_flex()
                    .id("command-palette-card")
                    .w(px(720.))
                    .max_w(relative(0.9))
                    .max_h(px(560.))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .border_b_1()
                            .border_color(theme::border())
                            .bg(theme::with_alpha(theme::hover(), 0.5))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child("Command Palette"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(format!(
                                                "{}+K toggle  ↑/↓ move  Enter run",
                                                primary_shortcut_label()
                                            )),
                                    ),
                            )
                            .child(Input::new(&self.shell_inputs.command_palette).w_full()),
                    )
                    .child(
                        v_flex()
                            .max_h(px(400.))
                            .overflow_y_scrollbar()
                            .p_3()
                            .gap_2()
                            .when(!candidates.is_empty(), |this| {
                                this.children(candidates.iter().enumerate().map(|(index, candidate)| {
                                    let command = candidate.command.clone();
                                    let selected = index == selected_index;
                                    h_flex()
                                        .id(("command-palette-item", index))
                                        .justify_between()
                                        .items_start()
                                        .gap_3()
                                        .p_3()
                                        .rounded(px(12.))
                                        .bg(if selected {
                                            theme::with_alpha(theme::accent(), 0.1)
                                        } else {
                                            theme::with_alpha(theme::hover(), 0.72)
                                        })
                                        .border_1()
                                        .border_color(if selected {
                                            theme::with_alpha(theme::accent(), 0.42)
                                        } else {
                                            theme::border()
                                        })
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme::hover()))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.run_command_in_active_pane(
                                                &command,
                                                "Command sent to the active session.",
                                                cx,
                                            ) {
                                                this.close_command_palette(window, cx);
                                            }
                                        }))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .gap(px(4.))
                                                .child(
                                                    h_flex()
                                                        .justify_between()
                                                        .items_center()
                                                        .gap_3()
                                                        .child(
                                                            div()
                                                                .text_size(px(14.))
                                                                .font_semibold()
                                                                .text_color(theme::text_main())
                                                                .child(candidate.title.clone()),
                                                        )
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .when(candidate.pinned, |this| {
                                                                    this.child(self.status_badge(
                                                                        "Pinned",
                                                                        theme::library_bg(),
                                                                        theme::warning(),
                                                                    ))
                                                                })
                                                                .child(self.status_badge(
                                                                    candidate.source.label(),
                                                                    theme::library_bg(),
                                                                    match candidate.source {
                                                                        AutocompleteSource::Path => {
                                                                            theme::warning()
                                                                        }
                                                                        AutocompleteSource::Context => {
                                                                            theme::accent()
                                                                        }
                                                                        AutocompleteSource::Argument => {
                                                                            theme::warning()
                                                                        }
                                                                        AutocompleteSource::History => {
                                                                            theme::accent()
                                                                        }
                                                                        AutocompleteSource::Snippet => {
                                                                            theme::success()
                                                                        }
                                                                        AutocompleteSource::Builtin => {
                                                                            theme::slate()
                                                                        }
                                                                    },
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .text_color(theme::text_muted())
                                                        .child(candidate.detail.clone()),
                                                ),
                                        )
                                        .into_any_element()
                                }))
                            })
                            .when(candidates.is_empty(), |this| {
                                this.child(
                                    v_flex()
                                        .items_center()
                                        .justify_center()
                                        .p_8()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::Search)
                                                .size(px(24.))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.45)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .font_medium()
                                                .text_color(theme::text_muted())
                                                .child(if query.is_empty() {
                                                    "No commands yet"
                                                } else {
                                                    "No matching commands"
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                                .child(if query.is_empty() {
                                                    "Run a few commands or save snippets to build the palette."
                                                } else {
                                                    "Try a command prefix, snippet name, or recent task."
                                                }),
                                        ),
                                )
                            }),
                    ),
            )
    }
}
