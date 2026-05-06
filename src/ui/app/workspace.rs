//! Workspace shell rendering: search bar, autocomplete bar, files (SFTP)
//! view, terminal pane (cells/rows), workspace body and shell wrapper.
//! All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Div, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    ScrollWheelEvent, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Window, div,
    point, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::ConnectionKind;
use crate::terminal::{TerminalRow, TerminalStyle};
use crate::ui::app::{
    ConnectDialogMode, EXPANDED_PANE_LIST_WIDTH, PANE_GAP, PANE_HEADER_HEIGHT, PaneDrag,
    PaneDragPreview, SearchMatch, SessionPane, SplitAxis, TERMINAL_INNER_PADDING_X,
    TERMINAL_INNER_PADDING_Y, TERMINAL_LINE_HEIGHT, TermiRustApp, WORKSPACE_AUTOCOMPLETE_HEIGHT,
    WORKSPACE_PADDING, WORKSPACE_SEARCH_ROW_HEIGHT, WorkspacePaneLayout, WorkspaceTab,
    WorkspaceTabDrag, WorkspaceViewMode, primary_shortcut_label,
};
use crate::ui::autocomplete::AutocompleteSource;
use crate::ui::path::format_file_size;
use crate::ui::render_terminal::{
    SelectionRange, default_terminal_style, display_terminal_text, selection_contains,
    style_for_render,
};
use crate::ui::theme;
use gpui_component::ActiveTheme as _;

