//! The terminal grid, painted directly onto the window.
//!
//! Each pane's grid is its own entity, so output invalidates only that pane. Rendering
//! follows the approach Zed's terminal takes: the visible cells are grouped into runs of
//! identical style and background rectangles are merged along each row, then every run
//! is shaped with its glyph advance forced to the cell width and painted at its exact
//! cell position. No layout nodes are built per row or per run.

use std::sync::Arc;

use gpui::{
    App, Bounds, Element, ElementId, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Render, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, font, outline, point, px,
    relative, size,
};

use crate::terminal::{TerminalCell, TerminalCursorShape, TerminalSnapshot, TerminalStyle};
use crate::ui::app::{SearchMatch, TERMINAL_LINE_HEIGHT};
use crate::ui::render_terminal::{SelectionRange, selection_contains, style_for_render};
use crate::ui::theme;

pub(super) struct TerminalGridView {
    snapshot: Arc<TerminalSnapshot>,
    selection: Option<SelectionRange>,
    visible_matches: Vec<(usize, SearchMatch, bool)>,
    font_family: SharedString,
    font_size: f32,
    #[cfg(test)]
    render_count: u64,
}

impl TerminalGridView {
    pub(super) fn new(
        snapshot: Arc<TerminalSnapshot>,
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
        snapshot: Arc<TerminalSnapshot>,
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
}

impl Render for TerminalGridView {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.render_count = self.render_count.saturating_add(1);
        }
        TerminalGridElement {
            snapshot: self.snapshot.clone(),
            selection: self.selection,
            visible_matches: self.visible_matches.clone(),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
        }
    }
}

struct TerminalGridElement {
    snapshot: Arc<TerminalSnapshot>,
    selection: Option<SelectionRange>,
    visible_matches: Vec<(usize, SearchMatch, bool)>,
    font_family: SharedString,
    font_size: f32,
}

/// A run of cells on one row that share a text style and sit in consecutive columns.
#[derive(Debug, PartialEq)]
struct BatchedRun {
    row: usize,
    column: usize,
    text: String,
    style: TerminalStyle,
}

/// Consecutive cells on one row with the same non-default background.
#[derive(Debug, PartialEq)]
struct BackgroundRect {
    row: usize,
    column: usize,
    cells: usize,
    color: Hsla,
}

#[derive(Debug, Default, PartialEq)]
struct GridLayout {
    runs: Vec<BatchedRun>,
    backgrounds: Vec<BackgroundRect>,
}

pub(super) struct GridPrepaint {
    line_height: Pixels,
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<(gpui::Point<Pixels>, ShapedLine)>,
    cursor: Option<(Bounds<Pixels>, TerminalCursorShape, Hsla)>,
}

impl TerminalGridElement {
    /// The cell's style after selection, search matches, and a block cursor recolor it.
    fn cell_style(&self, row: usize, index: usize, cell: &TerminalCell) -> TerminalStyle {
        let selected = selection_contains(self.selection, row, usize::from(cell.column));
        let (matched, active_match) = self.visible_matches.iter().fold(
            (false, false),
            |acc, (visible_row, search_match, current)| {
                if *visible_row == row
                    && (search_match.start_col..search_match.end_col).contains(&index)
                {
                    (true, acc.1 || *current)
                } else {
                    acc
                }
            },
        );
        let mut style = style_for_render(cell, selected, matched, active_match);
        if let Some(cursor) = self.snapshot.cursor
            && cursor.shape == TerminalCursorShape::Block
            && usize::from(cursor.row) == row
            && cursor.column == cell.column
        {
            style.fg = theme::terminal_default_bg();
            style.bg = theme::terminal_cursor();
        }
        style
    }

