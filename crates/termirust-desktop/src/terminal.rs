//! Terminal emulation for desktop panes, backed by `alacritty_terminal`.
//!
//! The app feeds session bytes in with [`TerminalState::process_bytes`] and draws
//! [`TerminalState::snapshot`], a copy of the visible cells with colors already resolved
//! against the theme. Replies the terminal owes the program, such as cursor position
//! reports, are collected for the caller to send back with
//! [`TerminalState::take_pty_replies`].

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell as GridCell, Flags, LineLength};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler,
};
use gpui::Hsla;

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

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

/// Which mouse events the program asked to receive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseProtocolMode {
    #[default]
    None,
    /// X10 press-only reporting. Kept for the report encoder; the emulator never enables it.
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

/// How mouse reports are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseProtocolEncoding {
    #[default]
    Default,
    Utf8,
    Sgr,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalStyle {
    pub fg: Hsla,
    pub bg: Hsla,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// One visible character. Wide characters occupy two columns and appear once.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCell {
    pub column: u16,
    pub character: char,
    /// Combining characters drawn with `character`.
    pub zero_width: Option<Box<[char]>>,
    pub wide: bool,
    pub style: TerminalStyle,
}

impl TerminalCell {
    pub fn is_blank(&self) -> bool {
        self.character == ' ' && self.zero_width.is_none()
    }

