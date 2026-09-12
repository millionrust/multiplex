//! Independently invalidated terminal row rendering.

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div, px, relative,
};
use gpui_component::{h_flex, v_flex};

use crate::terminal::{TerminalSnapshot, TerminalStyle};
use crate::ui::app::{SearchMatch, TERMINAL_LINE_HEIGHT};
use crate::ui::render_terminal::{
    SelectionRange, default_terminal_style, display_terminal_text, selection_contains,
    style_for_render,
};

pub(super) struct TerminalGridView {
    snapshot: TerminalSnapshot,
    selection: Option<SelectionRange>,
    visible_matches: Vec<(usize, SearchMatch, bool)>,
    font_family: SharedString,
    font_size: f32,
    #[cfg(test)]
    render_count: u64,
}

impl TerminalGridView {
    pub(super) fn new(
        snapshot: TerminalSnapshot,
        selection: Option<SelectionRange>,
        visible_matches: Vec<(usize, SearchMatch, bool)>,
        font_family: SharedString,
        font_size: f32,
    ) -> Self {
        Self {
            snapshot,
            selection,
            visible_matches,
            font_family,
            font_size,
            #[cfg(test)]
            render_count: 0,
        }
    }

    pub(super) fn replace(
        &mut self,
        snapshot: TerminalSnapshot,
        selection: Option<SelectionRange>,
        visible_matches: Vec<(usize, SearchMatch, bool)>,
        font_family: SharedString,
        font_size: f32,
    ) {
        self.snapshot = snapshot;
        self.selection = selection;
        self.visible_matches = visible_matches;
        self.font_family = font_family;
        self.font_size = font_size;
    }

    #[cfg(test)]
    pub(super) fn render_count(&self) -> u64 {
        self.render_count
    }

    fn render_cell_group(&self, text: String, style: TerminalStyle) -> AnyElement {
        let mut node = div()
            .whitespace_nowrap()
            .font_family(self.font_family.clone())
            .text_size(px(self.font_size))
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

    fn render_row(&self, row_ix: usize) -> AnyElement {
        let Some(row) = self.snapshot.rows.get(row_ix) else {
            return h_flex().w_full().into_any_element();
        };
        let mut groups = Vec::new();
        let mut pending_text = String::new();
        let mut pending_style: Option<TerminalStyle> = None;

        for (col_ix, cell) in row.cells.iter().enumerate() {
            let selected = selection_contains(self.selection, row_ix, col_ix);
            let (matched, active_match) = self.visible_matches.iter().fold(
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
                    groups.push(self.render_cell_group(std::mem::take(&mut pending_text), current));
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
            groups.push(self.render_cell_group(pending_text, style));
        } else {
            groups.push(self.render_cell_group(" ".to_string(), default_terminal_style()));
        }

        h_flex()
            .w_full()
            .gap_0()
            .whitespace_nowrap()
            .children(groups)
            .into_any_element()
    }
}

impl Render for TerminalGridView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        v_flex()
            .size_full()
            .overflow_hidden()
            .children((0..self.snapshot.rows.len()).map(|row| self.render_row(row)))
    }
}