impl TermiRustApp {
    fn render_workspace_search(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let workspace = self.active_workspace()?;
        if workspace.view_mode != WorkspaceViewMode::Terminal {
            return None;
        }
        if !workspace.search_visible {
            return None;
        }
        let matches = workspace.search_results.len();
        let current_match = workspace
            .active_search_index
            .map(|index| index + 1)
            .unwrap_or(0);

        Some(
            h_flex()
                .h(px(WORKSPACE_SEARCH_ROW_HEIGHT))
                .w_full()
                .px_4()
                .gap_3()
                .items_center()
                .bg(theme::terminal_bg())
                .border_b_1()
                .border_color(theme::border_dark())
                .child(Input::new(&self.shell_inputs.terminal_search).flex_1())
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted_dark())
                        .child(format!("{current_match}/{matches}")),
                )
                .child(
                    Button::new("workspace-search-prev")
                        .ghost()
                        .small()
                        .label("Prev")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(-1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-next")
                        .ghost()
                        .small()
                        .label("Next")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_workspace_search(window, cx);
                        })),
                ),
        )
    }

    fn render_workspace_autocomplete(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.show_command_palette {
            return None;
        }
        let workspace = self.active_workspace()?;
        if workspace.view_mode != WorkspaceViewMode::Terminal || workspace.search_visible {
            return None;
        }
        let pane = self.active_pane()?;

        let current_input = pane.current_input.trim().to_string();
        if current_input.is_empty() {
            let snippets = self.pinned_snippet_quick_actions();
            if snippets.is_empty() {
                return None;
            }

            return Some(
                h_flex()
                    .h(px(WORKSPACE_AUTOCOMPLETE_HEIGHT))
                    .w_full()
                    .px_4()
                    .gap_3()
                    .items_center()
                    .bg(theme::terminal_bg())
                    .border_b_1()
                    .border_color(theme::border_dark())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted_dark())
                            .child("Pinned commands"),
                    )
                    .child(h_flex().flex_1().gap_2().overflow_x_scrollbar().children(
                        snippets.into_iter().enumerate().map(|(index, snippet)| {
                            let command = snippet.command.clone();
                            Button::new(("workspace-pinned-snippet", index))
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::AccentSoft,
                                    cx,
                                ))
                                .label(snippet.display_name())
                                .icon(IconName::BookOpen)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_snippet_command(&command, window, cx);
                                }))
                                .into_any_element()
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::with_alpha(theme::text_muted_dark(), 0.75))
                            .child("Pin snippets to keep your common commands one click away."),
                    ),
            );
        }

        let candidates = self.workspace_autocomplete_candidates();
        if candidates.is_empty() {
            return None;
        }
        let selected_index = self.selected_autocomplete_index(candidates.len());
        let selected_candidate = candidates.get(selected_index);

        Some(
            h_flex()
                .h(px(WORKSPACE_AUTOCOMPLETE_HEIGHT))
                .w_full()
                .px_4()
                .gap_3()
                .items_center()
                .bg(theme::terminal_bg())
                .border_b_1()
                .border_color(theme::border_dark())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted_dark())
                        .child(
                            match selected_candidate.and_then(|candidate| {
                                candidate.scope_label.as_ref().map(|scope| {
                                    format!(
                                        "Autocomplete for '{}' • {} • {}",
                                        current_input,
                                        candidate.source.label(),
                                        scope
                                    )
                                })
                            }) {
                                Some(label) => label,
                                None => format!(
                                    "Autocomplete for '{}' • {}",
                                    current_input,
                                    selected_candidate
                                        .map(|candidate| candidate.source.label())
                                        .unwrap_or("suggestion")
                                ),
                            },
                        ),
                )
                .child(h_flex().flex_1().gap_2().overflow_x_scrollbar().children(
                    candidates.iter().enumerate().map(|(index, candidate)| {
                        let command = candidate.command.clone();
                        let source = candidate.source;
                        let is_selected = index == selected_index;
                        Button::new(("workspace-autocomplete", index))
                            .small()
                            .custom(Self::action_button_style(
                                if is_selected {
                                    theme::ActionTone::AccentSoft
                                } else {
                                    theme::ActionTone::Neutral
                                },
                                cx,
                            ))
                            .label(command.clone())
                            .icon(match source {
                                AutocompleteSource::Path => IconName::Folder,
                                AutocompleteSource::Context => IconName::SquareTerminal,
                                AutocompleteSource::Argument => IconName::ChevronRight,
                                AutocompleteSource::History => IconName::Redo2,
                                AutocompleteSource::Snippet => IconName::BookOpen,
                                AutocompleteSource::Builtin => IconName::SquareTerminal,
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_autocomplete_candidate(&command, source, cx);
                            }))
                            .into_any_element()
                    }),
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::with_alpha(theme::text_muted_dark(), 0.75))
                        .child(format!(
                            "{}+↑/↓ select  {}+Enter apply",
                            primary_shortcut_label(),
                            primary_shortcut_label()
                        )),
                ),
        )
    }

    fn render_workspace_files_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let workspace_id = workspace.id;
        let Some(browser) = workspace.sftp.as_ref() else {
            let active_pane_is_local = self
                .active_pane()
                .is_some_and(|pane| pane.request.kind == ConnectionKind::LocalShell);
            let empty_state = if active_pane_is_local {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(24.))
                        .text_color(theme::accent()),
                    "Files view is unavailable for local shells",
                    "SFTP file browsing only applies to SSH hosts. Switch back to the terminal to keep working locally.",
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .child(
                            Button::new("workspace-files-local-back")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Back to Terminal")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_active_workspace_terminal(cx);
                                })),
                        ),
                )
            } else {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(24.))
                        .text_color(theme::accent()),
                    "Open Files for this host",
                    "Browse the active SSH host over SFTP, upload and download files, or switch back to the terminal.",
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .child(
                            Button::new("workspace-files-open")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Open Files")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_active_workspace_files(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-back")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Back to Terminal")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_active_workspace_terminal(cx);
                                })),
                        ),
                )
            };

            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(theme::terminal_bg())
                .p(px(WORKSPACE_PADDING))
                .child(empty_state);
        };
        let selected_entry = self.selected_workspace_sftp_entry(workspace.id);

        v_flex()
            .flex_1()
            .p(px(WORKSPACE_PADDING))
            .gap_3()
            .bg(theme::terminal_bg())
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::terminal_panel())
                    .border_1()
                    .border_color(theme::border_dark())
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(theme::text_muted_dark())
                                            .child("Remote Path"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_semibold()
                                            .text_color(theme::text_on_dark())
                                            .child(browser.current_path.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(self.status_badge(
                                        browser.request.address(),
                                        theme::terminal_bg(),
                                        theme::accent(),
                                    ))
                                    .when(browser.loading, |this| {
                                        this.child(self.status_badge(
                                            "Syncing",
                                            theme::terminal_bg(),
                                            theme::warning(),
                                        ))
                                    })
                                    .when_some(selected_entry.as_ref(), |this, entry| {
                                        this.child(self.status_badge(
                                            if entry.is_dir { "Folder" } else { "File" },
                                            theme::terminal_bg(),
                                            theme::success(),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(selected_entry.as_ref(), |this, entry| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted_dark())
                                .child(if entry.is_dir {
                                    format!("Selected folder: {}", entry.path)
                                } else {
                                    format!(
                                        "Selected file: {}  •  {}",
                                        entry.path,
                                        format_file_size(entry.size.unwrap_or(0))
                                    )
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_2()
                    .when(browser.entries.is_empty() && browser.loading, |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::terminal_panel())
                                .border_1()
                                .border_color(theme::border_dark())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::LoaderCircle)
                                        .size(px(24.))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme::text_muted_dark())
                                        .child("Loading remote directory..."),
                                ),
                        )
                    })
                    .when(browser.entries.is_empty() && !browser.loading, |this| {
                        this.child(
                            self.render_workspace_empty_state(
                                Icon::new(IconName::Folder)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "This directory is empty",
                                "Try a different path, upload a file, or switch back to the terminal for shell work.",
                            )
                            .w_full(),
                        )
                    })
                    .children(browser.entries.iter().enumerate().map(|(index, entry)| {
                        let click_path = entry.path.clone();
                        let open_path = entry.path.clone();
                        let is_selected =
                            browser.selected_path.as_deref() == Some(entry.path.as_str());
                        let kind = if entry.is_dir {
                            "Folder".to_string()
                        } else if entry.is_symlink {
                            "Symlink".to_string()
                        } else {
                            format_file_size(entry.size.unwrap_or(0))
                        };

                        h_flex()
                            .id(("workspace-file-entry", index))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded(px(14.))
                            .bg(if is_selected {
                                theme::with_alpha(theme::accent(), 0.18)
                            } else {
                                theme::terminal_panel()
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::with_alpha(theme::accent(), 0.45)
                            } else {
                                theme::border_dark()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::accent(), 0.12)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_workspace_file_entry(
                                    workspace_id,
                                    click_path.clone(),
                                    cx,
                                );
                            }))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Icon::new(if entry.is_dir {
                                            IconName::FolderClosed
                                        } else {
                                            IconName::File
                                        })
                                        .size(px(16.))
                                        .text_color(
                                            if entry.is_dir {
                                                theme::warning()
                                            } else {
                                                theme::text_muted_dark()
                                            },
                                        ),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(1.))
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_medium()
                                                    .text_color(theme::text_on_dark())
                                                    .child(entry.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted_dark())
                                                    .child(entry.path.clone()),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted_dark())
                                            .child(kind),
                                    )
                                    .when(entry.is_dir, |this| {
                                        this.child(
                                            Button::new(("workspace-file-open", index))
                                                .ghost()
                                                .small()
                                                .icon(IconName::ChevronRight)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_workspace_file_entry(
                                                        workspace_id,
                                                        open_path.clone(),
                                                        cx,
                                                    );
                                                    this.open_selected_workspace_file_entry(cx);
                                                })),
                                        )
                                    }),
                            )
                            .into_any_element()
                    })),
            )
    }

    pub(super) fn terminal_font_family(&self, cx: &Context<Self>) -> SharedString {
        self.saved
            .settings
            .terminal_font_family
            .as_deref()
            .filter(|family| !family.trim().is_empty())
            .map(|family| SharedString::from(family.to_string()))
            .unwrap_or_else(|| cx.theme().mono_font_family.clone())
    }

    fn render_terminal_cell_group(
        &self,
        text: String,
        style: TerminalStyle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut node = div()
            .whitespace_nowrap()
            .font_family(self.terminal_font_family(cx))
            .text_size(px(self.terminal_font_size()))
            .line_height(relative(TERMINAL_LINE_HEIGHT))
            .text_color(style.fg)
            .text_bg(style.bg)
            .child(display_terminal_text(&text));

        if style.bold {
            node = node.font_weight(FontWeight::BOLD);
        }
        if style.italic {
            node = node.italic();
        }
        if style.underline {
            node = node.underline().text_decoration_color(style.fg);
        }

        node.into_any_element()
    }

    fn render_terminal_row(
        &self,
        row_ix: usize,
        row: &TerminalRow,
        selection: Option<SelectionRange>,
        visible_matches: &[(usize, SearchMatch, bool)],
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut groups = Vec::new();
        let mut pending_text = String::new();
        let mut pending_style: Option<TerminalStyle> = None;

        for (col_ix, cell) in row.cells.iter().enumerate() {
            let selected = selection_contains(selection, row_ix, col_ix);
            let (matched, active_match) = visible_matches.iter().fold(
                (false, false),
                |acc, (visible_row, search_match, current)| {
                    if *visible_row != row_ix {
                        return acc;
                    }
                    if (search_match.start_col..search_match.end_col).contains(&col_ix) {
                        (true, acc.1 || *current)
                    } else {
                        acc
                    }
                },
            );
            let style = style_for_render(cell, selected, matched, active_match);

            match pending_style {
                Some(current) if current == style => pending_text.push_str(&cell.text),
                Some(current) => {
                    groups.push(self.render_terminal_cell_group(
                        std::mem::take(&mut pending_text),
                        current,
                        cx,
                    ));
                    pending_text.push_str(&cell.text);
                    pending_style = Some(style);
                }
                None => {
                    pending_text.push_str(&cell.text);
                    pending_style = Some(style);
                }
            }
        }

        if let Some(style) = pending_style {
            groups.push(self.render_terminal_cell_group(pending_text, style, cx));
        } else {
            groups.push(self.render_terminal_cell_group(
                " ".to_string(),
                default_terminal_style(),
                cx,
            ));
        }

        h_flex()
            .w_full()
            .gap_0()
            .whitespace_nowrap()
            .children(groups)
            .into_any_element()
    }

    fn render_pane_drop_zones(&self, target_pane_id: u64, cx: &mut Context<Self>) -> Div {
        let active_workspace_id = self.active_workspace_id;

        div()
            .absolute()
            .top(px(PANE_HEADER_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w(relative(0.32))
                    .drag_over::<PaneDrag>(move |style, drag, _, _| {
                        if drag.pane_id == target_pane_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.14))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                        if Some(drag.workspace_id) == active_workspace_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.16))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                        this.drop_pane_on_pane(
                            drag.pane_id,
                            target_pane_id,
                            SplitAxis::Horizontal,
                            false,
                            window,
                            cx,
                        );
                    }))
                    .on_drop(
                        cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                            this.drop_workspace_on_pane(
                                drag.workspace_id,
                                target_pane_id,
                                SplitAxis::Horizontal,
                                false,
                                window,
                                cx,
                            );
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(relative(0.32))
                    .drag_over::<PaneDrag>(move |style, drag, _, _| {
                        if drag.pane_id == target_pane_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.14))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                        if Some(drag.workspace_id) == active_workspace_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.16))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                        this.drop_pane_on_pane(
                            drag.pane_id,
                            target_pane_id,
                            SplitAxis::Horizontal,
                            true,
                            window,
                            cx,
                        );
                    }))
                    .on_drop(
                        cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                            this.drop_workspace_on_pane(
                                drag.workspace_id,
                                target_pane_id,
                                SplitAxis::Horizontal,
                                true,
                                window,
                                cx,
                            );
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(relative(0.32))
                    .drag_over::<PaneDrag>(move |style, drag, _, _| {
                        if drag.pane_id == target_pane_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.14))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                        if Some(drag.workspace_id) == active_workspace_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.16))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                        this.drop_pane_on_pane(
                            drag.pane_id,
                            target_pane_id,
                            SplitAxis::Vertical,
                            false,
                            window,
                            cx,
                        );
                    }))
                    .on_drop(
                        cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                            this.drop_workspace_on_pane(
                                drag.workspace_id,
                                target_pane_id,
                                SplitAxis::Vertical,
                                false,
                                window,
                                cx,
                            );
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(relative(0.32))
                    .drag_over::<PaneDrag>(move |style, drag, _, _| {
                        if drag.pane_id == target_pane_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.14))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .drag_over::<WorkspaceTabDrag>(move |style, drag, _, _| {
                        if Some(drag.workspace_id) == active_workspace_id {
                            style
                        } else {
                            style
                                .bg(theme::with_alpha(theme::accent(), 0.16))
                                .border_2()
                                .border_color(theme::accent())
                        }
                    })
                    .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                        this.drop_pane_on_pane(
                            drag.pane_id,
                            target_pane_id,
                            SplitAxis::Vertical,
                            true,
                            window,
                            cx,
                        );
                    }))
                    .on_drop(
                        cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                            this.drop_workspace_on_pane(
                                drag.workspace_id,
                                target_pane_id,
                                SplitAxis::Vertical,
                                true,
                                window,
                                cx,
                            );
                        }),
                    ),
            )
    }

    fn render_terminal_pane(
        &self,
        pane: &SessionPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pane_id = pane.id;
        let snapshot = pane.terminal.snapshot();
        let selection = pane.selection;
        let visible_matches = self
            .active_workspace()
            .map(|workspace| self.workspace_visible_matches(workspace, pane))
            .unwrap_or_default();

        let is_active_pane = self.active_workspace().map(|w| w.active_pane_id) == Some(pane.id);
        let workspace_expanded_pane_id = self.active_workspace().and_then(|w| w.expanded_pane_id);
        let workspace_has_multiple_panes = self
            .active_workspace()
            .map(|workspace| workspace.pane_ids.len() > 1)
            .unwrap_or(false);
        let workspace_broadcasting = self
            .active_workspace()
            .map(|workspace| workspace.broadcast_input && workspace.pane_ids.len() > 1)
            .unwrap_or(false);
        let host_color_tag = self
            .saved
            .profiles
            .iter()
            .find(|profile| {
                profile.host == pane.request.host
                    && profile.port == pane.request.port
                    && profile.username == pane.request.username
            })
            .and_then(|profile| profile.color_tag);
        let status_color = if pane.connected {
            match host_color_tag {
                Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
                None => theme::success(),
            }
        } else if pane.closed && pane.status == "Error" {
            theme::danger()
        } else if pane.closed {
            theme::text_muted_dark()
        } else {
            theme::warning()
        };
        let drag_info = PaneDrag {
            pane_id,
            title: pane.title.clone(),
        };

        v_flex()
            .id(("terminal-pane", pane.id))
            .relative()
            .flex_1()
            .rounded(px(10.))
            .border_1()
            .border_color(if is_active_pane {
                theme::focus_ring()
            } else {
                theme::with_alpha(theme::accent(), 0.35)
            })
            .when(is_active_pane, |this| {
                this.shadow(vec![gpui::BoxShadow {
                    color: theme::pane_focus_glow(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(1.),
                }])
            })
            .bg(theme::terminal_panel())
            .overflow_hidden()
            .child(
                h_flex()
                    .id(("pane-header", pane.id))
                    .h(px(PANE_HEADER_HEIGHT))
                    .px(px(12.))
                    .items_center()
                    .justify_between()
                    .bg(theme::chrome_bg())
                    .border_b_1()
                    .border_color(theme::with_alpha(theme::border_dark(), 0.6))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                h_flex()
                                    .id(("pane-drag-grip", pane.id))
                                    .w(px(18.))
                                    .h(px(22.))
                                    .gap(px(2.))
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.))
                                    .cursor_grab()
                                    .hover(|style| {
                                        style.bg(theme::with_alpha(theme::text_muted_dark(), 0.12))
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    })
                                    .on_drag(drag_info, |drag: &PaneDrag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| PaneDragPreview {
                                            title: drag.title.clone(),
                                        })
                                    })
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .w(px(10.))
                                                    .h(px(1.5))
                                                    .rounded(px(999.))
                                                    .bg(theme::text_muted_dark()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(10.))
                                                    .h(px(1.5))
                                                    .rounded(px(999.))
                                                    .bg(theme::text_muted_dark()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(10.))
                                                    .h(px(1.5))
                                                    .rounded(px(999.))
                                                    .bg(theme::text_muted_dark()),
                                            ),
                                    ),
                            )
                            .child(div().size(px(9.)).rounded(px(999.)).bg(status_color))
                            .when(self.pane_rename_id == Some(pane.id), |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(180.))
                                                .child(Input::new(&self.pane_rename_input).small()),
                                        )
                                        .child(
                                            Button::new(("pane-rename-save", pane.id))
                                                .xsmall()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Accent,
                                                    cx,
                                                ))
                                                .label("Save")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.commit_pane_rename(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new(("pane-rename-cancel", pane.id))
                                                .xsmall()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.cancel_pane_rename(window, cx);
                                                })),
                                        ),
                                )
                            })
                            .when(self.pane_rename_id != Some(pane.id), |this| {
                                this.child(
                                    div()
                                        .id(("pane-title", pane.id))
                                        .text_size(px(14.))
                                        .font_medium()
                                        .text_color(theme::text_on_dark())
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.text_color(theme::with_alpha(
                                                theme::accent(),
                                                0.95,
                                            ))
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.start_pane_rename(pane_id, window, cx);
                                        }))
                                        .child(pane.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_muted_dark())
                                        .child(pane.endpoint.clone()),
                                )
                                .when(
                                    workspace_broadcasting,
                                    |this| {
                                        this.child(self.status_badge(
                                            "Broadcasting",
                                            theme::with_alpha(theme::warning(), 0.18),
                                            theme::warning(),
                                        ))
                                    },
                                )
                            }),
                    )
                    .when(workspace_has_multiple_panes, |this| {
                        this.child(
                            Button::new(("expand-pane", pane.id))
                                .ghost()
                                .xsmall()
                                .icon(if workspace_expanded_pane_id == Some(pane_id) {
                                    IconName::LayoutDashboard
                                } else {
                                    IconName::Maximize
                                })
                                .tooltip(if workspace_expanded_pane_id == Some(pane_id) {
                                    "Show split view"
                                } else {
                                    "Expand terminal"
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this
                                        .active_workspace()
                                        .and_then(|workspace| workspace.expanded_pane_id)
                                        == Some(pane_id)
                                    {
                                        this.restore_workspace_split_view(window, cx);
                                    } else {
                                        this.expand_pane(pane_id, window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(("close-pane", pane.id))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Close terminal")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_pane(pane_id, cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .id(("terminal-surface", pane.id))
                    .size_full()
                    .track_focus(&pane.terminal_focus)
                    .focusable()
                    .bg(theme::terminal_bg())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_pane(pane_id, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.handle_pane_mouse_down(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            this.handle_pane_mouse_move(pane_id, event, window, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, window, cx| {
                            this.handle_pane_scroll(pane_id, event, window, cx);
                        },
                    ))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if this.handle_terminal_key(pane_id, event, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        v_flex()
                            .size_full()
                            .overflow_hidden()
                            .px(px(TERMINAL_INNER_PADDING_X))
                            .pt(px(TERMINAL_INNER_PADDING_Y))
                            .pb(px(TERMINAL_INNER_PADDING_Y))
                            .children(snapshot.rows.iter().enumerate().map(|(row_ix, row)| {
                                self.render_terminal_row(
                                    row_ix,
                                    row,
                                    selection,
                                    &visible_matches,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(self.render_pane_drop_zones(pane_id, cx))
    }

    fn render_workspace_layout(
        &self,
        layout: &WorkspacePaneLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            WorkspacePaneLayout::Leaf(pane_id) => self
                .pane(*pane_id)
                .map(|pane| {
                    self.render_terminal_pane(pane, window, cx)
                        .into_any_element()
                })
                .unwrap_or_else(|| div().flex_1().into_any_element()),
            WorkspacePaneLayout::Split {
                axis,
                first,
                second,
            } => {
                let gap = px(PANE_GAP);
                match axis {
                    SplitAxis::Horizontal => h_flex()
                        .flex_1()
                        .gap(gap)
                        .child(self.render_workspace_layout(first, window, cx))
                        .child(self.render_workspace_layout(second, window, cx))
                        .into_any_element(),
                    SplitAxis::Vertical => v_flex()
                        .flex_1()
                        .gap(gap)
                        .child(self.render_workspace_layout(first, window, cx))
                        .child(self.render_workspace_layout(second, window, cx))
                        .into_any_element(),
                }
            }
        }
    }

    fn render_expanded_pane_list(
        &self,
        workspace: &WorkspaceTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        v_flex()
            .w(px(EXPANDED_PANE_LIST_WIDTH))
            .h_full()
            .min_h_0()
            .rounded(px(10.))
            .bg(theme::chrome_bg())
            .border_1()
            .border_color(theme::with_alpha(theme::border_dark(), 0.45))
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(44.))
                    .px(px(16.))
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::with_alpha(theme::border_dark(), 0.45))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_on_dark())
                                    .child("Terminals"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted_dark())
                                    .child(format!("• {}", workspace.pane_ids.len())),
                            ),
                    )
                    .child(
                        Button::new("expanded-restore-splits")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::PanelRight)
                            .label("Split View")
                            .tooltip("Show split view")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore_workspace_split_view(window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p(px(10.))
                    .gap(px(6.))
                    .overflow_y_scrollbar()
                    .children(workspace.pane_ids.iter().filter_map(|pane_id| {
                        let pane = self.pane(*pane_id)?;
                        let item_pane_id = *pane_id;
                        let active = workspace.expanded_pane_id == Some(item_pane_id)
                            || workspace.active_pane_id == item_pane_id;
                        let status_color = if pane.connected {
                            theme::success()
                        } else if pane.closed && pane.status == "Error" {
                            theme::danger()
                        } else if pane.closed {
                            theme::text_muted_dark()
                        } else {
                            theme::warning()
                        };

                        Some(
                            h_flex()
                                .id(("expanded-pane-list-item", item_pane_id))
                                .w_full()
                                .min_h(px(52.))
                                .px(px(10.))
                                .py(px(8.))
                                .gap(px(10.))
                                .items_start()
                                .rounded(px(8.))
                                .cursor_pointer()
                                .bg(if active {
                                    theme::with_alpha(theme::accent(), 0.12)
                                } else {
                                    gpui::transparent_black()
                                })
                                .hover(|style| {
                                    style.bg(theme::with_alpha(theme::text_muted_dark(), 0.09))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.expand_pane(item_pane_id, window, cx);
                                }))
                                .child(
                                    Icon::new(IconName::SquareTerminal)
                                        .size(px(14.))
                                        .text_color(if active {
                                            theme::accent()
                                        } else {
                                            theme::text_muted_dark()
                                        }),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(14.))
                                                .text_color(if active {
                                                    theme::accent()
                                                } else {
                                                    theme::text_on_dark()
                                                })
                                                .child(pane.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.))
                                                .text_color(theme::text_muted_dark())
                                                .child(pane.endpoint.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .mt(px(5.))
                                        .size(px(5.))
                                        .rounded(px(999.))
                                        .bg(status_color),
                                )
                                .into_any_element(),
                        )
                    })),
            )
    }

    fn render_expanded_workspace_body(
        &self,
        workspace: &WorkspaceTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let focused_pane_id = workspace
            .expanded_pane_id
            .filter(|pane_id| workspace.pane_ids.contains(pane_id))
            .unwrap_or(workspace.active_pane_id);
        let content = self
            .pane(focused_pane_id)
            .map(|pane| {
                self.render_terminal_pane(pane, window, cx)
                    .into_any_element()
            })
            .unwrap_or_else(|| div().flex_1().into_any_element());

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .p(px(WORKSPACE_PADDING))
            .gap(px(PANE_GAP))
            .bg(theme::terminal_bg())
            .child(self.render_expanded_pane_list(workspace, window, cx))
            .child(content)
    }

    fn render_workspace_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .items_center()
                .justify_center()
                .p(px(WORKSPACE_PADDING))
                .child(
                    self.render_workspace_empty_state(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "Open a host to start a workspace",
                        "Select a saved host from the library, use quick connect, or open a local terminal to start working.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("workspace-empty-local")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label("Local Terminal")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_local_terminal(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("workspace-empty-hosts")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("New Host")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    ),
                );
        };

        if let Some(failure) = workspace.connect_failure.clone() {
            let wid = workspace.id;
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .child(self.render_connect_failure_dialog(wid, &failure, cx));
        }
        if let Some(pending) = workspace.pending_connect.clone() {
            let mode = workspace.pending_connect_mode;
            let protocol = workspace.pending_connect_protocol;
            let wid = workspace.id;
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .child(match mode {
                    ConnectDialogMode::Username => self
                        .render_connect_dialog(wid, &pending, cx)
                        .into_any_element(),
                    ConnectDialogMode::ChooseProtocol => self
                        .render_choose_protocol_dialog(wid, &pending, protocol, cx)
                        .into_any_element(),
                });
        }

        if workspace.expanded_pane_id.is_some() && workspace.pane_ids.len() > 1 {
            return self.render_expanded_workspace_body(workspace, window, cx);
        }

        let content = self.render_workspace_layout(&workspace.layout, window, cx);

        v_flex()
            .flex_1()
            .p(px(WORKSPACE_PADDING))
            .bg(theme::terminal_bg())
            .child(content)
    }

    pub(super) fn render_workspace_shell(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let content = if self
            .active_workspace()
            .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Files)
        {
            self.render_workspace_files_view(window, cx)
        } else {
            self.render_workspace_body(window, cx)
        };
        let body = if self.terminal_panel_visible {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(div().flex_1().min_w_0().child(content))
                .child(self.render_terminal_side_panel(cx))
                .into_any_element()
        } else {
            content.into_any_element()
        };

        v_flex()
            .flex_1()
            .bg(theme::terminal_bg())
            .when_some(self.render_snippet_prompts_panel(cx), |this, panel| {
                this.child(panel)
            })
            .when_some(self.render_paste_confirmation(cx), |this, banner| {
                this.child(banner)
            })
            .when_some(self.render_workspace_search(window, cx), |this, search| {
                this.child(search)
            })
            .when_some(
                self.render_workspace_autocomplete(window, cx),
                |this, autocomplete| this.child(autocomplete),
            )
            .child(body)
    }
}
