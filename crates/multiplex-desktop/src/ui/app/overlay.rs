//! Workspace overlays: autocomplete and the command palette.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Stateful, StatefulInteractiveElement as _, Styled, Window, div, px, relative,
};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, StyledExt as _, h_flex, v_flex};

use crate::ui::app::global_search::{
    category_label, global_search_failure_message, search_status_label,
};
use crate::ui::app::palette::{PaletteAction, PaletteCategory};
use crate::ui::app::{MultiplexApp, primary_shortcut_label};
use crate::ui::autocomplete::AutocompleteSource;
use crate::ui::localization;
use crate::ui::theme;

/// How many suggestions the bar shows at once. Enough to choose from without taking the
/// terminal over.
const VISIBLE_AUTOCOMPLETE_SUGGESTIONS: usize = 5;

impl MultiplexApp {
    pub(super) fn render_autocomplete_suggestions(&self) -> Option<Stateful<Div>> {
        let candidates = self.workspace_autocomplete_candidates();
        if candidates.is_empty() {
            return None;
        }
        let chosen = self
            .active_pane()
            .and_then(|pane| pane.selected_autocomplete_index);
        Some(
            h_flex()
                .id("terminal-autocomplete")
                .debug_selector(|| "terminal-autocomplete".to_string())
                .w_full()
                .px(px(theme::SHELL_BANNER_HORIZONTAL))
                .py(px(theme::SPACE_2))
                .gap_2()
                .items_center()
                .flex_wrap()
                .bg(theme::with_alpha(theme::accent(), 0.10))
                .border_b_1()
                .border_color(theme::with_alpha(theme::accent(), 0.35))
                .children(
                    candidates
                        .into_iter()
                        .take(VISIBLE_AUTOCOMPLETE_SUGGESTIONS)
                        .enumerate()
                        .map(|(index, candidate)| {
                            let marked = chosen == Some(index);
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .px_2()
                                .py_0p5()
                                .rounded(px(theme::CONTROL_RADIUS))
                                .when(marked, |this| {
                                    this.bg(theme::with_alpha(theme::accent(), 0.28))
                                })
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(if marked {
                                            theme::text_main()
                                        } else {
                                            theme::text_muted()
                                        })
                                        .child(candidate.command.clone()),
                                )
                                .when_some(candidate.scope_label.clone(), |this, scope| {
                                    // Which host or session the suggestion came from, when it
                                    // came from one in particular.
                                    this.child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(scope),
                                    )
                                })
                                .child(self.status_badge(
                                    candidate.source.label(),
                                    theme::library_bg(),
                                    source_tone(candidate.source),
                                ))
                                .into_any_element()
                        }),
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
        let searching = self.global_search.searching;
        let archived_fallback = self.global_search.archived_fallback;
        let failure = self.global_search.failure;
        let skipped_documents = self.global_search.skipped_documents;

        div()
            .id("command-palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(theme::PALETTE_OFFSET_TOP))
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
                    .w(px(theme::PALETTE_WIDTH))
                    .max_w(relative(0.94))
                    .max_h(relative(0.84))
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
                                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child(localization::global_palette_title()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::global_palette_shortcut_hint(
                                                primary_shortcut_label(),
                                            )),
                                    ),
                            )
                            .child(Input::new(&self.shell_inputs.command_palette).w_full())
                            .when(searching || archived_fallback || failure.is_some(), |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted())
                                        .when(searching, |this| {
                                            this.child(
                                                Icon::new(IconName::LoaderCircle)
                                                    .size(px(theme::ICON_SIZE_SMALL))
                                                    .text_color(theme::accent()),
                                            )
                                            .child(localization::global_palette_searching())
                                        })
                                        .when(archived_fallback, |this| {
                                            this.child(localization::global_palette_archived_fallback())
                                        })
                                        .when_some(failure, |this, failure| {
                                            this.child(global_search_failure_message(
                                                failure,
                                                skipped_documents,
                                            ))
                                        }),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .p_3()
                            .gap_1()
                            .when(!candidates.is_empty(), |this| {
                                this.children(candidates.iter().enumerate().map(|(index, candidate)| {
                                    let selected = index == selected_index;
                                    let category_changed = index == 0
                                        || candidates[index - 1].category != candidate.category;
                                    let status = candidate.status.filter(|status| {
                                        *status != multiplex_domain::SearchStatus::Unknown
                                    });
                                    let source = candidate.source;
                                    let is_command = candidate.action == PaletteAction::RunCommand;
                                    let category = candidate.category;
                                    v_flex()
                                        .gap_1()
                                        .when(category_changed, |this| {
                                            this.child(
                                                div()
                                                    .id((
                                                        "command-palette-category",
                                                        category.rank() as usize,
                                                    ))
                                                    .pt_2()
                                                    .px_2()
                                                    .pb_1()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .font_semibold()
                                                    .text_color(theme::text_muted())
                                                    .child(category_label(category)),
                                            )
                                        })
                                        .child(
                                            h_flex()
                                                .id(("command-palette-item", index))
                                                .justify_between()
                                                .items_start()
                                                .gap_3()
                                                .px_3()
                                                .py(px(theme::SHELL_SPACE_COMPACT))
                                                .rounded(px(theme::CARD_RADIUS))
                                                .bg(if selected {
                                                    theme::with_alpha(theme::accent(), 0.12)
                                                } else {
                                                    theme::with_alpha(theme::hover(), 0.58)
                                                })
                                                .border_1()
                                                .border_color(if selected {
                                                    theme::with_alpha(theme::accent(), 0.55)
                                                } else {
                                                    theme::border()
                                                })
                                                .cursor_pointer()
                                                .hover(|style| style.bg(theme::hover()))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.activate_command_palette_candidate(
                                                        index, window, cx,
                                                    );
                                                }))
                                                .child(
                                                    Icon::new(category_icon(category))
                                                        .size(px(theme::ICON_SIZE_DEFAULT))
                                                        .text_color(if selected {
                                                            theme::accent()
                                                        } else {
                                                            theme::text_muted()
                                                        }),
                                                )
                                                .child(
                                                    v_flex()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .gap(px(theme::SPACE_2))
                                                        .child(render_palette_title(
                                                            &candidate.title,
                                                            &candidate.highlights,
                                                        ))
                                                        .when(!candidate.detail.is_empty(), |this| {
                                                            this.child(
                                                                div()
                                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                                    .text_color(theme::text_muted())
                                                                    .child(candidate.detail.clone()),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    h_flex()
                                                        .flex_wrap()
                                                        .justify_end()
                                                        .gap_2()
                                                        .items_center()
                                                        .when(candidate.pinned, |this| {
                                                            this.child(self.status_badge(
                                                                localization::global_palette_pinned(),
                                                                theme::library_bg(),
                                                                theme::warning(),
                                                            ))
                                                        })
                                                        .when_some(status, |this, status| {
                                                            this.child(self.status_kind_badge(
                                                                crate::ui::status::search_status(status),
                                                                search_status_label(status),
                                                            ))
                                                        })
                                                        .when(is_command, |this| {
                                                            this.child(self.status_badge(
                                                                source.label(),
                                                                theme::library_bg(),
                                                                source_tone(source),
                                                            ))
                                                        }),
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
                                                .size(px(theme::ICON_SIZE_LARGE))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.45)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                                .font_medium()
                                                .text_color(theme::text_muted())
                                                .child(if query.is_empty() {
                                                    localization::global_palette_empty()
                                                } else {
                                                    localization::global_palette_no_match()
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                                .text_color(theme::with_alpha(theme::text_muted(), 0.7))
                                                .child(if query.is_empty() {
                                                    localization::global_palette_empty_detail()
                                                } else {
                                                    localization::global_palette_no_match_detail()
                                                }),
                                        ),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_4()
                            .py_2()
                            .border_t_1()
                            .border_color(theme::border())
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::global_palette_shortcut_hint(
                                primary_shortcut_label(),
                            ))
                            .child(if candidates.is_empty() {
                                String::new()
                            } else {
                                localization::global_palette_position(
                                    selected_index + 1,
                                    candidates.len(),
                                )
                            }),
                    ),
            )
    }
}

fn source_tone(source: AutocompleteSource) -> gpui::Hsla {
    match source {
        AutocompleteSource::Path | AutocompleteSource::Argument => theme::warning(),
        AutocompleteSource::Context | AutocompleteSource::History => theme::accent(),
        AutocompleteSource::Snippet => theme::success(),
        AutocompleteSource::Builtin => theme::slate(),
    }
}

fn category_icon(category: PaletteCategory) -> IconName {
    match category {
        PaletteCategory::Attention => IconName::TriangleAlert,
        PaletteCategory::Sessions | PaletteCategory::Presets | PaletteCategory::Commands => {
            IconName::SquareTerminal
        }
        PaletteCategory::Groups => IconName::Folder,
        PaletteCategory::Actions => IconName::Plus,
        PaletteCategory::Archive => IconName::Inbox,
    }
}

fn render_palette_title(title: &str, highlights: &[multiplex_domain::TextHighlight]) -> AnyElement {
    let mut ranges = highlights
        .iter()
        .filter(|highlight| highlight.field == multiplex_domain::HighlightField::Title)
        .filter_map(|highlight| {
            (highlight.start < highlight.end
                && highlight.end <= title.len()
                && title.is_char_boundary(highlight.start)
                && title.is_char_boundary(highlight.end))
            .then_some((highlight.start, highlight.end))
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    ranges.dedup();

    let mut parts = Vec::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        if cursor < start {
            parts.push(
                div()
                    .text_color(theme::text_main())
                    .child(title[cursor..start].to_string())
                    .into_any_element(),
            );
        }
        parts.push(
            div()
                .font_semibold()
                .text_color(theme::accent())
                .child(title[start..end].to_string())
                .into_any_element(),
        );
        cursor = end;
    }
    if cursor < title.len() {
        parts.push(
            div()
                .text_color(theme::text_main())
                .child(title[cursor..].to_string())
                .into_any_element(),
        );
    }
    if parts.is_empty() {
        parts.push(
            div()
                .text_color(theme::text_main())
                .child(title.to_string())
                .into_any_element(),
        );
    }

    h_flex()
        .min_w_0()
        .flex_wrap()
        .text_size(px(theme::TYPE_BODY_SIZE))
        .font_medium()
        .children(parts)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use multiplex_ui_contract::{
        AnnouncementPolicy, FocusReturn, OverlayError, OverlayFrame, OverlayId, OverlayKind,
        OverlayOwner, OverlayPhase, OverlayStack, ShellRegionId,
    };

    fn frame(id: u64, kind: OverlayKind, owner: FocusReturn) -> OverlayFrame {
        OverlayFrame {
            id: OverlayId::new(id).unwrap(),
            kind,
            owner: OverlayOwner {
                window_generation: 7,
                owner,
            },
            focus_scope: id,
            safe_action: FocusReturn::Region(ShellRegionId::Content),
            announcements: AnnouncementPolicy::Coalesced,
            phase: OverlayPhase::Opening,
        }
    }

    #[test]
    fn nested_overlay_close_unwinds_children_and_uses_safe_focus_fallback() {
        let mut stack = OverlayStack::default();
        let parent = frame(1, OverlayKind::Dialog, FocusReturn::Exact(41));
        let child = frame(2, OverlayKind::Popover, FocusReturn::Exact(42));
        stack.open(parent).unwrap();
        stack.mark_open(parent.id, 7).unwrap();
        stack.open(child).unwrap();
        stack.mark_open(child.id, 7).unwrap();

        stack.begin_close(parent.id, 7).unwrap();
        let closed = stack.finish_close(parent.id, 7, |target| {
            target == FocusReturn::Region(ShellRegionId::Content)
        });

        assert_eq!(closed.unwrap().len(), 2);
        assert!(stack.frames().is_empty());
    }

    #[test]
    fn ordinary_overlay_cannot_cover_an_open_security_prompt() {
        let mut stack = OverlayStack::default();
        let security = frame(1, OverlayKind::SecurityPrompt, FocusReturn::FirstAvailable);
        stack.open(security).unwrap();
        stack.mark_open(security.id, 7).unwrap();

        assert_eq!(
            stack.open(frame(
                2,
                OverlayKind::GlobalPalette,
                FocusReturn::FirstAvailable,
            )),
            Err(OverlayError::SecurityPromptObscured)
        );
    }
}
