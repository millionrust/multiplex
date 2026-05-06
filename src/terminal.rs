use std::mem;

use gpui::Hsla;
use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode, Parser};

use crate::ui::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            pixel_width,
            pixel_height,
        }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::new(160, 48, 0, 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalStyle {
    pub fg: Hsla,
    pub bg: Hsla,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCell {
    pub text: String,
    pub style: TerminalStyle,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalRow {
    pub cells: Vec<TerminalCell>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalSnapshot {
    pub rows: Vec<TerminalRow>,
}

pub struct TerminalState {
    parser: Parser,
    size: TerminalSize,
}

impl TerminalState {
    pub fn new(size: TerminalSize, scrollback: usize) -> Self {
        Self {
            parser: Parser::new(size.rows, size.cols, scrollback),
            size,
        }
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        self.parser.process(data);
    }

    pub fn resize(&mut self, size: TerminalSize) {
        if self.size.cols == size.cols && self.size.rows == size.rows {
            self.size = size;
            return;
        }

        self.size = size;
        self.parser.screen_mut().set_size(size.rows, size.cols);
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        self.parser.screen().mouse_protocol_mode()
    }

    pub fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        self.parser.screen().mouse_protocol_encoding()
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn max_scrollback(&self) -> usize {
        let mut screen = self.parser.screen().clone();
        screen.set_scrollback(usize::MAX);
        screen.scrollback()
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        self.parser.screen_mut().set_scrollback(rows);
    }

    pub fn reset_scrollback(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn scroll_scrollback(&mut self, delta: i32) {
        let current = self.scrollback() as i32;
        let next = (current + delta).max(0) as usize;
        self.parser.screen_mut().set_scrollback(next);
    }

    pub fn visible_row_text(&self, row: u16) -> Option<String> {
        let (_, cols) = self.parser.screen().size();
        self.parser.screen().rows(0, cols).nth(usize::from(row))
    }

    pub fn all_rows_text(&self) -> Vec<String> {
        let screen = self.parser.screen().clone();
        let (rows, cols) = screen.size();
        let viewport_rows = usize::from(rows.max(1));
        let max_scrollback = {
            let mut top = screen.clone();
            top.set_scrollback(usize::MAX);
            top.scrollback()
        };
        let total_rows = max_scrollback + viewport_rows;
        let mut all_rows = Vec::with_capacity(total_rows);

        let full_pages = max_scrollback / viewport_rows;
        let remainder = max_scrollback % viewport_rows;

        for page in 0..full_pages {
            let mut view = screen.clone();
            view.set_scrollback(max_scrollback - page * viewport_rows);
            for row in view.rows(0, cols) {
                all_rows.push(row);
            }
        }

        if remainder > 0 {
            let mut view = screen.clone();
            view.set_scrollback(remainder);
            for row in view.rows(0, cols).take(remainder) {
                all_rows.push(row);
            }
        }

        {
            let mut view = screen.clone();
            view.set_scrollback(0);
            for row in view.rows(0, cols) {
                all_rows.push(row);
            }
        }

        all_rows
    }

    pub fn visible_row_start(&self) -> usize {
        self.max_scrollback().saturating_sub(self.scrollback())
    }

    pub fn contents_between(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        self.parser
            .screen()
            .contents_between(start_row, start_col, end_row, end_col)
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let show_cursor = !screen.hide_cursor() && screen.scrollback() == 0;
        let mut rendered_rows = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut cells = Vec::with_capacity(cols as usize);
            let mut text = String::new();

            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }

                let cursor_here = show_cursor && row == cursor_row && col == cursor_col;
                let cell_text = cell_text(cell);
                text.push_str(&cell_text);
                cells.push(TerminalCell {
                    text: cell_text,
                    style: style_for_cell(cell, cursor_here),
                });
            }

            rendered_rows.push(TerminalRow { cells, text });
        }

        TerminalSnapshot {
            rows: rendered_rows,
        }
    }
}

fn cell_text(cell: &vt100::Cell) -> String {
    if cell.has_contents() {
        cell.contents().to_string()
    } else {
        " ".to_string()
    }
}

fn style_for_cell(cell: &vt100::Cell, cursor_here: bool) -> TerminalStyle {
    let mut fg = map_terminal_color(cell.fgcolor(), true);
    let mut bg = map_terminal_color(cell.bgcolor(), false);

    if cell.inverse() {
        mem::swap(&mut fg, &mut bg);
    }

    if cell.dim() {
        fg.a = (fg.a * 0.72).clamp(0.0, 1.0);
    }

    if cursor_here {
        mem::swap(&mut fg, &mut bg);
    }

    TerminalStyle {
        fg,
        bg,
        bold: cell.bold(),
        italic: cell.italic(),
        underline: cell.underline(),
    }
}

fn map_terminal_color(color: Color, foreground: bool) -> Hsla {
    match color {
        Color::Default => {
            if foreground {
                theme::terminal_default_fg()
            } else {
                theme::terminal_default_bg()
            }
        }
        Color::Rgb(r, g, b) => rgb_color(r, g, b),
        Color::Idx(index) => palette_color(index),
    }
}

fn palette_color(index: u8) -> Hsla {
    match index {
        0 => hex_color(0x000000),
        1 => hex_color(0xcd3131),
        2 => hex_color(0x0dbc79),
        3 => hex_color(0xe5e510),
        4 => hex_color(0x2472c8),
        5 => hex_color(0xbc3fbc),
        6 => hex_color(0x11a8cd),
        7 => hex_color(0xe5e5e5),
        8 => hex_color(0x666666),
        9 => hex_color(0xf14c4c),
        10 => hex_color(0x23d18b),
        11 => hex_color(0xf5f543),
        12 => hex_color(0x3b8eea),
        13 => hex_color(0xd670d6),
        14 => hex_color(0x29b8db),
        15 => hex_color(0xffffff),
        16..=231 => {
            let value = index - 16;
            let r = cube_component(value / 36);
            let g = cube_component((value % 36) / 6);
            let b = cube_component(value % 6);
            rgb_color(r, g, b)
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            rgb_color(gray, gray, gray)
        }
    }
}

fn cube_component(index: u8) -> u8 {
    match index {
        0 => 0,
        1 => 95,
        2 => 135,
        3 => 175,
        4 => 215,
        _ => 255,
    }
}

fn hex_color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn rgb_color(r: u8, g: u8, b: u8) -> Hsla {
    hex_color(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
}
