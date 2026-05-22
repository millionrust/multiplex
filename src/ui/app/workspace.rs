//! Workspace shell rendering: search bar, autocomplete bar, files (SFTP)
//! view, terminal pane (cells/rows), workspace body and shell wrapper.
//! All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, CursorStyle, Div, DragMoveEvent, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, ScrollWheelEvent, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::ConnectionKind;
use crate::terminal::{TerminalRow, TerminalStyle};
use crate::ui::app::{
    ConnectDialogMode, DropZone, PANE_GAP, SearchMatch, SessionPane, SplitAxis,
    TERMINAL_INNER_PADDING_X, TERMINAL_INNER_PADDING_Y, TERMINAL_LINE_HEIGHT, TermiRustApp,
    WORKSPACE_AUTOCOMPLETE_HEIGHT, WORKSPACE_PADDING, WORKSPACE_SEARCH_ROW_HEIGHT,
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
        let drop_zone = self
            .split_drop_target
            .and_then(|(pid, zone)| (pid == pane.id).then_some(zone));

        v_flex()
            .id(("terminal-pane", pane.id))
            .relative()
            .size_full()
            .bg(theme::terminal_panel())
            .overflow_hidden()
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<WorkspaceTabDrag>, _, cx| {
                    this.update_split_drop_target(pane_id, event, cx);
                },
            ))
            .on_drop(
                cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                    this.drop_tab_on_pane(drag.workspace_id, pane_id, window, cx);
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
            .when_some(drop_zone, |this, zone| {
                this.child(
                    div()
                        .absolute()
                        .bg(theme::with_alpha(theme::accent(), 0.22))
                        .border_2()
                        .border_color(theme::accent())
                        .map(|d| match zone {
                            DropZone::Left => d.left(px(0.)).top(px(0.)).h_full().w(relative(0.5)),
                            DropZone::Right => {
                                d.right(px(0.)).top(px(0.)).h_full().w(relative(0.5))
                            }
                            DropZone::Top => d.top(px(0.)).left(px(0.)).w_full().h(relative(0.5)),
                            DropZone::Bottom => {
                                d.bottom(px(0.)).left(px(0.)).w_full().h(relative(0.5))
                            }
                        }),
                )
            })
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

        let workspace_id = workspace.id;
        let axis = workspace.split_axis;
        let pane_ids: Vec<u64> = workspace.pane_ids.clone();
        let layouts = self.pane_layouts(window, cx);
        let pane_count = pane_ids.len();

        let mut children: Vec<AnyElement> = Vec::new();
        for (index, pane_id) in pane_ids.iter().copied().enumerate() {
            let Some(pane) = self.pane(pane_id) else {
                continue;
            };
            let layout = layouts.iter().find(|layout| layout.pane_id == pane_id);
            let pane_el = self.render_terminal_pane(pane, window, cx);
            let sized = match axis {
                SplitAxis::Horizontal => pane_el
                    .h_full()
                    .w(px(layout.map(|layout| layout.pane_width).unwrap_or(320.0))),
                SplitAxis::Vertical => pane_el
                    .w_full()
                    .h(px(layout.map(|layout| layout.pane_height).unwrap_or(240.0))),
            };
            children.push(sized.into_any_element());
            if index + 1 < pane_count {
                children.push(
                    self.render_pane_divider(workspace_id, index, axis, cx)
                        .into_any_element(),
                );
            }
        }

        let content = match axis {
            SplitAxis::Horizontal => h_flex().size_full().children(children).into_any_element(),
            SplitAxis::Vertical => v_flex().size_full().children(children).into_any_element(),
        };

        v_flex().flex_1().bg(theme::terminal_bg()).child(content)
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
            .child(content)
    }

    fn render_pane_divider(
        &self,
        workspace_id: u64,
        index: usize,
        axis: SplitAxis,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self
            .divider_drag
            .is_some_and(|drag| drag.workspace_id == workspace_id && drag.index == index);
        let grip = if active {
            theme::accent()
        } else {
            theme::with_alpha(theme::text_muted_dark(), 0.45)
        };
        let base = div()
            .id(("pane-divider", index as u64))
            .flex()
            .items_center()
            .justify_center()
            .hover(|style| style.bg(theme::with_alpha(theme::accent(), 0.12)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.start_divider_drag(workspace_id, index, axis, event.position, window, cx);
                }),
            );
        match axis {
            SplitAxis::Horizontal => base
                .w(px(PANE_GAP))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight)
                .child(div().w(px(2.)).h(px(40.)).rounded(px(1.)).bg(grip)),
            SplitAxis::Vertical => base
                .h(px(PANE_GAP))
                .w_full()
                .cursor(CursorStyle::ResizeUpDown)
                .child(div().h(px(2.)).w(px(40.)).rounded(px(1.)).bg(grip)),
        }
    }

    fn update_split_drop_target(
        &mut self,
        pane_id: u64,
        event: &DragMoveEvent<WorkspaceTabDrag>,
        cx: &mut Context<Self>,
    ) {
        let source_workspace_id = event.drag(cx).workspace_id;
        if self.workspace_id_for_pane(pane_id) == Some(source_workspace_id) {
            if self.split_drop_target.is_some() {
                self.split_drop_target = None;
                cx.notify();
            }
            return;
        }
        let bounds = event.bounds;
        let position = event.event.position;
        let width = f32::from(bounds.size.width).max(1.0);
        let height = f32::from(bounds.size.height).max(1.0);
        let rx = (f32::from(position.x) - f32::from(bounds.origin.x)) / width - 0.5;
        let ry = (f32::from(position.y) - f32::from(bounds.origin.y)) / height - 0.5;
        let zone = if rx.abs() > ry.abs() {
            if rx < 0.0 {
                DropZone::Left
            } else {
                DropZone::Right
            }
        } else if ry < 0.0 {
            DropZone::Top
        } else {
            DropZone::Bottom
        };
        let next = Some((pane_id, zone));
        if self.split_drop_target != next {
            self.split_drop_target = next;
            cx.notify();
        }
    }

    fn drop_tab_on_pane(
        &mut self,
        source_workspace_id: u64,
        target_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone = self
            .split_drop_target
            .and_then(|(pid, zone)| (pid == target_pane_id).then_some(zone))
            .unwrap_or(DropZone::Right);
        self.merge_tab_as_split(source_workspace_id, target_pane_id, zone, window, cx);
    }
}
