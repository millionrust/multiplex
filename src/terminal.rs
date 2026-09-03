use std::cell::{Cell, Ref, RefCell};
use std::mem;
use std::sync::Arc;

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
    pub rows: Vec<Arc<TerminalRow>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalRenderMetrics {
    pub parser_batches: u64,
    pub parser_bytes: u64,
    pub snapshot_requests: u64,
    pub snapshot_cache_hits: u64,
    pub rows_scanned: u64,
    pub rows_rebuilt: u64,
}

#[derive(Default)]
struct TerminalSnapshotCache {
    revision: u64,
    theme_key: Option<(Hsla, Hsla)>,
    initialized: bool,
    snapshot: TerminalSnapshot,
}

pub struct TerminalState {
    parser: Parser,
    size: TerminalSize,
    revision: u64,
    snapshot_cache: RefCell<TerminalSnapshotCache>,
    render_metrics: Cell<TerminalRenderMetrics>,
}

impl TerminalState {
    pub fn new(size: TerminalSize, scrollback: usize) -> Self {
        Self {
            parser: Parser::new(size.rows, size.cols, scrollback),
            size,
            revision: 0,
            snapshot_cache: RefCell::new(TerminalSnapshotCache::default()),
            render_metrics: Cell::new(TerminalRenderMetrics::default()),
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
        self.mark_dirty();
        let mut metrics = self.render_metrics.get();
        metrics.parser_batches = metrics.parser_batches.saturating_add(1);
        metrics.parser_bytes = metrics
            .parser_bytes
            .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        self.render_metrics.set(metrics);
    }

    pub fn controller_snapshot_bytes(&self) -> Vec<u8> {
        self.parser.screen().state_formatted()
    }

    pub fn resize(&mut self, size: TerminalSize) {
        if self.size.cols == size.cols && self.size.rows == size.rows {
            self.size = size;
            return;
        }

        self.size = size;
        self.parser.screen_mut().set_size(size.rows, size.cols);
        self.mark_dirty();
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

    #[cfg(test)]
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    #[cfg(test)]
    pub fn cursor_visible(&self) -> bool {
        !self.parser.screen().hide_cursor()
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
        let previous = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(rows);
        if self.parser.screen().scrollback() != previous {
            self.mark_dirty();
        }
    }

    pub fn reset_scrollback(&mut self) {
        self.set_scrollback(0);
    }

    pub fn scroll_scrollback(&mut self, delta: i32) {
        let current = self.scrollback() as i32;
        let next = (current + delta).max(0) as usize;
        self.set_scrollback(next);
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

    pub fn snapshot(&self) -> Ref<'_, TerminalSnapshot> {
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
            let (rows_scanned, rows_rebuilt) = refresh_snapshot(&self.parser, &mut cache.snapshot);
            cache.revision = self.revision;
            cache.theme_key = Some(theme_key);
            cache.initialized = true;
            metrics.rows_scanned = metrics.rows_scanned.saturating_add(u64::from(rows_scanned));
            metrics.rows_rebuilt = metrics.rows_rebuilt.saturating_add(u64::from(rows_rebuilt));
        } else {
            metrics.snapshot_cache_hits = metrics.snapshot_cache_hits.saturating_add(1);
        }
        self.render_metrics.set(metrics);
        Ref::map(self.snapshot_cache.borrow(), |cache| &cache.snapshot)
    }

    #[cfg(test)]
    pub fn render_metrics(&self) -> TerminalRenderMetrics {
        self.render_metrics.get()
    }

    #[cfg(test)]
    pub fn reset_render_metrics(&self) {
        self.render_metrics.set(TerminalRenderMetrics::default());
    }

    fn mark_dirty(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn refresh_snapshot(parser: &Parser, snapshot: &mut TerminalSnapshot) -> (u16, u16) {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let show_cursor = !screen.hide_cursor() && screen.scrollback() == 0;
    snapshot.rows.resize_with(usize::from(rows), Arc::default);

    let mut rebuilt = 0_u16;
    for row in 0..rows {
        let cached = &snapshot.rows[usize::from(row)];
        if terminal_row_matches(
            screen,
            row,
            cols,
            cursor_row,
            cursor_col,
            show_cursor,
            cached,
        ) {
            continue;
        }
        snapshot.rows[usize::from(row)] = Arc::new(render_terminal_row(
            screen,
            row,
            cols,
            cursor_row,
            cursor_col,
            show_cursor,
        ));
        rebuilt = rebuilt.saturating_add(1);
    }
    (rows, rebuilt)
}

fn terminal_row_matches(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    cursor_row: u16,
    cursor_col: u16,
    show_cursor: bool,
    cached: &TerminalRow,
) -> bool {
    let mut cached_index = 0_usize;
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let Some(cached_cell) = cached.cells.get(cached_index) else {
            return false;
        };
        let text_matches = if cell.has_contents() {
            cached_cell.text == cell.contents()
        } else {
            cached_cell.text == " "
        };
        let cursor_here = show_cursor && row == cursor_row && col == cursor_col;
        if !text_matches || cached_cell.style != style_for_cell(cell, cursor_here) {
            return false;
        }
        cached_index += 1;
    }
    cached_index == cached.cells.len()
}

fn render_terminal_row(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    cursor_row: u16,
    cursor_col: u16,
    show_cursor: bool,
) -> TerminalRow {
    let mut cells = Vec::with_capacity(usize::from(cols));
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
    TerminalRow { cells, text }
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde::Deserialize;

    use super::*;

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
    fn unchanged_terminal_snapshot_is_a_zero_scan_cache_hit() {
        let terminal = TerminalState::new(TerminalSize::new(80, 24, 0, 0), 10_000);

        let first = terminal.snapshot();
        assert_eq!(first.rows.len(), 24);
        drop(first);
        assert_eq!(
            terminal.render_metrics(),
            TerminalRenderMetrics {
                snapshot_requests: 1,
                rows_scanned: 24,
                rows_rebuilt: 24,
                ..TerminalRenderMetrics::default()
            }
        );

        let second = terminal.snapshot();
        assert_eq!(second.rows.len(), 24);
        drop(second);
        assert_eq!(
            terminal.render_metrics(),
            TerminalRenderMetrics {
                snapshot_requests: 2,
                snapshot_cache_hits: 1,
                rows_scanned: 24,
                rows_rebuilt: 24,
                ..TerminalRenderMetrics::default()
            }
        );
    }

    #[test]
    fn terminal_snapshot_rebuilds_only_rows_changed_by_output() {
        let mut terminal = TerminalState::new(TerminalSize::new(80, 24, 0, 0), 10_000);
        drop(terminal.snapshot());
        terminal.reset_render_metrics();

        terminal.process_bytes(b"hello");
        drop(terminal.snapshot());

        assert_eq!(
            terminal.render_metrics(),
            TerminalRenderMetrics {
                parser_batches: 1,
                parser_bytes: 5,
                snapshot_requests: 1,
                rows_scanned: 24,
                rows_rebuilt: 1,
                ..TerminalRenderMetrics::default()
            }
        );

        terminal.reset_render_metrics();
        terminal.process_bytes(b"\r\nworld");
        drop(terminal.snapshot());
        let metrics = terminal.render_metrics();
        assert_eq!(metrics.rows_scanned, 24);
        assert!(
            metrics.rows_rebuilt <= 2,
            "cursor movement and one new line rebuilt {} rows",
            metrics.rows_rebuilt
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

        let metrics = terminal.render_metrics();
        assert_eq!(metrics.rows_scanned, 8);
        assert!(metrics.rows_rebuilt <= 8);
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
        assert!(interactive_metrics.rows_rebuilt <= (RUNS * 2) as u64);

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
            "terminal component profile: startup p50={}us p95={}us p99={}us; input+snapshot p50={}us p95={}us p99={}us; sustained={throughput_mib_s:.2}MiB/s bytes={} batches={} rows_scanned={} rows_rebuilt={}",
            startup_p50.as_micros(),
            startup_p95.as_micros(),
            startup_p99.as_micros(),
            input_p50.as_micros(),
            input_p95.as_micros(),
            input_p99.as_micros(),
            output_bytes.len(),
            output_metrics.parser_batches,
            output_metrics.rows_scanned,
            output_metrics.rows_rebuilt,
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

    #[test]
    fn terminal_conformance_v1_matches_canonical_fixture() {
        let fixture: TerminalConformanceFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/terminal/terminal-conformance-v1.json"
        ))
        .expect("terminal conformance fixture should decode");
        assert_eq!(fixture.schema_version, 1);

        for case in fixture.cases {
            let mut terminal = TerminalState::new(
                TerminalSize::new(case.columns, case.rows, 0, 0),
                case.scrollback,
            );
            for chunk in &case.chunks {
                terminal.process_bytes(chunk);
            }

            let lines = terminal
                .all_rows_text()
                .into_iter()
                .map(|line| line.trim_end().to_string())
                .collect::<Vec<_>>();
            assert_eq!(lines, case.expected.lines, "{} lines", case.name);
            assert_eq!(
                terminal.cursor_position(),
                (case.expected.cursor_row, case.expected.cursor_column),
                "{} cursor",
                case.name
            );
            assert_eq!(
                terminal.cursor_visible(),
                case.expected.cursor_visible,
                "{} cursor visibility",
                case.name
            );
            assert_eq!(
                terminal.application_cursor(),
                case.expected.application_cursor,
                "{} application cursor",
                case.name
            );
            assert_eq!(
                terminal.alternate_screen(),
                case.expected.alternate_screen,
                "{} alternate screen",
                case.name
            );
            assert_eq!(
                terminal.bracketed_paste(),
                case.expected.bracketed_paste,
                "{} bracketed paste",
                case.name
            );
            assert_eq!(
                mouse_mode_name(terminal.mouse_protocol_mode()),
                case.expected.mouse_mode,
                "{} mouse mode",
                case.name
            );
            assert_eq!(
                mouse_encoding_name(terminal.mouse_protocol_encoding()),
                case.expected.mouse_encoding,
                "{} mouse encoding",
                case.name
            );
            assert_eq!(
                terminal.max_scrollback(),
                case.expected.scrollback_rows,
                "{} scrollback",
                case.name
            );
        }
    }

    #[test]
    fn terminal_conformance_v1_is_chunk_boundary_invariant() {
        let fixture: TerminalConformanceFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/terminal/terminal-conformance-v1.json"
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
            assert_eq!(actual, case.expected, "{} configured operations", case.name);
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
                        actual, case.expected,
                        "{} operation {operation_index} split at {split}",
                        case.name
                    );
                }
            }
        }
    }

    fn terminal_conformance_v2_fixture() -> TerminalConformanceV2Fixture {
        serde_json::from_str(include_str!(
            "../tests/fixtures/terminal/terminal-conformance-v2.json"
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

        let screen = terminal.parser.screen();
        let (rows, columns) = screen.size();
        let (cursor_row, cursor_column) = screen.cursor_position();
        TerminalConformanceV2Expected {
            lines: screen
                .rows(0, columns)
                .map(|line| line.trim_end().to_string())
                .collect(),
            cells: (0..rows)
                .map(|row| {
                    (0..columns)
                        .map(|column| {
                            let cell = screen.cell(row, column).expect("fixture cell exists");
                            let style = terminal_conformance_v2_style(cell);
                            TerminalConformanceV2Cell {
                                text: if cell.is_wide_continuation() {
                                    String::new()
                                } else if cell.has_contents() {
                                    cell.contents().to_string()
                                } else {
                                    " ".to_string()
                                },
                                width: if cell.is_wide_continuation() {
                                    0
                                } else if cell.is_wide() {
                                    2
                                } else {
                                    1
                                },
                                style: styles
                                    .binary_search(&style)
                                    .expect("fixture style should be registered"),
                            }
                        })
                        .collect()
                })
                .collect(),
            cursor_row,
            cursor_column,
            cursor_visible: !screen.hide_cursor(),
            application_cursor: screen.application_cursor(),
            alternate_screen: screen.alternate_screen(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_mode: mouse_mode_name(screen.mouse_protocol_mode()).to_string(),
            mouse_encoding: mouse_encoding_name(screen.mouse_protocol_encoding()).to_string(),
            scrollback_rows: terminal.max_scrollback(),
        }
    }

    fn terminal_conformance_v2_style(cell: &vt100::Cell) -> TerminalConformanceV2Style {
        TerminalConformanceV2Style {
            foreground: terminal_conformance_v2_color(cell.fgcolor()),
            background: terminal_conformance_v2_color(cell.bgcolor()),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    fn terminal_conformance_v2_color(color: Color) -> TerminalConformanceV2Color {
        match color {
            Color::Default => TerminalConformanceV2Color::Default,
            Color::Idx(value) => TerminalConformanceV2Color::Indexed { value },
            Color::Rgb(red, green, blue) => TerminalConformanceV2Color::Rgb { red, green, blue },
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
            lines: terminal
                .all_rows_text()
                .into_iter()
                .map(|line| line.trim_end().to_string())
                .collect(),
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
