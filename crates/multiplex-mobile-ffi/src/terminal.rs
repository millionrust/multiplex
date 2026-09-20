use crate::{TermiRustMobileResult, error_result, read_bytes, success_result};
use serde::Serialize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode, Parser, Screen};

const MAX_COLUMNS: u16 = 1_000;
const MAX_ROWS: u16 = 1_000;
const MAX_SCROLLBACK_ROWS: usize = 50_000;
const MAX_PROCESS_BYTES: usize = 1024 * 1024;

pub struct TermiRustMobileTerminal {
    parser: Parser,
    scrollback_rows: usize,
}

#[derive(Serialize)]
struct TerminalSnapshot {
    schema_version: u8,
    columns: u16,
    rows: u16,
    lines: Vec<String>,
    cells: Vec<Vec<TerminalCell>>,
    cursor_row: u16,
    cursor_column: u16,
    cursor_visible: bool,
    application_cursor: bool,
    application_keypad: bool,
    alternate_screen: bool,
    bracketed_paste: bool,
    mouse_mode: &'static str,
    mouse_encoding: &'static str,
    scrollback_rows: usize,
    retained_cells: usize,
    accounted_bytes: usize,
}

#[derive(Serialize)]
struct TerminalCell {
    text: String,
    width: u8,
    foreground: TerminalColor,
    background: TerminalColor,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TerminalColor {
    Default,
    Indexed { value: u8 },
    Rgb { red: u8, green: u8, blue: u8 },
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_create(
    columns: u16,
    rows: u16,
    scrollback_rows: usize,
) -> *mut TermiRustMobileTerminal {
    catch_unwind(AssertUnwindSafe(|| {
        validated_dimensions(columns, rows, scrollback_rows).map_or(
            std::ptr::null_mut(),
            |(columns, rows, scrollback_rows)| {
                Box::into_raw(Box::new(TermiRustMobileTerminal {
                    parser: Parser::new(rows, columns, scrollback_rows),
                    scrollback_rows,
                }))
            },
        )
    }))
    .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_process(
    terminal: *mut TermiRustMobileTerminal,
    input_ptr: *const u8,
    input_len: usize,
) -> TermiRustMobileResult {
    ffi_result(|| {
        let terminal = terminal_mut(terminal)?;
        if input_len > MAX_PROCESS_BYTES {
            return Err("TermiRust mobile terminal frame exceeded 1 MiB.".to_string());
        }
        let input = read_bytes(input_ptr, input_len, "terminal input")?;
        terminal.parser.process(input);
        snapshot_json(terminal)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_feed(
    terminal: *mut TermiRustMobileTerminal,
    input_ptr: *const u8,
    input_len: usize,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if input_len > MAX_PROCESS_BYTES {
            return false;
        }
        let Ok(terminal) = terminal_mut(terminal) else {
            return false;
        };
        let Ok(input) = read_bytes(input_ptr, input_len, "terminal input") else {
            return false;
        };
        terminal.parser.process(input);
        true
    }))
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_resize(
    terminal: *mut TermiRustMobileTerminal,
    columns: u16,
    rows: u16,
) -> TermiRustMobileResult {
    ffi_result(|| {
        let terminal = terminal_mut(terminal)?;
        let (columns, rows, _) = validated_dimensions(columns, rows, terminal.scrollback_rows)?;
        terminal.parser.screen_mut().set_size(rows, columns);
        snapshot_json(terminal)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_snapshot(
    terminal: *mut TermiRustMobileTerminal,
) -> TermiRustMobileResult {
    ffi_result(|| snapshot_json(terminal_mut(terminal)?))
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_terminal_destroy(terminal: *mut TermiRustMobileTerminal) {
    if terminal.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(terminal));
    }));
}

fn ffi_result(operation: impl FnOnce() -> Result<Vec<u8>, String>) -> TermiRustMobileResult {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(bytes)) => success_result(bytes),
        Ok(Err(error)) => error_result(&error),
        Err(_) => error_result("TermiRust mobile terminal operation panicked."),
    }
}

fn terminal_mut<'a>(
    terminal: *mut TermiRustMobileTerminal,
) -> Result<&'a mut TermiRustMobileTerminal, String> {
    if terminal.is_null() {
        return Err("TermiRust mobile terminal handle was null.".to_string());
    }
    Ok(unsafe { &mut *terminal })
}

fn validated_dimensions(
    columns: u16,
    rows: u16,
    scrollback_rows: usize,
) -> Result<(u16, u16, usize), String> {
    if columns == 0 || columns > MAX_COLUMNS {
        return Err(format!(
            "Terminal columns must be between 1 and {MAX_COLUMNS}."
        ));
    }
    if rows == 0 || rows > MAX_ROWS {
        return Err(format!("Terminal rows must be between 1 and {MAX_ROWS}."));
    }
    if scrollback_rows > MAX_SCROLLBACK_ROWS {
        return Err(format!(
            "Terminal scrollback must not exceed {MAX_SCROLLBACK_ROWS} rows."
        ));
    }
    Ok((columns, rows, scrollback_rows))
}