    fn layout_grid(&self) -> GridLayout {
        let default_bg = theme::terminal_default_bg();
        let mut layout = GridLayout::default();
        for (row_index, row) in self.snapshot.rows.iter().enumerate() {
            let mut run: Option<BatchedRun> = None;
            let mut next_column = 0_usize;
            for (index, cell) in row.cells.iter().enumerate() {
                let column = usize::from(cell.column);
                let width = if cell.wide { 2 } else { 1 };
                let style = self.cell_style(row_index, index, cell);

                if style.bg != default_bg {
                    match layout.backgrounds.last_mut() {
                        Some(rect)
                            if rect.row == row_index
                                && rect.color == style.bg
                                && rect.column + rect.cells == column =>
                        {
                            rect.cells += width;
                        }
                        _ => layout.backgrounds.push(BackgroundRect {
                            row: row_index,
                            column,
                            cells: width,
                            color: style.bg,
                        }),
                    }
                }

                if cell.is_blank() && !style.underline && !style.strikethrough {
                    if let Some(finished) = run.take() {
                        layout.runs.push(finished);
                    }
                    next_column = column + width;
                    continue;
                }

                // Shaping forces every glyph to one cell of advance, so a run continues
                // only through narrow characters and a wide character always ends it.
                let continues = run.as_ref().is_some_and(|run| {
                    same_text_style(&run.style, &style) && next_column == column
                });
                if !continues {
                    if let Some(finished) = run.take() {
                        layout.runs.push(finished);
                    }
                    run = Some(BatchedRun {
                        row: row_index,
                        column,
                        text: String::new(),
                        style,
                    });
                }
                if let Some(run) = run.as_mut() {
                    cell.push_text(&mut run.text);
                }
                next_column = column + width;
                if cell.wide
                    && let Some(finished) = run.take()
                {
                    layout.runs.push(finished);
                }
            }
            if let Some(finished) = run.take() {
                layout.runs.push(finished);
            }
        }
        layout
    }

    fn text_run(&self, style: &TerminalStyle, len: usize) -> TextRun {
        let mut run_font = font(self.font_family.clone());
        if style.bold {
            run_font.weight = FontWeight::BOLD;
        }
        if style.italic {
            run_font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font: run_font,
            color: style.fg,
            background_color: None,
            underline: style.underline.then_some(UnderlineStyle {
                thickness: px(theme::BORDER_HAIRLINE),
                color: Some(style.fg),
                wavy: false,
            }),
            strikethrough: style.strikethrough.then_some(StrikethroughStyle {
                thickness: px(theme::BORDER_HAIRLINE),
                color: Some(style.fg),
            }),
        }
    }
}

/// Everything but the background, which is painted separately.
fn same_text_style(left: &TerminalStyle, right: &TerminalStyle) -> bool {
    left.fg == right.fg
        && left.bold == right.bold
        && left.italic == right.italic
        && left.underline == right.underline
        && left.strikethrough == right.strikethrough
}

