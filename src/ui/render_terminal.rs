//! Terminal cell selection + style helpers used by the renderer.

use std::cmp::Ordering;

use gpui::SharedString;

use crate::terminal::{TerminalCell, TerminalStyle};
use crate::ui::keys::TerminalCellPos;
use crate::ui::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub anchor: TerminalCellPos,
    pub head: TerminalCellPos,
}

pub fn selection_contains(selection: Option<SelectionRange>, row: usize, col: usize) -> bool {
    let Some(selection) = selection.and_then(normalized_selection) else {
        return false;
    };
    let current = TerminalCellPos {
        row: row as u16,
        col: col as u16,
    };

    compare_cell_pos(current, selection.anchor) != Ordering::Less
        && compare_cell_pos(current, selection.head) == Ordering::Less
}

pub fn normalized_selection(selection: SelectionRange) -> Option<SelectionRange> {
    let start = selection.anchor;
    let end_inclusive = selection.head;
    if start == end_inclusive {
        return None;
    }

    if compare_cell_pos(start, end_inclusive) == Ordering::Less {
        Some(SelectionRange {
            anchor: start,
            head: TerminalCellPos {
                row: end_inclusive.row,
                col: end_inclusive.col.saturating_add(1),
            },
        })
    } else {
        Some(SelectionRange {
            anchor: end_inclusive,
            head: TerminalCellPos {
                row: start.row,
                col: start.col.saturating_add(1),
            },
        })
    }
}

pub fn compare_cell_pos(left: TerminalCellPos, right: TerminalCellPos) -> Ordering {
    match left.row.cmp(&right.row) {
        Ordering::Equal => left.col.cmp(&right.col),
        ordering => ordering,
    }
}

pub fn style_for_render(
    cell: &TerminalCell,
    selected: bool,
    matched: bool,
    active_match: bool,
) -> TerminalStyle {
    let mut style = cell.style;
    if matched {
        style.bg = if active_match {
            theme::terminal_search_active_match_bg()
        } else {
            theme::terminal_search_match_bg()
        };
    }
    if selected {
        style.bg = theme::terminal_selection_bg();
        style.fg = theme::terminal_selection_fg();
    }
    style
}

pub fn default_terminal_style() -> TerminalStyle {
    TerminalStyle {
        fg: theme::terminal_default_fg(),
        bg: theme::terminal_default_bg(),
        bold: false,
        italic: false,
        underline: false,
    }
}

pub fn display_terminal_text(text: &str) -> SharedString {
    if text.is_empty() {
        return "\u{00a0}".into();
    }

    text.replace(' ', "\u{00a0}").into()
}