fn snapshot_json(terminal: &TermiRustMobileTerminal) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&snapshot(terminal))
        .map_err(|error| format!("Unable to encode mobile terminal snapshot: {error}"))
}

fn snapshot(terminal: &TermiRustMobileTerminal) -> TerminalSnapshot {
    let screen = terminal.parser.screen();
    let (rows, columns) = screen.size();
    let viewport_rows = usize::from(rows);
    let max_scrollback = {
        let mut top = screen.clone();
        top.set_scrollback(usize::MAX);
        top.scrollback()
    };
    let mut lines = Vec::with_capacity(max_scrollback + viewport_rows);
    let mut cells = Vec::with_capacity(max_scrollback + viewport_rows);

    let full_pages = max_scrollback / viewport_rows;
    let remainder = max_scrollback % viewport_rows;
    for page in 0..full_pages {
        let mut view = screen.clone();
        view.set_scrollback(max_scrollback - page * viewport_rows);
        append_view(&view, rows, columns, &mut lines, &mut cells);
    }
    if remainder > 0 {
        let mut view = screen.clone();
        view.set_scrollback(remainder);
        append_view(
            &view,
            u16::try_from(remainder).unwrap_or(rows),
            columns,
            &mut lines,
            &mut cells,
        );
    }
    let mut current = screen.clone();
    current.set_scrollback(0);
    append_view(&current, rows, columns, &mut lines, &mut cells);

    let (cursor_row, cursor_column) = screen.cursor_position();
    let retained_cells = cells
        .iter()
        .map(|row| {
            row.iter()
                .rposition(cell_has_content)
                .map_or(0, |index| index + 1)
        })
        .sum();
    let accounted_bytes = cells.iter().flatten().fold(0usize, |total, cell| {
        total.saturating_add(cell.text.len()).saturating_add(64)
    });

    TerminalSnapshot {
        schema_version: 1,
        columns,
        rows,
        lines,
        cells,
        cursor_row,
        cursor_column,
        cursor_visible: !screen.hide_cursor(),
        application_cursor: screen.application_cursor(),
        application_keypad: screen.application_keypad(),
        alternate_screen: screen.alternate_screen(),
        bracketed_paste: screen.bracketed_paste(),
        mouse_mode: mouse_mode_name(screen.mouse_protocol_mode()),
        mouse_encoding: mouse_encoding_name(screen.mouse_protocol_encoding()),
        scrollback_rows: max_scrollback,
        retained_cells,
        accounted_bytes,
    }
}