    pub fn push_text(&self, text: &mut String) {
        text.push(self.character);
        if let Some(zero_width) = &self.zero_width {
            text.extend(zero_width.iter());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalRow {
    pub cells: Vec<TerminalCell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    pub row: u16,
    pub column: u16,
    pub shape: TerminalCursorShape,
    pub wide: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalSnapshot {
    pub rows: Vec<TerminalRow>,
    pub columns: u16,
    /// Absent when the cursor is hidden or scrolled out of view.
    pub cursor: Option<TerminalCursor>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalRenderMetrics {
    pub parser_batches: u64,
    pub parser_bytes: u64,
    pub snapshot_requests: u64,
    pub snapshot_cache_hits: u64,
    pub rows_scanned: u64,
}

#[derive(Default)]
struct TerminalSnapshotCache {
    revision: u64,
    theme_key: Option<(Hsla, Hsla)>,
    initialized: bool,
    snapshot: Arc<TerminalSnapshot>,
}

/// Collects what the emulator asks of its host while bytes are processed.
#[derive(Clone, Default)]
struct TerminalEvents {
    replies: Arc<Mutex<Vec<u8>>>,
}

impl EventListener for TerminalEvents {
    fn send_event(&self, event: TermEvent) {
        let reply = match event {
            TermEvent::PtyWrite(text) => text,
            // Which colours this terminal is using. tmux asks for the foreground and background
            // when a client attaches and waits for both answers, and a program reads the
            // background to decide whether to draw itself for a light or a dark terminal, so
            // leaving these unanswered makes both behave as though they were somewhere else.
            TermEvent::ColorRequest(index, format) => format(queried_color(index)),
            _ => return,
        };
        if let Ok(mut replies) = self.replies.lock() {
            replies.extend_from_slice(reply.as_bytes());
        }
    }
}

/// Whether this terminal's background is dark, which a program asks about to pick its palette.
fn dark_background() -> bool {
    let background = gpui::Rgba::from(theme::terminal_default_bg());
    // Rec. 709 luma: the eye weighs green most and blue least.
    let luma = 0.2126 * background.r + 0.7152 * background.g + 0.0722 * background.b;
    luma < 0.5
}

/// The colour this terminal draws one palette entry with, for a program that asked.
fn queried_color(index: usize) -> Rgb {
    let color = if index == NamedColor::Foreground as usize {
        theme::terminal_default_fg()
    } else if index == NamedColor::Background as usize {
        theme::terminal_default_bg()
    } else if index == NamedColor::Cursor as usize {
        theme::terminal_cursor()
    } else if let Ok(index) = u8::try_from(index) {
        palette_color(index)
    } else {
        theme::terminal_default_fg()
    };
    let rgba = gpui::Rgba::from(color);
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: channel(rgba.r),
        g: channel(rgba.g),
        b: channel(rgba.b),
    }
}

pub struct TerminalState {
    term: Term<TerminalEvents>,
    parser: Processor<StdSyncHandler>,
    events: TerminalEvents,
    size: TerminalSize,
    revision: u64,
    /// Tail of the last read, so a name-and-version query split across reads is still seen.
    name_query_tail: Vec<u8>,
    snapshot_cache: RefCell<TerminalSnapshotCache>,
    render_metrics: Cell<TerminalRenderMetrics>,
}

impl TerminalState {
    pub fn new(size: TerminalSize, scrollback: usize) -> Self {
        let events = TerminalEvents::default();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, events.clone()),
            parser: Processor::new(),
            events,
            size,
            revision: 0,
            name_query_tail: Vec::new(),
            snapshot_cache: RefCell::new(TerminalSnapshotCache::default()),
            render_metrics: Cell::new(TerminalRenderMetrics::default()),
        }
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Answers the two questions the emulator underneath does not: which terminal this is
    /// (XTVERSION), and whether it is light or dark.
    ///
    /// tmux asks both when a client attaches, and keeps asking while they go unanswered. A
    /// terminal interface program reads the second through tmux to choose a palette, and draws
    /// itself for the wrong background without it.
    fn answer_name_and_version(&mut self, data: &[u8]) {
        const NAME: &[u8] = b"\x1b[>q";
        const SCHEME: &[u8] = b"\x1b[?996n";
        // A query split across two reads still has to be recognised, so the tail of the previous
        // read is carried over.
        self.name_query_tail.extend_from_slice(data);
        let window = std::mem::take(&mut self.name_query_tail);
        let count = |query: &[u8]| window.windows(query.len()).filter(|w| *w == query).count();
        let names = count(NAME);
        let schemes = count(SCHEME);
        let longest = NAME.len().max(SCHEME.len());
        let keep = window.len().saturating_sub(longest - 1);
        self.name_query_tail = window[keep..].to_vec();
        if names == 0 && schemes == 0 {
            return;
        }
        let name = format!("\x1bP>|TermiRust {}\x1b\\", env!("CARGO_PKG_VERSION"));
        // 1 is a dark background, 2 a light one.
        let scheme = if dark_background() {
            "\x1b[?997;1n"
        } else {
            "\x1b[?997;2n"
        };
        if let Ok(mut replies) = self.events.replies.lock() {
            for _ in 0..names {
                replies.extend_from_slice(name.as_bytes());
            }
            for _ in 0..schemes {
                replies.extend_from_slice(scheme.as_bytes());
            }
        }
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        self.answer_name_and_version(data);
        self.parser.advance(&mut self.term, data);
        self.mark_dirty();
        let mut metrics = self.render_metrics.get();
        metrics.parser_batches = metrics.parser_batches.saturating_add(1);
        metrics.parser_bytes = metrics
            .parser_bytes
            .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        self.render_metrics.set(metrics);
    }

    /// Applies a synchronized update (DEC mode 2026) whose program never ended it before
    /// the emulator's timeout. Returns whether the screen changed.
    pub fn flush_expired_synchronized_update(&mut self) -> bool {
        let expired = self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| deadline <= std::time::Instant::now());
        if expired {
            self.parser.stop_sync(&mut self.term);
            self.mark_dirty();
        }
        expired
    }

    /// Bytes the terminal owes the program, such as device attribute and cursor position
    /// reports, in the order they were requested.
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        self.events
            .replies
            .lock()
            .map(|mut replies| std::mem::take(&mut *replies))
            .unwrap_or_default()
    }

    /// Escape sequences that reproduce the visible screen and input modes on a fresh
    /// terminal of the same size, for a controller that attaches mid-session.
    pub fn controller_snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mode = *self.term.mode();
        bytes.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J\x1b[3J");
        let grid = self.term.grid();
        let columns = grid.columns();
        let mut current = SgrState::default();
        for row in 0..grid.screen_lines() {
            let line = Line(row as i32);
            bytes.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
            let length = grid[line].line_length().0.min(columns);
            for column in 0..length {
                let cell = &grid[line][Column(column)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let next = SgrState::of(cell);
                if next != current {
                    next.write(&mut bytes);
                    current = next;
                }
                let mut text = String::new();
                text.push(cell.c);
                text.extend(cell.zerowidth().into_iter().flatten());
                bytes.extend_from_slice(text.as_bytes());
            }
        }
        bytes.extend_from_slice(b"\x1b[0m");
        let cursor = grid.cursor.point;
        bytes.extend_from_slice(
            format!("\x1b[{};{}H", cursor.line.0 + 1, cursor.column.0 + 1).as_bytes(),
        );
        let private_modes = [
            (TermMode::SHOW_CURSOR, 25),
            (TermMode::APP_CURSOR, 1),
            (TermMode::BRACKETED_PASTE, 2004),
            (TermMode::MOUSE_REPORT_CLICK, 1000),
            (TermMode::MOUSE_DRAG, 1002),
            (TermMode::MOUSE_MOTION, 1003),
            (TermMode::UTF8_MOUSE, 1005),
            (TermMode::SGR_MOUSE, 1006),
        ];
        for (flag, number) in private_modes {
            let suffix = if mode.contains(flag) { 'h' } else { 'l' };
            bytes.extend_from_slice(format!("\x1b[?{number}{suffix}").as_bytes());
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            bytes.extend_from_slice(b"\x1b=");
        } else {
            bytes.extend_from_slice(b"\x1b>");
        }
        bytes
    }

    pub fn resize(&mut self, size: TerminalSize) {
        if self.size.cols == size.cols && self.size.rows == size.rows {
            self.size = size;
            return;
        }

        self.size = size;
        self.term.resize(size);
        self.mark_dirty();
    }

    fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn application_cursor(&self) -> bool {
        self.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn alternate_screen(&self) -> bool {
        self.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.mode().contains(TermMode::BRACKETED_PASTE)
    }

    #[cfg(test)]
    pub fn cursor_position(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        (
            u16::try_from(point.line.0.max(0)).unwrap_or(u16::MAX),
            u16::try_from(point.column.0).unwrap_or(u16::MAX),
        )
    }

    #[cfg(test)]
    pub fn cursor_visible(&self) -> bool {
        self.mode().contains(TermMode::SHOW_CURSOR)
    }

    pub fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        let mode = self.mode();
        if mode.contains(TermMode::MOUSE_MOTION) {
            MouseProtocolMode::AnyMotion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseProtocolMode::ButtonMotion
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseProtocolMode::PressRelease
        } else {
            MouseProtocolMode::None
        }
    }

    pub fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        let mode = self.mode();
        if mode.contains(TermMode::SGR_MOUSE) {
            MouseProtocolEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseProtocolEncoding::Utf8
        } else {
            MouseProtocolEncoding::Default
        }
    }

    /// How many rows the view is scrolled back from the live screen.
    pub fn scrollback(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// How many rows of history exist above the live screen.
    pub fn max_scrollback(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        let previous = self.scrollback();
        let target = rows.min(self.max_scrollback());
        if target == previous {
            return;
        }
        let delta =
            i32::try_from(target).unwrap_or(i32::MAX) - i32::try_from(previous).unwrap_or(i32::MAX);
        self.term.scroll_display(Scroll::Delta(delta));
        self.mark_dirty();
    }

    pub fn reset_scrollback(&mut self) {
        self.set_scrollback(0);
    }

    pub fn scroll_scrollback(&mut self, delta: i32) {
        let current = self.scrollback() as i64;
        let next = (current + i64::from(delta)).max(0) as usize;
        self.set_scrollback(next);
    }

    /// The text of a visible row, without trailing blanks.
    pub fn visible_row_text(&self, row: u16) -> Option<String> {
        if usize::from(row) >= self.term.grid().screen_lines() {
            return None;
        }
        Some(self.line_text(self.visible_line(row)))
    }

    /// Every row of history followed by the live screen, without trailing blanks.
    pub fn all_rows_text(&self) -> Vec<String> {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let bottom = grid.screen_lines() as i32;
        (top..bottom)
            .map(|line| self.line_text(Line(line)))
            .collect()
    }

    /// Index into [`Self::all_rows_text`] of the first visible row.
    pub fn visible_row_start(&self) -> usize {
        self.max_scrollback().saturating_sub(self.scrollback())
    }

    /// The text from a visible cell up to, but not including, another visible cell.
    /// Rows that wrapped join without a line break.
    pub fn contents_between(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        let columns = self.term.grid().columns();
        if columns == 0 || (start_row, start_col) >= (end_row, end_col) {
            return String::new();
        }
        let start = Point::new(
            self.visible_line(start_row),
            Column(usize::from(start_col).min(columns - 1)),
        );
        let end = if end_col == 0 {
            Point::new(self.visible_line(end_row) - 1, Column(columns - 1))
        } else {
            Point::new(
                self.visible_line(end_row),
                Column((usize::from(end_col) - 1).min(columns - 1)),
            )
        };
        if end < start {
            return String::new();
        }
        self.term.bounds_to_string(start, end)
    }

    /// The visible screen. Unchanged output returns the same shared snapshot.
    pub fn snapshot(&self) -> Arc<TerminalSnapshot> {
        let theme_key = (theme::terminal_default_fg(), theme::terminal_default_bg());
        let requires_scan = {
            let cache = self.snapshot_cache.borrow();
            !cache.initialized
                || cache.revision != self.revision
                || cache.theme_key != Some(theme_key)
        };
        let mut metrics = self.render_metrics.get();
        metrics.snapshot_requests = metrics.snapshot_requests.saturating_add(1);
        if requires_scan {
            let mut cache = self.snapshot_cache.borrow_mut();
            // Reuse the previous snapshot's buffers when nothing else still holds it.
            let rows_scanned = match Arc::get_mut(&mut cache.snapshot) {
                Some(snapshot) => refresh_snapshot(&self.term, snapshot),
                None => {
                    let mut snapshot = TerminalSnapshot::default();
                    let rows_scanned = refresh_snapshot(&self.term, &mut snapshot);
                    cache.snapshot = Arc::new(snapshot);
                    rows_scanned
                }
            };
            cache.revision = self.revision;
            cache.theme_key = Some(theme_key);
            cache.initialized = true;
            metrics.rows_scanned = metrics.rows_scanned.saturating_add(rows_scanned);
        } else {
            metrics.snapshot_cache_hits = metrics.snapshot_cache_hits.saturating_add(1);
        }
        self.render_metrics.set(metrics);
        self.snapshot_cache.borrow().snapshot.clone()
    }

    #[cfg(test)]
    pub fn render_metrics(&self) -> TerminalRenderMetrics {
        self.render_metrics.get()
    }

    #[cfg(test)]
    pub fn reset_render_metrics(&self) {
        self.render_metrics.set(TerminalRenderMetrics::default());
    }

    fn visible_line(&self, row: u16) -> Line {
        Line(i32::from(row) - self.term.grid().display_offset() as i32)
    }

    fn line_text(&self, line: Line) -> String {
        let row = &self.term.grid()[line];
        let mut text = String::new();
        for column in 0..row.line_length().0 {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            text.extend(cell.zerowidth().into_iter().flatten());
        }
        text.truncate(text.trim_end_matches(' ').len());
        text
    }

    fn mark_dirty(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Copies the visible cells, the way a renderer reads them: colors resolved, wide-character
/// spacers dropped, and the cursor recorded separately.
fn refresh_snapshot(term: &Term<TerminalEvents>, snapshot: &mut TerminalSnapshot) -> u64 {
    let grid = term.grid();
    let screen_lines = grid.screen_lines();
    let columns = grid.columns();
    let display_offset = grid.display_offset() as i32;
    snapshot.columns = u16::try_from(columns).unwrap_or(u16::MAX);
    snapshot
        .rows
        .resize_with(screen_lines, TerminalRow::default);

    for (row_index, row) in snapshot.rows.iter_mut().enumerate() {
        row.cells.clear();
        let line = Line(row_index as i32 - display_offset);
        let grid_row = &grid[line];
        for column in 0..columns {
            let cell = &grid_row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            row.cells.push(TerminalCell {
                column: column as u16,
                character: if cell.flags.contains(Flags::HIDDEN) || cell.c == '\t' {
                    ' '
                } else {
                    cell.c
                },
                zero_width: cell.zerowidth().map(Box::from),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                style: style_for_cell(term, cell),
            });
        }
    }

    let mode = term.mode();
    let cursor = grid.cursor.point;
    let cursor_row = cursor.line.0 + display_offset;
    snapshot.cursor = (mode.contains(TermMode::SHOW_CURSOR)
        && (0..screen_lines as i32).contains(&cursor_row))
    .then(|| TerminalCursor {
        row: cursor_row as u16,
        column: cursor.column.0 as u16,
        shape: match term.cursor_style().shape {
            CursorShape::Underline => TerminalCursorShape::Underline,
            CursorShape::Beam => TerminalCursorShape::Beam,
            CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
            CursorShape::Block | CursorShape::Hidden => TerminalCursorShape::Block,
        },
        wide: grid[cursor].flags.contains(Flags::WIDE_CHAR),
    });
    screen_lines as u64
}

fn style_for_cell(term: &Term<TerminalEvents>, cell: &GridCell) -> TerminalStyle {
    let mut fg = resolve_color(term, cell.fg, true);
    let mut bg = resolve_color(term, cell.bg, false);

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.flags.contains(Flags::DIM) {
        fg.a = (fg.a * 0.72).clamp(0.0, 1.0);
    }

    TerminalStyle {
        fg,
        bg,
        bold: cell.flags.contains(Flags::BOLD),
        italic: cell.flags.contains(Flags::ITALIC),
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        strikethrough: cell.flags.contains(Flags::STRIKEOUT),
    }
}

/// Resolves a cell color against colors the program set with OSC 4, 10, and 11, then the
/// theme.
fn resolve_color(term: &Term<TerminalEvents>, color: Color, foreground: bool) -> Hsla {
    match color {
        Color::Spec(Rgb { r, g, b }) => rgb_color(r, g, b),
        Color::Indexed(index) => term.colors()[usize::from(index)]
            .map(|Rgb { r, g, b }| rgb_color(r, g, b))
            .unwrap_or_else(|| palette_color(index)),
        Color::Named(name) => {
            if let Some(Rgb { r, g, b }) = term.colors()[name] {
                return rgb_color(r, g, b);
            }
            named_color(name, foreground)
        }
    }
}

fn named_color(name: NamedColor, foreground: bool) -> Hsla {
    let index = name as usize;
    if index < 16 {
        return palette_color(index as u8);
    }
    if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&index) {
        let mut color = palette_color((index - NamedColor::DimBlack as usize) as u8);
        color.a *= 0.72;
        return color;
    }
    match name {
        NamedColor::Background => theme::terminal_default_bg(),
        NamedColor::Cursor => theme::terminal_cursor(),
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
            theme::terminal_default_fg()
        }
        _ if foreground => theme::terminal_default_fg(),
        _ => theme::terminal_default_bg(),
    }
}

/// SGR attributes written by [`TerminalState::controller_snapshot_bytes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SgrState {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Default for SgrState {
    fn default() -> Self {
        Self {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

impl SgrState {
    const STYLE_FLAGS: Flags = Flags::BOLD
        .union(Flags::DIM)
        .union(Flags::ITALIC)
        .union(Flags::UNDERLINE)
        .union(Flags::INVERSE)
        .union(Flags::HIDDEN)
        .union(Flags::STRIKEOUT);

    fn of(cell: &GridCell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags & Self::STYLE_FLAGS,
        }
    }

    fn write(&self, bytes: &mut Vec<u8>) {
        let mut codes = vec!["0".to_owned()];
        for (flag, code) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::UNDERLINE, "4"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
        ] {
            if self.flags.contains(flag) {
                codes.push(code.to_owned());
            }
        }
        if let Some(code) = sgr_color(self.fg, true) {
            codes.push(code);
        }
        if let Some(code) = sgr_color(self.bg, false) {
            codes.push(code);
        }
        bytes.extend_from_slice(format!("\x1b[{}m", codes.join(";")).as_bytes());
    }
}

fn sgr_color(color: Color, foreground: bool) -> Option<String> {
    let base = if foreground { 38 } else { 48 };
    match color {
        Color::Spec(Rgb { r, g, b }) => Some(format!("{base};2;{r};{g};{b}")),
        Color::Indexed(index) => Some(format!("{base};5;{index}")),
        Color::Named(name) => {
            let index = name as usize;
            if index < 8 {
                Some(format!("{}", base - 8 + index))
            } else if index < 16 {
                Some(format!("{}", base + 52 + index - 8))
            } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                .contains(&index)
            {
                Some(format!(
                    "{}",
                    base - 8 + index - NamedColor::DimBlack as usize
                ))
            } else {
                None
            }
        }
    }
}

fn palette_color(index: u8) -> Hsla {
    match index {
        0..=15 => theme::terminal_ansi(index).unwrap_or_else(theme::terminal_default_fg),
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde::Deserialize;

    use super::*;

    #[test]
    fn named_colors_and_cursor_come_from_the_slate_terminal_tokens() {
        let tokens = crate::ui::theme::current_design_tokens();
        let expected = |value: termirust_ui_contract::ColorValue| {
            rgb_color(value.red, value.green, value.blue)
        };
        assert_eq!(palette_color(1), expected(tokens.color_terminal_ansi_red()));
        assert_eq!(
            palette_color(12),
            expected(tokens.color_terminal_ansi_bright_blue())
        );
        assert_eq!(
            named_color(NamedColor::Foreground, true),
            expected(tokens.color_terminal_fg())
        );
        assert_eq!(
            named_color(NamedColor::Background, false),
            expected(tokens.color_bg_terminal())
        );
        // The xterm cube and gray ramp stay fixed.
        assert_eq!(palette_color(16), rgb_color(0, 0, 0));
        assert_eq!(palette_color(255), rgb_color(238, 238, 238));
    }

    #[test]
    fn colors_a_program_sets_override_the_theme_palette() {
        let mut terminal = TerminalState::new(TerminalSize::new(4, 1, 0, 0), 0);
        terminal.process_bytes(b"\x1b]4;1;rgb:12/34/56\x07\x1b[31mA");
        let snapshot = terminal.snapshot();
        assert_eq!(
            snapshot.rows[0].cells[0].style.fg,
            rgb_color(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn snapshot_records_the_cursor_and_drops_wide_character_spacers() {
        let mut terminal = TerminalState::new(TerminalSize::new(6, 2, 0, 0), 10);
        terminal.process_bytes("a界b".as_bytes());
        let snapshot = terminal.snapshot();
        let cells = &snapshot.rows[0].cells;
        assert_eq!(
            cells
                .iter()
                .take(3)
                .map(|cell| (cell.column, cell.character, cell.wide))
                .collect::<Vec<_>>(),
            [(0, 'a', false), (1, '界', true), (3, 'b', false)]
        );
        assert_eq!(
            snapshot.cursor,
            Some(TerminalCursor {
                row: 0,
                column: 4,
                shape: TerminalCursorShape::Block,
                wide: false,
            })
        );
        drop(snapshot);

        terminal.process_bytes(b"\x1b[6 q\x1b[?25l");
        assert_eq!(terminal.snapshot().cursor, None, "hidden cursor");
        terminal.process_bytes(b"\x1b[?25h");
        assert_eq!(
            terminal.snapshot().cursor.map(|cursor| cursor.shape),
            Some(TerminalCursorShape::Beam)
        );
    }

    #[test]
    fn device_status_requests_queue_replies_for_the_program() {
        let mut terminal = TerminalState::new(TerminalSize::new(20, 4, 0, 0), 0);
        terminal.process_bytes(b"ab\x1b[6n");
        assert_eq!(terminal.take_pty_replies(), b"\x1b[1;3R");
        assert!(terminal.take_pty_replies().is_empty());
    }

    fn reply_to(query: &[u8]) -> String {
        let mut terminal = TerminalState::new(TerminalSize::new(80, 24, 0, 0), 0);
        terminal.process_bytes(query);
        String::from_utf8(terminal.take_pty_replies()).expect("replies are text")
    }

    /// What this terminal tells a program about itself. tmux asks all of this when a client
    /// attaches and decides which features to offer the programs inside from the answers, and a
    /// terminal interface library asks the same questions directly.
    #[test]
    fn the_terminal_answers_what_it_can_and_cannot_do() {
        // Answered, so a program knows it can draw a frame between a begin and an end.
        assert_eq!(reply_to(b"\x1b[?2026$p"), "\x1b[?2026;2$y");
        // Answered, so a paste arrives bracketed rather than as typing.
        assert_eq!(reply_to(b"\x1b[?2004$p"), "\x1b[?2004;2$y");
        // Answered, so a program learns when the window takes and loses focus.
        assert_eq!(reply_to(b"\x1b[?1004$p"), "\x1b[?1004;2$y");
        // Declined honestly: a program that asked keeps its own fallback instead of drawing
        // through a mode this terminal does not implement.
        for unsupported in [b"\x1b[?2027$p", b"\x1b[?2031$p", b"\x1b[?1016$p"] {
            assert!(
                reply_to(unsupported).ends_with(";0$y"),
                "{} should be declined, not ignored",
                String::from_utf8_lossy(unsupported)
            );
        }
        // Identified, which is how tmux recognises a terminal at all.
        assert!(reply_to(b"\x1b[c").starts_with("\x1b[?"));
        assert!(reply_to(b"\x1b[>c").starts_with("\x1b[>"));
        // Named, so tmux stops asking who this is every few seconds.
        assert!(reply_to(b"\x1b[>q").starts_with("\x1bP>|TermiRust "));
        // Light or dark, which a program picks its palette from.
        let scheme = reply_to(b"\x1b[?996n");
        assert!(
            scheme == "\x1b[?997;1n" || scheme == "\x1b[?997;2n",
            "expected a light or dark answer, got {scheme:?}"
        );
        // Asked the way tmux asks them, in one stream, every answer still comes back.
        let together = reply_to(b"\x1b[c\x1b[>c\x1b[>q\x1b[?2026$p\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        for expected in [
            "\x1b[?",
            "\x1b[>0",
            "TermiRust ",
            "?2026;2$y",
            "]10;rgb:",
            "]11;rgb:",
        ] {
            assert!(
                together.contains(expected),
                "{expected:?} missing from {together:?}"
            );
        }
    }

    /// A query that arrives in pieces, as it does from a pseudoterminal, is still answered.
    #[test]
    fn a_query_split_across_reads_is_still_answered() {
        let mut terminal = TerminalState::new(TerminalSize::new(80, 24, 0, 0), 0);
        terminal.process_bytes(b"\x1b[>");
        terminal.process_bytes(b"q");
        let reply = String::from_utf8(terminal.take_pty_replies()).expect("replies are text");
        assert!(
            reply.starts_with("\x1bP>|TermiRust "),
            "expected a name, got {reply:?}"
        );
    }

    /// Runs a real terminal interface program against this emulator through a pseudoterminal,
    /// answering its questions the way a pane does, and checks it drew what it was asked to.
    ///
    /// Ignored by default because it needs a program to drive: set `TERMIRUST_TUI_PROBE` to a
    /// command, such as `bun run /path/to/app.ts`, and run with `--ignored`.
    #[test]
    #[ignore = "requires TERMIRUST_TUI_PROBE to name a terminal interface program to run"]
    fn a_terminal_interface_program_renders_against_this_emulator() {
        use std::io::{Read as _, Write as _};

        let Ok(probe) = std::env::var("TERMIRUST_TUI_PROBE") else {
            return;
        };
        let mut words = probe.split_whitespace();
        let program = words.next().expect("a program to run");
        let size = TerminalSize::new(100, 30, 800, 480);
        let pty = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            })
            .expect("a pseudoterminal");
        let mut command = portable_pty::CommandBuilder::new(program);
        for word in words {
            command.arg(word);
        }
        // What a pane tells a program about itself.
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "TermiRust");
        command.env_remove("TMUX");
        let mut child = pty
            .slave
            .spawn_command(command)
            .expect("the program starts");
        drop(pty.slave);

        let mut writer = pty.master.take_writer().expect("a writer");
        let mut reader = pty.master.try_clone_reader().expect("a reader");
        let (output_tx, output_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 || output_tx.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut terminal = TerminalState::new(size, 2_000);
        let mut answered = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match output_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(bytes) => {
                    if std::env::var_os("TERMIRUST_TUI_PROBE_TRACE").is_some() {
                        println!("read: {}", String::from_utf8_lossy(&bytes).escape_debug());
                    }
                    terminal.process_bytes(&bytes);
                    let replies = terminal.take_pty_replies();
                    if !replies.is_empty() {
                        answered.extend_from_slice(&replies);
                        let _ = writer.write_all(&replies);
                        let _ = writer.flush();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = child.kill();

        let drawn = terminal.all_rows_text().join("\n");
        println!(
            "answered: {}",
            String::from_utf8_lossy(&answered).escape_debug()
        );
        println!("drawn:\n{drawn}");
        assert!(
            !drawn.trim().is_empty(),
            "the program drew nothing; it answered {} bytes of questions",
            answered.len()
        );
    }

    /// tmux waits for the foreground and background before it finishes attaching, and a program
    /// reads the background to choose a light or a dark palette.
    #[test]
    fn the_terminal_answers_which_colours_it_is_using() {
        let foreground = reply_to(b"\x1b]10;?\x1b\\");
        let background = reply_to(b"\x1b]11;?\x1b\\");
        assert!(
            foreground.starts_with("\x1b]10;rgb:"),
            "expected a foreground colour, got {foreground:?}"
        );
        assert!(
            background.starts_with("\x1b]11;rgb:"),
            "expected a background colour, got {background:?}"
        );
        assert_ne!(
            foreground, background,
            "text drawn in the background colour would be invisible"
        );
        // Asked together, as tmux asks, both answers come back.
        let both = reply_to(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        assert!(both.contains("\x1b]10;rgb:") && both.contains("\x1b]11;rgb:"));
    }

    #[test]
    fn scrolling_back_moves_the_view_and_the_selection_text_with_it() {
        let mut terminal = TerminalState::new(TerminalSize::new(10, 2, 0, 0), 100);
        terminal.process_bytes(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(terminal.max_scrollback(), 2);
        assert_eq!(terminal.visible_row_text(0).as_deref(), Some("three"));

        terminal.scroll_scrollback(1);
        assert_eq!(terminal.scrollback(), 1);
        assert_eq!(terminal.visible_row_start(), 1);
        assert_eq!(terminal.visible_row_text(0).as_deref(), Some("two"));
        assert_eq!(terminal.contents_between(0, 0, 1, 3), "two\nthr");

        terminal.scroll_scrollback(50);
        assert_eq!(terminal.scrollback(), 2);
        terminal.reset_scrollback();
        assert_eq!(terminal.scrollback(), 0);
        assert_eq!(terminal.all_rows_text(), ["one", "two", "three", "four"]);
    }

    #[test]
    fn controller_snapshot_reproduces_the_screen_and_modes() {
        let mut terminal = TerminalState::new(TerminalSize::new(12, 3, 0, 0), 10);
        terminal
            .process_bytes(b"\x1b[1;31mred\x1b[0m plain\r\n\x1b[?2004h\x1b[?1000h\x1b[?1006hnext");
        let mut replay = TerminalState::new(TerminalSize::new(12, 3, 0, 0), 10);
        replay.process_bytes(&terminal.controller_snapshot_bytes());

        assert_eq!(replay.all_rows_text(), ["red plain", "next", ""]);
        assert_eq!(replay.cursor_position(), terminal.cursor_position());
        assert!(replay.bracketed_paste());
        assert_eq!(
            replay.mouse_protocol_mode(),
            MouseProtocolMode::PressRelease
        );
        assert_eq!(replay.mouse_protocol_encoding(), MouseProtocolEncoding::Sgr);
        let original = terminal.snapshot();
        let replayed = replay.snapshot();
        assert_eq!(
            original.rows[0].cells[0].style,
            replayed.rows[0].cells[0].style
        );
        assert_eq!(
            original.rows[0].cells[4].style,
            replayed.rows[0].cells[4].style
        );
    }

    #[test]
    fn unchanged_terminal_snapshot_is_a_zero_scan_cache_hit() {
        let terminal = TerminalState::new(TerminalSize::new(80, 24, 0, 0), 10_000);

        let first = terminal.snapshot();
        assert_eq!(first.rows.len(), 24);
        drop(first);
        let second = terminal.snapshot();
        assert_eq!(second.rows.len(), 24);
        drop(second);
        assert_eq!(
            terminal.render_metrics(),
            TerminalRenderMetrics {
                snapshot_requests: 2,
                snapshot_cache_hits: 1,
                rows_scanned: 24,
                ..TerminalRenderMetrics::default()
            }
        );
    }

    #[test]
    fn terminal_snapshot_memory_work_is_bounded_by_the_viewport() {
        let mut terminal = TerminalState::new(TerminalSize::new(40, 8, 0, 0), 10_000);
        for line in 0..2_000 {
            terminal.process_bytes(format!("line-{line:04}\r\n").as_bytes());
        }
        assert!(terminal.max_scrollback() > 1_000);

        terminal.reset_render_metrics();
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.rows.len(), 8);
        assert!(snapshot.rows.iter().all(|row| row.cells.len() <= 40));
        drop(snapshot);

        assert_eq!(terminal.render_metrics().rows_scanned, 8);
    }

    #[test]
    #[ignore = "manual fixed-fixture performance profile"]
    fn desktop_terminal_performance_profile() {
        const RUNS: usize = 1_000;
        let mut startup_samples = Vec::with_capacity(RUNS);
        for _ in 0..RUNS {
            let started = Instant::now();
            let terminal = TerminalState::new(TerminalSize::new(120, 40, 0, 0), 10_000);
            drop(terminal.snapshot());
            startup_samples.push(started.elapsed());
        }

        let mut terminal = TerminalState::new(TerminalSize::new(120, 40, 0, 0), 10_000);
        drop(terminal.snapshot());
        terminal.reset_render_metrics();
        let mut input_samples = Vec::with_capacity(RUNS);
        for index in 0..RUNS {
            let byte = b'a' + u8::try_from(index % 26).expect("bounded fixture index");
            let started = Instant::now();
            terminal.process_bytes(&[byte]);
            drop(terminal.snapshot());
            input_samples.push(started.elapsed());
        }
        let interactive_metrics = terminal.render_metrics();
        assert_eq!(interactive_metrics.snapshot_requests, RUNS as u64);

        let line = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\r\n";
        let output_bytes = line.repeat(32_768);
        let mut output_terminal = TerminalState::new(TerminalSize::new(120, 40, 0, 0), 10_000);
        let output_started = Instant::now();
        for chunk in output_bytes.chunks(64 * 1024) {
            output_terminal.process_bytes(chunk);
            drop(output_terminal.snapshot());
        }
        let output_elapsed = output_started.elapsed();
        let output_metrics = output_terminal.render_metrics();
        let throughput_mib_s =
            output_bytes.len() as f64 / output_elapsed.as_secs_f64() / (1024.0 * 1024.0);

        let (startup_p50, startup_p95, startup_p99) = percentiles(&mut startup_samples);
        let (input_p50, input_p95, input_p99) = percentiles(&mut input_samples);
        assert!(
            startup_p99 < Duration::from_millis(20),
            "terminal component startup p99 regressed to {startup_p99:?}"
        );
        assert!(
            input_p99 < Duration::from_millis(10),
            "terminal input plus snapshot p99 regressed to {input_p99:?}"
        );
        assert!(
            throughput_mib_s >= 10.0,
            "terminal sustained-output throughput regressed to {throughput_mib_s:.2} MiB/s"
        );
        println!(
            "terminal component profile: startup p50={}us p95={}us p99={}us; input+snapshot p50={}us p95={}us p99={}us; sustained={throughput_mib_s:.2}MiB/s bytes={} batches={} rows_scanned={}",
            startup_p50.as_micros(),
            startup_p95.as_micros(),
            startup_p99.as_micros(),
            input_p50.as_micros(),
            input_p95.as_micros(),
            input_p99.as_micros(),
            output_bytes.len(),
            output_metrics.parser_batches,
            output_metrics.rows_scanned,
        );
    }

    fn percentiles(samples: &mut [Duration]) -> (Duration, Duration, Duration) {
        samples.sort_unstable();
        (
            samples[percentile_index(samples.len(), 50)],
            samples[percentile_index(samples.len(), 95)],
            samples[percentile_index(samples.len(), 99)],
        )
    }

    fn percentile_index(len: usize, percentile: usize) -> usize {
        len.saturating_mul(percentile)
            .div_ceil(100)
            .saturating_sub(1)
            .min(len.saturating_sub(1))
    }

    #[derive(Debug, Deserialize)]
    struct TerminalConformanceFixture {
        schema_version: u32,
        cases: Vec<TerminalConformanceCase>,
    }

    #[derive(Debug, Deserialize)]
    struct TerminalConformanceCase {
        name: String,
        columns: u16,
        rows: u16,
        scrollback: usize,
        chunks: Vec<Vec<u8>>,
        expected: TerminalConformanceExpected,
    }

    #[derive(Debug, Deserialize)]
    struct TerminalConformanceExpected {
        lines: Vec<String>,
        cursor_row: u16,
        cursor_column: u16,
        cursor_visible: bool,
        application_cursor: bool,
        alternate_screen: bool,
        bracketed_paste: bool,
        mouse_mode: String,
        mouse_encoding: String,
        scrollback_rows: usize,
    }

    #[derive(Debug, Deserialize)]
    struct TerminalConformanceV2Fixture {
        schema_version: u32,
        unicode_width_version: String,
        styles: Vec<TerminalConformanceV2Style>,
        cases: Vec<TerminalConformanceV2Case>,
    }

    #[derive(Debug, Deserialize)]
    struct TerminalConformanceV2Case {
        name: String,
        columns: u16,
        rows: u16,
        scrollback: usize,
        operations: Vec<TerminalConformanceV2Operation>,
        expected: TerminalConformanceV2Expected,
    }

    #[derive(Clone, Debug, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum TerminalConformanceV2Operation {
        Process { bytes: Vec<u8> },
        Resize { columns: u16, rows: u16 },
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct TerminalConformanceV2Expected {
        lines: Vec<String>,
        cells: Vec<Vec<TerminalConformanceV2Cell>>,
        cursor_row: u16,
        cursor_column: u16,
        cursor_visible: bool,
        application_cursor: bool,
        alternate_screen: bool,
        bracketed_paste: bool,
        mouse_mode: String,
        mouse_encoding: String,
        scrollback_rows: usize,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct TerminalConformanceV2Cell {
        text: String,
        width: u8,
        style: usize,
    }

    #[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
    struct TerminalConformanceV2Style {
        foreground: TerminalConformanceV2Color,
        background: TerminalConformanceV2Color,
        bold: bool,
        dim: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
    }

    #[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum TerminalConformanceV2Color {
        Default,
        Indexed { value: u8 },
        Rgb { red: u8, green: u8, blue: u8 },
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TerminalSignature {
        lines: Vec<String>,
        cursor: (u16, u16),
        cursor_visible: bool,
        application_cursor: bool,
        alternate_screen: bool,
        bracketed_paste: bool,
        mouse_mode: String,
        mouse_encoding: String,
        scrollback_rows: usize,
    }

    #[test]
    fn terminal_conformance_v1_matches_canonical_fixture() {
        let fixture: TerminalConformanceFixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/terminal/terminal-conformance-v1.json"
        ))
        .expect("terminal conformance fixture should decode");
        assert_eq!(fixture.schema_version, 1);

        for case in fixture.cases {
            let actual = terminal_signature(&case, case.chunks.iter().map(Vec::as_slice));
            let expected = TerminalSignature {
                lines: case.expected.lines.clone(),
                cursor: (case.expected.cursor_row, case.expected.cursor_column),
                cursor_visible: case.expected.cursor_visible,
                application_cursor: case.expected.application_cursor,
                alternate_screen: case.expected.alternate_screen,
                bracketed_paste: case.expected.bracketed_paste,
                mouse_mode: case.expected.mouse_mode.clone(),
                mouse_encoding: case.expected.mouse_encoding.clone(),
                scrollback_rows: case.expected.scrollback_rows,
            };
            if case.name == "private_modes_inside_alternate_screen" {
                // The fixture follows vt100, which homes the cursor on entering the
                // alternate screen. Like xterm, this emulator keeps the cursor where it was.
                assert_eq!(actual.lines, ["    alt", "", ""], "{}", case.name);
                assert_eq!(actual.cursor, (0, 7), "{}", case.name);
                assert_eq!(
                    TerminalSignature {
                        lines: expected.lines.clone(),
                        cursor: expected.cursor,
                        ..actual
                    },
                    expected,
                    "{}",
                    case.name
                );
                continue;
            }
            assert_eq!(actual, expected, "{}", case.name);
        }
    }

    #[test]
    fn terminal_conformance_v1_is_chunk_boundary_invariant() {
        let fixture: TerminalConformanceFixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/terminal/terminal-conformance-v1.json"
        ))
        .expect("terminal conformance fixture should decode");

        for case in fixture.cases {
            let bytes = case.chunks.concat();
            let expected = terminal_signature(&case, [&bytes[..]]);
            for split in 0..=bytes.len() {
                let actual = terminal_signature(&case, [&bytes[..split], &bytes[split..]]);
                assert_eq!(actual, expected, "{} split at {split}", case.name);
            }
        }
    }

    #[test]
    fn terminal_conformance_v2_matches_styles_widths_and_resize_operations() {
        let fixture = terminal_conformance_v2_fixture();
        assert_eq!(fixture.schema_version, 2);
        assert_eq!(fixture.unicode_width_version, "0.2.2");

        for case in &fixture.cases {
            let actual = terminal_conformance_v2_signature(case, &fixture.styles, None);
            match case.name.as_str() {
                // The fixture follows vt100, which cuts rows and columns off on resize.
                // Like xterm, iTerm2, and Zed, this emulator re-wraps long lines when the
                // width shrinks and moves top rows into history when the height shrinks.
                "resize_shrink_columns_repairs_wide_cell" => {
                    assert_eq!(actual.scrollback_rows, 2, "{}", case.name);
                    assert_eq!(
                        rewrapped_text(case),
                        "ab界cd",
                        "{} keeps every character",
                        case.name
                    );
                }
                "resize_shrink_rows_clamps_cursor" => {
                    assert_eq!(actual.lines, ["two", "three"], "{}", case.name);
                    assert_eq!(
                        (actual.cursor_row, actual.cursor_column),
                        (1, 5),
                        "{}",
                        case.name
                    );
                    assert_eq!(actual.scrollback_rows, 1, "{}", case.name);
                }
                _ => assert_eq!(actual, case.expected, "{} configured operations", case.name),
            }
        }
    }

    #[test]
    fn terminal_conformance_v2_process_steps_are_chunk_boundary_invariant() {
        let fixture = terminal_conformance_v2_fixture();
        for case in &fixture.cases {
            for (operation_index, operation) in case.operations.iter().enumerate() {
                let TerminalConformanceV2Operation::Process { bytes } = operation else {
                    continue;
                };
                for split in 0..=bytes.len() {
                    let actual = terminal_conformance_v2_signature(
                        case,
                        &fixture.styles,
                        Some((operation_index, split)),
                    );
                    assert_eq!(
                        actual,
                        terminal_conformance_v2_signature(case, &fixture.styles, None),
                        "{} operation {operation_index} split at {split}",
                        case.name
                    );
                }
            }
        }
    }

    fn rewrapped_text(case: &TerminalConformanceV2Case) -> String {
        let mut terminal = TerminalState::new(
            TerminalSize::new(case.columns, case.rows, 0, 0),
            case.scrollback,
        );
        for operation in &case.operations {
            match operation {
                TerminalConformanceV2Operation::Process { bytes } => terminal.process_bytes(bytes),
                TerminalConformanceV2Operation::Resize { columns, rows } => {
                    terminal.resize(TerminalSize::new(*columns, *rows, 0, 0));
                }
            }
        }
        terminal.all_rows_text().concat()
    }

    fn terminal_conformance_v2_fixture() -> TerminalConformanceV2Fixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/terminal/terminal-conformance-v2.json"
        ))
        .expect("terminal conformance v2 fixture should decode")
    }

    fn terminal_conformance_v2_signature(
        case: &TerminalConformanceV2Case,
        styles: &[TerminalConformanceV2Style],
        split: Option<(usize, usize)>,
    ) -> TerminalConformanceV2Expected {
        let mut terminal = TerminalState::new(
            TerminalSize::new(case.columns, case.rows, 0, 0),
            case.scrollback,
        );
        for (operation_index, operation) in case.operations.iter().enumerate() {
            match operation {
                TerminalConformanceV2Operation::Process { bytes } => {
                    if let Some((split_operation, split_at)) = split
                        && split_operation == operation_index
                    {
                        terminal.process_bytes(&bytes[..split_at]);
                        terminal.process_bytes(&bytes[split_at..]);
                    } else {
                        terminal.process_bytes(bytes);
                    }
                }
                TerminalConformanceV2Operation::Resize { columns, rows } => {
                    terminal.resize(TerminalSize::new(*columns, *rows, 0, 0));
                }
            }
        }

        let grid = terminal.term.grid();
        let (cursor_row, cursor_column) = terminal.cursor_position();
        TerminalConformanceV2Expected {
            lines: (0..grid.screen_lines())
                .map(|row| terminal.line_text(Line(row as i32)))
                .collect(),
            cells: (0..grid.screen_lines())
                .map(|row| {
                    (0..grid.columns())
                        .map(|column| {
                            let cell = &grid[Line(row as i32)][Column(column)];
                            let spacer = cell.flags.contains(Flags::WIDE_CHAR_SPACER);
                            TerminalConformanceV2Cell {
                                text: if spacer {
                                    String::new()
                                } else {
                                    let mut text = cell.c.to_string();
                                    text.extend(cell.zerowidth().into_iter().flatten());
                                    text
                                },
                                width: if spacer {
                                    0
                                } else if cell.flags.contains(Flags::WIDE_CHAR) {
                                    2
                                } else {
                                    1
                                },
                                style: styles
                                    .binary_search(&terminal_conformance_v2_style(cell))
                                    .expect("fixture style should be registered"),
                            }
                        })
                        .collect()
                })
                .collect(),
            cursor_row,
            cursor_column,
            cursor_visible: terminal.cursor_visible(),
            application_cursor: terminal.application_cursor(),
            alternate_screen: terminal.alternate_screen(),
            bracketed_paste: terminal.bracketed_paste(),
            mouse_mode: mouse_mode_name(terminal.mouse_protocol_mode()).to_string(),
            mouse_encoding: mouse_encoding_name(terminal.mouse_protocol_encoding()).to_string(),
            scrollback_rows: terminal.max_scrollback(),
        }
    }

    fn terminal_conformance_v2_style(cell: &GridCell) -> TerminalConformanceV2Style {
        TerminalConformanceV2Style {
            foreground: terminal_conformance_v2_color(cell.fg),
            background: terminal_conformance_v2_color(cell.bg),
            bold: cell.flags.contains(Flags::BOLD),
            dim: cell.flags.contains(Flags::DIM),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            inverse: cell.flags.contains(Flags::INVERSE),
        }
    }

    fn terminal_conformance_v2_color(color: Color) -> TerminalConformanceV2Color {
        match color {
            Color::Named(name) if (name as usize) < 16 => {
                TerminalConformanceV2Color::Indexed { value: name as u8 }
            }
            Color::Named(_) => TerminalConformanceV2Color::Default,
            Color::Indexed(value) => TerminalConformanceV2Color::Indexed { value },
            Color::Spec(Rgb { r, g, b }) => TerminalConformanceV2Color::Rgb {
                red: r,
                green: g,
                blue: b,
            },
        }
    }

    fn terminal_signature<'a>(
        case: &TerminalConformanceCase,
        chunks: impl IntoIterator<Item = &'a [u8]>,
    ) -> TerminalSignature {
        let mut terminal = TerminalState::new(
            TerminalSize::new(case.columns, case.rows, 0, 0),
            case.scrollback,
        );
        for chunk in chunks {
            terminal.process_bytes(chunk);
        }
        TerminalSignature {
            lines: terminal.all_rows_text(),
            cursor: terminal.cursor_position(),
            cursor_visible: terminal.cursor_visible(),
            application_cursor: terminal.application_cursor(),
            alternate_screen: terminal.alternate_screen(),
            bracketed_paste: terminal.bracketed_paste(),
            mouse_mode: mouse_mode_name(terminal.mouse_protocol_mode()).to_string(),
            mouse_encoding: mouse_encoding_name(terminal.mouse_protocol_encoding()).to_string(),
            scrollback_rows: terminal.max_scrollback(),
        }
    }

    fn mouse_mode_name(mode: MouseProtocolMode) -> &'static str {
        match mode {
            MouseProtocolMode::None => "none",
            MouseProtocolMode::Press => "press",
            MouseProtocolMode::PressRelease => "press_release",
            MouseProtocolMode::ButtonMotion => "button_motion",
            MouseProtocolMode::AnyMotion => "any_motion",
        }
    }

    fn mouse_encoding_name(encoding: MouseProtocolEncoding) -> &'static str {
        match encoding {
            MouseProtocolEncoding::Default => "default",
            MouseProtocolEncoding::Utf8 => "utf8",
            MouseProtocolEncoding::Sgr => "sgr",
        }
    }
}
