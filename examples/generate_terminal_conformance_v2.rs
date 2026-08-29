use std::{collections::BTreeSet, env, fs, path::PathBuf};

use serde::Serialize;
use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode, Parser};

const UNICODE_WIDTH_VERSION: &str = "0.2.2";

#[derive(Serialize)]
struct Fixture {
    schema_version: u32,
    unicode_width_version: &'static str,
    styles: Vec<CellStyle>,
    cases: Vec<FixtureCase>,
}

#[derive(Serialize)]
struct FixtureCase {
    name: &'static str,
    columns: u16,
    rows: u16,
    scrollback: usize,
    operations: Vec<Operation>,
    expected: Expected,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Operation {
    Process { bytes: Vec<u8> },
    Resize { columns: u16, rows: u16 },
}

#[derive(Serialize)]
struct Expected {
    lines: Vec<String>,
    cells: Vec<Vec<Cell>>,
    cursor_row: u16,
    cursor_column: u16,
    cursor_visible: bool,
    application_cursor: bool,
    alternate_screen: bool,
    bracketed_paste: bool,
    mouse_mode: &'static str,
    mouse_encoding: &'static str,
    scrollback_rows: usize,
}

#[derive(Serialize)]
struct Cell {
    text: String,
    width: u8,
    style: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct CellStyle {
    foreground: CellColor,
    background: CellColor,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CellColor {
    Default,
    Indexed { value: u8 },
    Rgb { red: u8, green: u8, blue: u8 },
}

struct RenderedCase {
    name: &'static str,
    columns: u16,
    rows: u16,
    scrollback: usize,
    operations: Vec<Operation>,
    lines: Vec<String>,
    cells: Vec<Vec<RenderedCell>>,
    cursor_row: u16,
    cursor_column: u16,
    cursor_visible: bool,
    application_cursor: bool,
    alternate_screen: bool,
    bracketed_paste: bool,
    mouse_mode: &'static str,
    mouse_encoding: &'static str,
    scrollback_rows: usize,
}

struct RenderedCell {
    text: String,
    width: u8,
    style: CellStyle,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let check = match env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(_) => return Err("usage: generate_terminal_conformance_v2 [--check]".into()),
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/terminal/terminal-conformance-v2.json");
    let rendered = case_specs()
        .into_iter()
        .map(render_case)
        .collect::<Vec<_>>();
    let styles = rendered
        .iter()
        .flat_map(|case| case.cells.iter().flatten())
        .map(|cell| cell.style.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let fixture = Fixture {
        schema_version: 2,
        unicode_width_version: UNICODE_WIDTH_VERSION,
        cases: rendered
            .into_iter()
            .map(|case| finish_case(case, &styles))
            .collect(),
        styles,
    };
    let expected = format!("{}\n", serde_json::to_string_pretty(&fixture)?);

    if check {
        if fs::read_to_string(&path)? != expected {
            return Err(format!(
                "terminal conformance v2 fixture is stale: {}",
                path.display()
            )
            .into());
        }
        println!("Terminal conformance v2 fixture matches desktop vt100.");
    } else {
        fs::write(&path, expected)?;
        println!("Generated terminal conformance v2 fixture from desktop vt100.");
    }
    Ok(())
}

fn case_specs() -> Vec<FixtureCase> {
    vec![
        spec(
            "sgr_attributes_and_resets",
            10,
            2,
            vec![
                process("\u{1b}[1;3;4;31;44mA"),
                process("\u{1b}[22;23;24;39;49mB\u{1b}[2;7mC\u{1b}[27mD\u{1b}[0mE"),
            ],
        ),
        spec(
            "sgr_indexed_and_rgb",
            8,
            2,
            vec![
                process("\u{1b}[38;5;196;48;2;1;2;3mX"),
                process("\u{1b}[38;2;12;34;56;48;5;17mY\u{1b}[0mZ"),
            ],
        ),
        spec(
            "unicode_zero_narrow_and_wide",
            10,
            2,
            vec![process("e\u{0301}界Ω🙂X")],
        ),
        spec(
            "wide_wraps_at_occupied_margin",
            4,
            2,
            vec![process("abc界Z")],
        ),
        spec(
            "overwrite_wide_continuation_repairs_pair",
            5,
            2,
            vec![process("界\u{1b}[1DX")],
        ),
        spec(
            "resize_shrink_columns_repairs_wide_cell",
            6,
            2,
            vec![process("ab界cd"), resize(3, 2)],
        ),
        spec(
            "resize_grow_preserves_rows_without_reflow",
            4,
            2,
            vec![process("abcd\r\nxy"), resize(6, 3)],
        ),
        spec(
            "resize_shrink_rows_clamps_cursor",
            6,
            3,
            vec![process("one\r\ntwo\r\nthree"), resize(6, 2)],
        ),
        spec(
            "alternate_screen_resize_restores_resized_primary",
            6,
            2,
            vec![
                process("main\u{1b}[?1049halt"),
                resize(4, 3),
                process("\u{1b}[?1049l"),
            ],
        ),
    ]
}

fn spec(name: &'static str, columns: u16, rows: u16, operations: Vec<Operation>) -> FixtureCase {
    FixtureCase {
        name,
        columns,
        rows,
        scrollback: 4,
        operations,
        expected: empty_expected(),
    }
}

fn process(value: &str) -> Operation {
    Operation::Process {
        bytes: value.as_bytes().to_vec(),
    }
}

fn resize(columns: u16, rows: u16) -> Operation {
    Operation::Resize { columns, rows }
}

fn empty_expected() -> Expected {
    Expected {
        lines: Vec::new(),
        cells: Vec::new(),
        cursor_row: 0,
        cursor_column: 0,
        cursor_visible: true,
        application_cursor: false,
        alternate_screen: false,
        bracketed_paste: false,
        mouse_mode: "none",
        mouse_encoding: "default",
        scrollback_rows: 0,
    }
}

fn render_case(case: FixtureCase) -> RenderedCase {
    let mut parser = Parser::new(case.rows, case.columns, case.scrollback);
    for operation in &case.operations {
        match operation {
            Operation::Process { bytes } => parser.process(bytes),
            Operation::Resize { columns, rows } => parser.screen_mut().set_size(*rows, *columns),
        }
    }
    let screen = parser.screen();
    let (rows, columns) = screen.size();
    let (cursor_row, cursor_column) = screen.cursor_position();
    let cells = (0..rows)
        .map(|row| {
            (0..columns)
                .map(|column| {
                    let cell = screen.cell(row, column).expect("fixture cell exists");
                    RenderedCell {
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
                        style: style(cell),
                    }
                })
                .collect()
        })
        .collect();
    let mut scrollback = screen.clone();
    scrollback.set_scrollback(usize::MAX);
    RenderedCase {
        name: case.name,
        columns: case.columns,
        rows: case.rows,
        scrollback: case.scrollback,
        operations: case.operations,
        lines: screen
            .rows(0, columns)
            .map(|line| line.trim_end().to_string())
            .collect(),
        cells,
        cursor_row,
        cursor_column,
        cursor_visible: !screen.hide_cursor(),
        application_cursor: screen.application_cursor(),
        alternate_screen: screen.alternate_screen(),
        bracketed_paste: screen.bracketed_paste(),
        mouse_mode: mouse_mode_name(screen.mouse_protocol_mode()),
        mouse_encoding: mouse_encoding_name(screen.mouse_protocol_encoding()),
        scrollback_rows: scrollback.scrollback(),
    }
}

fn finish_case(case: RenderedCase, styles: &[CellStyle]) -> FixtureCase {
    FixtureCase {
        name: case.name,
        columns: case.columns,
        rows: case.rows,
        scrollback: case.scrollback,
        operations: case.operations,
        expected: Expected {
            lines: case.lines,
            cells: case
                .cells
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|cell| Cell {
                            text: cell.text,
                            width: cell.width,
                            style: styles
                                .binary_search(&cell.style)
                                .expect("collected style must exist"),
                        })
                        .collect()
                })
                .collect(),
            cursor_row: case.cursor_row,
            cursor_column: case.cursor_column,
            cursor_visible: case.cursor_visible,
            application_cursor: case.application_cursor,
            alternate_screen: case.alternate_screen,
            bracketed_paste: case.bracketed_paste,
            mouse_mode: case.mouse_mode,
            mouse_encoding: case.mouse_encoding,
            scrollback_rows: case.scrollback_rows,
        },
    }
}

fn style(cell: &vt100::Cell) -> CellStyle {
    CellStyle {
        foreground: color(cell.fgcolor()),
        background: color(cell.bgcolor()),
        bold: cell.bold(),
        dim: cell.dim(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
    }
}

fn color(color: Color) -> CellColor {
    match color {
        Color::Default => CellColor::Default,
        Color::Idx(value) => CellColor::Indexed { value },
        Color::Rgb(red, green, blue) => CellColor::Rgb { red, green, blue },
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