fn append_view(
    screen: &Screen,
    row_count: u16,
    columns: u16,
    lines: &mut Vec<String>,
    cells: &mut Vec<Vec<TerminalCell>>,
) {
    lines.extend(screen.rows(0, columns).take(usize::from(row_count)));
    for row in 0..row_count {
        let mut output = Vec::with_capacity(usize::from(columns));
        for column in 0..columns {
            if let Some(cell) = screen.cell(row, column) {
                output.push(TerminalCell {
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
                    foreground: color(cell.fgcolor()),
                    background: color(cell.bgcolor()),
                    bold: cell.bold(),
                    dim: cell.dim(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                });
            }
        }
        while output.last().is_some_and(|cell| !cell_has_content(cell)) {
            output.pop();
        }
        cells.push(output);
    }
}

fn cell_has_content(cell: &TerminalCell) -> bool {
    cell.text != " "
        || cell.width != 1
        || !matches!(cell.foreground, TerminalColor::Default)
        || !matches!(cell.background, TerminalColor::Default)
        || cell.bold
        || cell.dim
        || cell.italic
        || cell.underline
        || cell.inverse
}

fn color(value: Color) -> TerminalColor {
    match value {
        Color::Default => TerminalColor::Default,
        Color::Idx(value) => TerminalColor::Indexed { value },
        Color::Rgb(red, green, blue) => TerminalColor::Rgb { red, green, blue },
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct InteractiveFixture {
        schema_version: u8,
        cases: Vec<InteractiveCase>,
    }

    #[derive(Deserialize)]
    struct InteractiveCase {
        name: String,
        columns: u16,
        rows: u16,
        scrollback: usize,
        input: String,
        expected: InteractiveExpected,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct InteractiveExpected {
        lines: Vec<String>,
        cursor_row: u16,
        cursor_column: u16,
        cursor_visible: bool,
        application_cursor: bool,
        application_keypad: bool,
        alternate_screen: bool,
        bracketed_paste: bool,
        mouse_mode: String,
        mouse_encoding: String,
        scrollback_rows: usize,
    }

    #[test]
    fn stateful_terminal_handles_full_screen_editing_and_modes() {
        let mut terminal = TermiRustMobileTerminal {
            parser: Parser::new(4, 12, 8),
            scrollback_rows: 8,
        };
        terminal.parser.process(
            b"one\r\ntwo\r\nthree\x1b[2;1H\x1b[Linsert\x1b[3;1H\x1b[2@>>\x1b[?1049hALT\x1b[?1h\x1b[?2004h",
        );
        let alternate = serde_json::to_value(snapshot(&terminal)).unwrap();
        assert_eq!(alternate["alternate_screen"], true);
        assert_eq!(alternate["application_cursor"], true);
        assert_eq!(alternate["bracketed_paste"], true);
        assert_eq!(alternate["lines"][0], "ALT");

        terminal.parser.process(b"\x1b[?1049l");
        let primary = serde_json::to_value(snapshot(&terminal)).unwrap();
        assert_eq!(primary["alternate_screen"], false);
        assert_eq!(primary["lines"][0], "one");
        assert_eq!(primary["lines"][1], "insert");
        assert_eq!(primary["lines"][2], ">>two");
    }

    #[test]
    fn snapshot_preserves_styles_unicode_and_mouse_modes() {
        let mut terminal = TermiRustMobileTerminal {
            parser: Parser::new(2, 8, 2),
            scrollback_rows: 2,
        };
        terminal
            .parser
            .process("\x1b[1;38;2;1;2;3m界e\u{301}\x1b[?1002h\x1b[?1006h".as_bytes());
        let value: Value = serde_json::to_value(snapshot(&terminal)).unwrap();
        assert_eq!(value["cells"][0][0]["text"], "界");
        assert_eq!(value["cells"][0][0]["width"], 2);
        assert_eq!(value["cells"][0][2]["text"], "é");
        assert_eq!(value["cells"][0][0]["foreground"]["kind"], "rgb");
        assert_eq!(value["mouse_mode"], "button_motion");
        assert_eq!(value["mouse_encoding"], "sgr");
    }

    #[test]
    fn ffi_rejects_invalid_dimensions_and_oversized_frames() {
        assert!(termirust_mobile_terminal_create(0, 24, 10).is_null());
        let terminal = termirust_mobile_terminal_create(80, 24, 10);
        assert!(!terminal.is_null());
        let bytes = vec![b'x'; MAX_PROCESS_BYTES + 1];
        let result = termirust_mobile_terminal_process(terminal, bytes.as_ptr(), bytes.len());
        assert!(!result.ok);
        crate::termirust_mobile_free_result(result);
        termirust_mobile_terminal_destroy(terminal);
    }

    #[test]
    fn feed_updates_state_without_serializing_a_snapshot() {
        let terminal = termirust_mobile_terminal_create(12, 4, 8);
        assert!(!terminal.is_null());
        let bytes = b"one\r\ntwo\x1b[1;1Htop";
        assert!(termirust_mobile_terminal_feed(
            terminal,
            bytes.as_ptr(),
            bytes.len()
        ));
        let value = unsafe { &*terminal };
        assert_eq!(snapshot(value).lines[0], "top");
        termirust_mobile_terminal_destroy(terminal);
    }

    #[test]
    fn high_output_feed_retains_bounded_scrollback_and_compact_snapshots() {
        let mut terminal = TermiRustMobileTerminal {
            parser: Parser::new(24, 80, 2_000),
            scrollback_rows: 2_000,
        };
        let frame = (0..8_000)
            .map(|line| format!("line-{line:05}\r\n"))
            .collect::<String>();
        for chunk in frame.as_bytes().chunks(16 * 1_024) {
            terminal.parser.process(chunk);
        }

        let snapshot = snapshot(&terminal);
        assert_eq!(snapshot.scrollback_rows, 2_000);
        assert!(snapshot.lines.last().is_some_and(|line| line.is_empty()));
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        assert!(encoded.len() < 8 * 1_024 * 1_024, "{}", encoded.len());
    }

    #[test]
    fn interactive_fixture_matches_at_every_network_split() {
        let fixture: InteractiveFixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/terminal/terminal-interactive-v1.json"
        ))
        .unwrap();
        assert_eq!(fixture.schema_version, 1);
        for case in fixture.cases {
            for split in 0..=case.input.len() {
                let mut terminal = TermiRustMobileTerminal {
                    parser: Parser::new(case.rows, case.columns, case.scrollback),
                    scrollback_rows: case.scrollback,
                };
                terminal.parser.process(&case.input.as_bytes()[..split]);
                terminal.parser.process(&case.input.as_bytes()[split..]);
                assert_eq!(
                    interactive_expected(&terminal),
                    case.expected,
                    "{} split at {split}",
                    case.name
                );
            }
        }
    }

    fn interactive_expected(terminal: &TermiRustMobileTerminal) -> InteractiveExpected {
        let snapshot = snapshot(terminal);
        InteractiveExpected {
            lines: snapshot.lines,
            cursor_row: snapshot.cursor_row,
            cursor_column: snapshot.cursor_column,
            cursor_visible: snapshot.cursor_visible,
            application_cursor: snapshot.application_cursor,
            application_keypad: snapshot.application_keypad,
            alternate_screen: snapshot.alternate_screen,
            bracketed_paste: snapshot.bracketed_paste,
            mouse_mode: snapshot.mouse_mode.to_string(),
            mouse_encoding: snapshot.mouse_encoding.to_string(),
            scrollback_rows: snapshot.scrollback_rows,
        }
    }
}