impl IntoElement for TerminalGridElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalGridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = px(self.font_size);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font(self.font_family.clone()));
        // The same metrics the app uses to size the PTY and map mouse positions to cells.
        let cell_width = px(text_system
            .ch_advance(font_id, font_size)
            .map(|width| {
                let width: f32 = width.into();
                width.max(1.0)
            })
            .unwrap_or(8.0));
        let line_height = px((self.font_size * TERMINAL_LINE_HEIGHT).max(1.));
        let origin = bounds.origin;
        let cell_origin = |row: usize, column: usize| {
            point(
                origin.x + cell_width * column as f32,
                origin.y + line_height * row as f32,
            )
        };

        let layout = self.layout_grid();
        let backgrounds = layout
            .backgrounds
            .iter()
            .map(|rect| {
                let position = cell_origin(rect.row, rect.column);
                (
                    Bounds::new(
                        point(position.x.floor(), position.y),
                        size((cell_width * rect.cells as f32).ceil(), line_height),
                    ),
                    rect.color,
                )
            })
            .collect();
        let lines = layout
            .runs
            .into_iter()
            .map(|run| {
                let text_run = self.text_run(&run.style, run.text.len());
                let shaped = text_system.shape_line(
                    run.text.into(),
                    font_size,
                    std::slice::from_ref(&text_run),
                    Some(cell_width),
                );
                (cell_origin(run.row, run.column), shaped)
            })
            .collect();
        let cursor = self.snapshot.cursor.and_then(|cursor| {
            let row = usize::from(cursor.row);
            if row >= self.snapshot.rows.len() || cursor.column >= self.snapshot.columns {
                return None;
            }
            let width = if cursor.wide {
                cell_width + cell_width
            } else {
                cell_width
            };
            let position = cell_origin(row, usize::from(cursor.column));
            let stroke = px(theme::BORDER_HAIRLINE + theme::BORDER_HAIRLINE);
            let bounds = match cursor.shape {
                // Painted into the cell's own colors during layout.
                TerminalCursorShape::Block => return None,
                TerminalCursorShape::Beam => Bounds::new(position, size(stroke, line_height)),
                TerminalCursorShape::Underline => Bounds::new(
                    point(position.x, position.y + line_height - stroke),
                    size(width, stroke),
                ),
                TerminalCursorShape::HollowBlock => Bounds::new(position, size(width, line_height)),
            };
            Some((bounds, cursor.shape, theme::terminal_cursor()))
        });

        GridPrepaint {
            line_height,
            backgrounds,
            lines,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (rect, color) in &prepaint.backgrounds {
                window.paint_quad(fill(*rect, *color));
            }
            for (origin, line) in &prepaint.lines {
                let _ = line.paint(*origin, prepaint.line_height, window, cx);
            }
            if let Some((rect, shape, color)) = prepaint.cursor {
                if shape == TerminalCursorShape::HollowBlock {
                    window.paint_quad(outline(rect, color, gpui::BorderStyle::Solid));
                } else {
                    window.paint_quad(fill(rect, color));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{TerminalSize, TerminalState};
    use crate::ui::keys::TerminalCellPos;

    fn element(bytes: &[u8]) -> TerminalGridElement {
        let mut terminal = TerminalState::new(TerminalSize::new(12, 2, 0, 0), 0);
        terminal.process_bytes(bytes);
        TerminalGridElement {
            snapshot: terminal.snapshot(),
            selection: None,
            visible_matches: Vec::new(),
            font_family: "Menlo".into(),
            font_size: 13.0,
        }
    }

    #[test]
    fn same_style_cells_batch_into_one_run_and_blanks_split_runs() {
        let layout = element(b"ab \x1b[31mcd\x1b[0m").layout_grid();
        let runs = layout
            .runs
            .iter()
            .map(|run| (run.row, run.column, run.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(runs, [(0, 0, "ab"), (0, 3, "cd")]);
        // Only the block cursor after "cd" has a background.
        assert_eq!(
            layout
                .backgrounds
                .iter()
                .map(|rect| (rect.row, rect.column, rect.cells))
                .collect::<Vec<_>>(),
            [(0, 5, 1)]
        );
    }

    #[test]
    fn wide_characters_end_their_run_and_backgrounds_merge_across_cells() {
        let layout = element("\x1b[44ma界b\x1b[0m".as_bytes()).layout_grid();
        let runs = layout
            .runs
            .iter()
            .map(|run| (run.column, run.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(runs, [(0, "a界"), (3, "b")]);
        assert_eq!(
            (layout.backgrounds[0].column, layout.backgrounds[0].cells),
            (0, 4),
            "one rectangle covers a, the wide character, and b"
        );
    }

    #[test]
    fn selection_recolors_cells_by_grid_column() {
        let mut grid = element("界ab".as_bytes());
        grid.selection = Some(SelectionRange {
            anchor: TerminalCellPos { row: 0, col: 2 },
            head: TerminalCellPos { row: 0, col: 3 },
        });
        let layout = grid.layout_grid();
        let selected = layout
            .backgrounds
            .iter()
            .find(|rect| rect.color == theme::terminal_selection_bg())
            .expect("selected cells have a background");
        assert_eq!((selected.column, selected.cells), (2, 2));
    }
}
