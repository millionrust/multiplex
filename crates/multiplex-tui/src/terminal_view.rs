use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use vt100::{Cell, Parser};

pub const MAX_TERMINAL_COLUMNS: u16 = 320;
pub const MAX_TERMINAL_ROWS: u16 = 120;
pub const TERMINAL_SCROLLBACK_ROWS: usize = 1_000;

pub struct TerminalView {
    parser: Parser,
    processed_bytes: u64,
}

impl std::fmt::Debug for TerminalView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalView")
            .field("size", &self.parser.screen().size())
            .field("processed_bytes", &self.processed_bytes)
            .finish_non_exhaustive()
    }
}

impl TerminalView {
    pub fn new(columns: u16, rows: u16) -> Self {
        let (columns, rows) = bounded_viewport(columns, rows);
        Self {
            parser: Parser::new(rows, columns, TERMINAL_SCROLLBACK_ROWS),
            processed_bytes: 0,
        }
    }

    pub fn reset(&mut self, columns: u16, rows: u16, bytes: &[u8]) {
        let (columns, rows) = bounded_viewport(columns, rows);
        self.parser = Parser::new(rows, columns, TERMINAL_SCROLLBACK_ROWS);
        self.process(bytes);
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.processed_bytes = self.processed_bytes.saturating_add(bytes.len() as u64);
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> (u16, u16) {
        let (columns, rows) = bounded_viewport(columns, rows);
        self.parser.screen_mut().set_size(rows, columns);
        (columns, rows)
    }

    pub const fn processed_bytes(&self) -> u64 {
        self.processed_bytes
    }

    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, no_color: bool) {
        let screen = self.parser.screen();
        let rows = area.height.min(screen.size().0);
        let columns = area.width.min(screen.size().1);
        let buffer = frame.buffer_mut();
        for row in 0..rows {
            for column in 0..columns {
                let Some(cell) = screen.cell(row, column) else {
                    continue;
                };
                let target = &mut buffer[(area.x + column, area.y + row)];
                target.set_symbol(if cell.is_wide_continuation() {
                    " "
                } else if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                });
                target.set_style(cell_style(cell, no_color));
            }
        }
        if !screen.hide_cursor() {
            let (row, column) = screen.cursor_position();
            if row < rows && column < columns {
                frame.set_cursor_position((area.x + column, area.y + row));
            }
        }
    }
}

pub fn bounded_viewport(columns: u16, rows: u16) -> (u16, u16) {
    (
        columns.clamp(1, MAX_TERMINAL_COLUMNS),
        rows.clamp(1, MAX_TERMINAL_ROWS),
    )
}

fn cell_style(cell: &Cell, no_color: bool) -> Style {
    let mut style = Style::default();
    if !no_color {
        style = style
            .fg(map_color(cell.fgcolor()))
            .bg(map_color(cell.bgcolor()));
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn map_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn parser_confines_title_clipboard_and_url_control_sequences() {
        let mut view = TerminalView::new(40, 8);
        view.process(b"safe\x1b]0;outer-title\x07\x1b]52;c;ZXZpbA==\x07\x1b]8;;https://example.test\x07link\x1b]8;;\x07");
        let contents = view.contents();
        assert!(contents.contains("safelink"));
        assert!(!contents.contains("outer-title"));
        assert!(!contents.contains("ZXZpbA"));
        assert!(!contents.contains("https://"));

        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), false))
            .unwrap();
        let rendered =
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .fold(String::new(), |mut output, cell| {
                    output.push_str(cell.symbol());
                    output
                });
        assert!(rendered.contains("safelink"));
        assert!(!rendered.contains('\x1b'));
    }

    #[test]
    fn viewport_and_scrollback_are_bounded() {
        let mut view = TerminalView::new(u16::MAX, u16::MAX);
        assert_eq!(
            view.resize(u16::MAX, u16::MAX),
            (MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS)
        );
        view.process(b"\x1b[?2004h");
        assert!(view.bracketed_paste());
    }

    #[test]
    fn eight_mibibyte_replay_exceeds_one_mibibyte_per_second() {
        let mut view = TerminalView::new(160, 48);
        let replay = vec![b'x'; 8 * 1024 * 1024];
        let started = Instant::now();
        view.process(&replay);
        let elapsed = started.elapsed();
        eprintln!("8 MiB terminal replay: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(8));
        assert_eq!(view.processed_bytes(), replay.len() as u64);
    }
}
