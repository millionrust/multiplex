//! Keystroke and mouse encoding helpers shared by the terminal UI.

use crate::terminal::{MouseProtocolEncoding, MouseProtocolMode};
use gpui::{Keystroke, Modifiers, ScrollDelta};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

pub fn encode_terminal_key(
    key: &str,
    text: Option<&str>,
    modifiers: TerminalModifiers,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    let mut bytes = if modifiers.control {
        vec![encode_control_char(key)?]
    } else {
        match key {
            "enter" => b"\r".to_vec(),
            "backspace" => vec![0x7f],
            "tab" if modifiers.shift => b"\x1b[Z".to_vec(),
            "tab" => b"\t".to_vec(),
            "escape" => vec![0x1b],
            "up" => cursor_key(application_cursor, b'A'),
            "down" => cursor_key(application_cursor, b'B'),
            "right" => cursor_key(application_cursor, b'C'),
            "left" => cursor_key(application_cursor, b'D'),
            "home" => cursor_key(application_cursor, b'H'),
            "end" => cursor_key(application_cursor, b'F'),
            "insert" => b"\x1b[2~".to_vec(),
            "delete" => b"\x1b[3~".to_vec(),
            "pageup" => b"\x1b[5~".to_vec(),
            "pagedown" => b"\x1b[6~".to_vec(),
            "space" => b" ".to_vec(),
            _ => text?.as_bytes().to_vec(),
        }
    };

    if modifiers.alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

fn cursor_key(application_cursor: bool, final_byte: u8) -> Vec<u8> {
    vec![
        0x1b,
        if application_cursor { b'O' } else { b'[' },
        final_byte,
    ]
}

#[cfg(test)]
pub fn normalize_terminal_paste(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").into_bytes()
}

#[cfg(test)]
pub fn terminal_paste_requires_confirmation(bytes: &[u8], threshold: usize) -> bool {
    bytes.len() > threshold || bytes.contains(&b'\n') || bytes.contains(&b'\r')
}

#[cfg(test)]
pub fn wrap_terminal_paste(bytes: &[u8], bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return bytes.to_vec();
    }
    let mut wrapped = Vec::with_capacity(bytes.len() + 12);
    wrapped.extend_from_slice(b"\x1b[200~");
    wrapped.extend_from_slice(bytes);
    wrapped.extend_from_slice(b"\x1b[201~");
    wrapped
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSelectionPoint {
    pub row: usize,
    pub column: usize,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSelectionCell {
    pub text: String,
    pub width: usize,
}

#[cfg(test)]
pub fn extract_terminal_selection(
    rows: &[Vec<TerminalSelectionCell>],
    start: TerminalSelectionPoint,
    end: TerminalSelectionPoint,
) -> String {
    if rows.is_empty()
        || start.row >= rows.len()
        || start.row > end.row
        || (start.row == end.row && start.column >= end.column)
    {
        return String::new();
    }

    let mut selected_rows = Vec::new();
    let last_row = end.row.min(rows.len().saturating_sub(1));
    for (row_index, row) in rows
        .iter()
        .enumerate()
        .take(last_row.saturating_add(1))
        .skip(start.row)
    {
        let from = if row_index == start.row {
            start.column
        } else {
            0
        };
        let to = if row_index == end.row {
            end.column
        } else {
            usize::MAX
        };
        let mut column = 0;
        let mut text = String::new();
        for cell in row {
            if cell.width == 0 {
                continue;
            }
            if column >= from && column < to {
                text.push_str(&cell.text);
            }
            column = column.saturating_add(cell.width);
        }
        selected_rows.push(text);
    }
    selected_rows.join("\n")
}

#[cfg(test)]
pub fn visible_http_urls(text: &str, max_url_bytes: usize, max_urls: usize) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split_whitespace() {
        let Some(start) = [token.find("https://"), token.find("http://")]
            .into_iter()
            .flatten()
            .min()
        else {
            continue;
        };
        let candidate =
            token[start..].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
        if candidate.len() <= max_url_bytes
            && candidate.is_ascii()
            && !candidate.contains(['\'', '"', '<', '>'])
            && candidate
                .split_once("://")
                .is_some_and(|(_, authority)| !authority.is_empty())
        {
            urls.push(candidate.to_string());
            if urls.len() == max_urls {
                break;
            }
        }
    }
    urls
}

#[cfg(test)]
#[derive(Default)]
pub struct TerminalImeState {
    marked_text: String,
}

#[cfg(test)]
impl TerminalImeState {
    pub fn update(&mut self, text: &str) {
        self.marked_text.clear();
        self.marked_text.push_str(text);
    }

    pub fn cancel(&mut self) {
        self.marked_text.clear();
    }

    pub fn commit(&mut self, text: &str) -> Option<Vec<u8>> {
        self.marked_text.clear();
        (!text.is_empty()).then(|| text.as_bytes().to_vec())
    }

    pub fn finish(&mut self) -> Option<Vec<u8>> {
        let marked = std::mem::take(&mut self.marked_text);
        (!marked.is_empty()).then(|| marked.into_bytes())
    }
}

#[derive(Clone, Copy)]
pub enum MouseEventKind {
    Press,
    Move { dragging: bool },
    Release,
    Wheel { delta: ScrollDelta },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCellPos {
    pub row: u16,
    pub col: u16,
}

pub fn encode_terminal_input(keystroke: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    if keystroke.modifiers.function || keystroke.key.is_empty() {
        return None;
    }

    encode_terminal_key(
        &keystroke.key,
        keystroke.key_char.as_deref(),
        TerminalModifiers {
            shift: keystroke.modifiers.shift,
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
        },
        application_cursor,
    )
}

/// What a shortcut asks the app to do. Every one of them is held with the platform's primary
/// modifier: Command on macOS, Control elsewhere, which is what GPUI calls `secondary`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppShortcut {
    CommandPalette,
    CycleWorkspace {
        forward: bool,
    },
    OpenFiles,
    ToggleFilesAndTerminal,
    BroadcastInput,
    ClearScreen,
    LocalTerminal,
    /// The numbered library sections, 1 through 7.
    NavigateSection(u8),
    Settings,
    LogsOrHostSearch,
    NewHostOrSession,
    DuplicatePane,
}

/// What a shortcut does while a terminal pane has the keyboard, before the keystroke would be
/// typed into the program.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalShortcut {
    FocusChrome,
    Copy,
    Paste,
    Search,
    CommandPalette,
    CloseWorkspace,
}

/// Classifies a keystroke, with no reference to what the app is showing. Whether the action can
/// run is the caller's decision.
pub fn app_shortcut(keystroke: &Keystroke) -> Option<AppShortcut> {
    if !keystroke.modifiers.secondary() {
        return None;
    }
    let shift = keystroke.modifiers.shift;
    let alt = keystroke.modifiers.alt;
    let key = keystroke.key.as_str();
    if alt {
        return match key {
            "right" | "tab" => Some(AppShortcut::CycleWorkspace { forward: true }),
            "left" => Some(AppShortcut::CycleWorkspace { forward: false }),
            _ => None,
        };
    }
    if shift {
        return match key {
            "f" => Some(AppShortcut::OpenFiles),
            "t" => Some(AppShortcut::ToggleFilesAndTerminal),
            "b" => Some(AppShortcut::BroadcastInput),
            "l" => Some(AppShortcut::ClearScreen),
            _ => None,
        };
    }
    match key {
        "k" => Some(AppShortcut::CommandPalette),
        "t" => Some(AppShortcut::LocalTerminal),
        "1" | "2" | "3" | "4" | "5" | "6" | "7" => {
            key.parse().ok().map(AppShortcut::NavigateSection)
        }
        "," => Some(AppShortcut::Settings),
        "l" => Some(AppShortcut::LogsOrHostSearch),
        "n" => Some(AppShortcut::NewHostOrSession),
        "d" => Some(AppShortcut::DuplicatePane),
        _ => None,
    }
}

/// Classifies a keystroke that reached a focused terminal pane.
pub fn terminal_shortcut(keystroke: &Keystroke) -> Option<TerminalShortcut> {
    if !keystroke.modifiers.secondary() {
        return None;
    }
    if keystroke.modifiers.shift {
        return (keystroke.key == "escape").then_some(TerminalShortcut::FocusChrome);
    }
    match keystroke.key.as_str() {
        "c" => Some(TerminalShortcut::Copy),
        "v" => Some(TerminalShortcut::Paste),
        "f" => Some(TerminalShortcut::Search),
        "k" => Some(TerminalShortcut::CommandPalette),
        "w" => Some(TerminalShortcut::CloseWorkspace),
        _ => None,
    }
}

/// Dropped paths as one line of shell words followed by a space, so the next word can be typed
/// straight after. Paths of plain characters stay readable; any other path is single-quoted.
pub fn dropped_paths_text(paths: &[std::path::PathBuf]) -> String {
    let mut text = paths
        .iter()
        .map(|path| {
            let path = path.to_string_lossy();
            if !path.is_empty()
                && path
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c))
            {
                path.into_owned()
            } else {
                format!("'{}'", path.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if !text.is_empty() {
        text.push(' ');
    }
    text
}

pub fn encode_control_char(key: &str) -> Option<u8> {
    if key.len() == 1 {
        let ch = key.as_bytes()[0];
        return match ch {
            b'a'..=b'z' => Some(ch & 0x1f),
            b'2' | b'@' => Some(0),
            b'3' | b'[' => Some(27),
            b'4' | b'\\' => Some(28),
            b'5' | b']' => Some(29),
            b'6' | b'^' => Some(30),
            b'7' | b'_' | b'/' => Some(31),
            _ => None,
        };
    }

    match key {
        "space" => Some(0),
        "enter" => Some(b'\r'),
        "backspace" => Some(0x7f),
        _ => None,
    }
}

/// Arrow keys that scroll a full-screen program without mouse reporting: one Up per line
/// toward earlier output (positive `lines`), one Down per line back. Follows the cursor-key
/// mode the program asked for.
pub fn alternate_scroll_keys(lines: i32, application_cursor: bool) -> Vec<u8> {
    cursor_key(application_cursor, if lines > 0 { b'A' } else { b'B' })
        .repeat(lines.unsigned_abs() as usize)
}

pub fn encode_mouse_report(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    kind: MouseEventKind,
    pos: TerminalCellPos,
    modifiers: Modifiers,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }

    let mut button_code = modifier_bits(modifiers);
    let terminator = match kind {
        MouseEventKind::Press => {
            if mode == MouseProtocolMode::Press
                || mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 0;
                'M'
            } else {
                return None;
            }
        }
        MouseEventKind::Release => {
            if mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 3;
                'm'
            } else {
                return None;
            }
        }
        MouseEventKind::Move { dragging } => match mode {
            MouseProtocolMode::ButtonMotion if dragging => {
                button_code += 32;
                'M'
            }
            MouseProtocolMode::AnyMotion => {
                button_code += 35;
                'M'
            }
            _ => return None,
        },
        MouseEventKind::Wheel { delta } => {
            let direction = match delta {
                ScrollDelta::Lines(delta) => delta.y,
                ScrollDelta::Pixels(delta) => {
                    let value: f32 = delta.y.into();
                    value
                }
            };
            // GPUI reports a positive delta when content should move down to reveal earlier
            // output, which is wheel up (button 4, code 64) in the xterm protocol.
            if direction > 0.0 {
                button_code += 64;
            } else if direction < 0.0 {
                button_code += 65;
            } else {
                return None;
            }
            'M'
        }
    };

    let x = pos.col as u32 + 1;
    let y = pos.row as u32 + 1;

    match encoding {
        MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{};{};{}{}", button_code, x, y, terminator).into_bytes())
        }
        MouseProtocolEncoding::Default => Some(vec![
            0x1b,
            b'[',
            b'M',
            (button_code + 32) as u8,
            (x + 32) as u8,
            (y + 32) as u8,
        ]),
        MouseProtocolEncoding::Utf8 => {
            let mut bytes = b"\x1b[M".to_vec();
            bytes.extend(char::from_u32(button_code + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(x + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(y + 32)?.to_string().into_bytes());
            Some(bytes)
        }
    }
}

pub fn modifier_bits(modifiers: Modifiers) -> u32 {
    let mut bits = 0;
    if modifiers.shift {
        bits += 4;
    }
    if modifiers.alt {
        bits += 8;
    }
    if modifiers.control {
        bits += 16;
    }
    bits
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        schema_version: u32,
        limits: Limits,
        key_cases: Vec<KeyCase>,
        paste_cases: Vec<PasteCase>,
        selection_cases: Vec<SelectionCase>,
        ime_cases: Vec<ImeCase>,
        url_cases: Vec<UrlCase>,
        mouse_cases: Vec<MouseCase>,
        scroll_cases: Vec<ScrollCase>,
        shortcut_cases: Vec<ShortcutCase>,
        drop_cases: Vec<DropCase>,
    }

    #[derive(Deserialize)]
    struct Limits {
        max_paste_bytes: usize,
        paste_confirmation_bytes: usize,
        max_url_bytes: usize,
        max_urls: usize,
    }

    #[derive(Deserialize)]
    struct KeyCase {
        name: String,
        key: String,
        text: Option<String>,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        control: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        application_cursor: bool,
        expected: Vec<u8>,
    }

    #[derive(Deserialize)]
    struct PasteCase {
        name: String,
        input: Option<String>,
        input_repeat: Option<Repeat>,
        bracketed: bool,
        requires_confirmation: bool,
        expected: Option<Vec<u8>>,
    }

    #[derive(Deserialize)]
    struct Repeat {
        value: String,
        count: usize,
    }

    #[derive(Deserialize)]
    struct SelectionCase {
        name: String,
        rows: Vec<Vec<Cell>>,
        start: Point,
        end: Point,
        expected: String,
    }

    #[derive(Deserialize)]
    struct Cell {
        text: String,
        width: usize,
    }

    #[derive(Deserialize)]
    struct Point {
        row: usize,
        column: usize,
    }

    #[derive(Deserialize)]
    struct ImeCase {
        name: String,
        operations: Vec<ImeOperation>,
        expected_emissions: Vec<Vec<u8>>,
    }

    #[derive(Deserialize)]
    struct ImeOperation {
        kind: String,
        text: Option<String>,
    }

    #[derive(Deserialize)]
    struct UrlCase {
        name: String,
        text: String,
        expected: Vec<String>,
    }

    #[derive(Deserialize)]
    struct MouseCase {
        name: String,
        mode: String,
        encoding: String,
        kind: String,
        row: u16,
        col: u16,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        control: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        dragging: bool,
        #[serde(default)]
        wheel_lines: f32,
        expected: Option<Vec<u8>>,
    }

    #[derive(Deserialize)]
    struct ScrollCase {
        name: String,
        lines: i32,
        application_cursor: bool,
        expected: Vec<u8>,
    }

    #[derive(Deserialize)]
    struct ShortcutCase {
        name: String,
        scope: String,
        keystroke: String,
        expected: Option<String>,
    }

    #[derive(Deserialize)]
    struct DropCase {
        name: String,
        paths: Vec<String>,
        expected: String,
    }

    fn shortcut_name(shortcut: AppShortcut) -> String {
        match shortcut {
            AppShortcut::CommandPalette => "command_palette".to_owned(),
            AppShortcut::CycleWorkspace { forward: true } => "cycle_workspace_forward".to_owned(),
            AppShortcut::CycleWorkspace { forward: false } => "cycle_workspace_back".to_owned(),
            AppShortcut::OpenFiles => "open_files".to_owned(),
            AppShortcut::ToggleFilesAndTerminal => "toggle_files_and_terminal".to_owned(),
            AppShortcut::BroadcastInput => "broadcast_input".to_owned(),
            AppShortcut::ClearScreen => "clear_screen".to_owned(),
            AppShortcut::LocalTerminal => "local_terminal".to_owned(),
            AppShortcut::NavigateSection(number) => format!("navigate_section_{number}"),
            AppShortcut::Settings => "settings".to_owned(),
            AppShortcut::LogsOrHostSearch => "logs_or_host_search".to_owned(),
            AppShortcut::NewHostOrSession => "new_host_or_session".to_owned(),
            AppShortcut::DuplicatePane => "duplicate_pane".to_owned(),
        }
    }

    fn terminal_shortcut_name(shortcut: TerminalShortcut) -> &'static str {
        match shortcut {
            TerminalShortcut::FocusChrome => "focus_chrome",
            TerminalShortcut::Copy => "copy",
            TerminalShortcut::Paste => "paste",
            TerminalShortcut::Search => "search",
            TerminalShortcut::CommandPalette => "command_palette",
            TerminalShortcut::CloseWorkspace => "close_workspace",
        }
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/terminal/terminal-interaction-v1.json"
        ))
        .expect("terminal interaction fixture must parse")
    }

    #[test]
    fn terminal_interaction_conformance_keys_paste_and_limits() {
        let fixture = fixture();
        assert_eq!(fixture.schema_version, 1);
        assert_eq!(fixture.limits.max_paste_bytes, 256 * 1024);
        for case in fixture.key_cases {
            let actual = encode_terminal_key(
                &case.key,
                case.text.as_deref(),
                TerminalModifiers {
                    shift: case.shift,
                    control: case.control,
                    alt: case.alt,
                },
                case.application_cursor,
            );
            assert_eq!(actual, Some(case.expected), "{}", case.name);
        }
        for case in fixture.paste_cases {
            let input = match (case.input, case.input_repeat) {
                (Some(input), None) => input,
                (None, Some(repeat)) => repeat.value.repeat(repeat.count),
                _ => panic!("{} has invalid input", case.name),
            };
            let normalized = normalize_terminal_paste(&input);
            assert!(normalized.len() <= fixture.limits.max_paste_bytes);
            assert_eq!(
                terminal_paste_requires_confirmation(
                    &normalized,
                    fixture.limits.paste_confirmation_bytes
                ),
                case.requires_confirmation,
                "{}",
                case.name
            );
            if let Some(expected) = case.expected {
                assert_eq!(
                    wrap_terminal_paste(&normalized, case.bracketed),
                    expected,
                    "{}",
                    case.name
                );
            }
        }
    }

    #[test]
    fn terminal_interaction_conformance_selection_ime_and_urls() {
        let fixture = fixture();
        for case in fixture.selection_cases {
            let rows = case
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|cell| TerminalSelectionCell {
                            text: cell.text,
                            width: cell.width,
                        })
                        .collect()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                extract_terminal_selection(
                    &rows,
                    TerminalSelectionPoint {
                        row: case.start.row,
                        column: case.start.column,
                    },
                    TerminalSelectionPoint {
                        row: case.end.row,
                        column: case.end.column,
                    },
                ),
                case.expected,
                "{}",
                case.name
            );
        }
        for case in fixture.ime_cases {
            let mut state = TerminalImeState::default();
            let mut emissions = Vec::new();
            for operation in case.operations {
                match operation.kind.as_str() {
                    "update" => state.update(operation.text.as_deref().unwrap_or_default()),
                    "cancel" => state.cancel(),
                    "commit" => {
                        if let Some(bytes) =
                            state.commit(operation.text.as_deref().unwrap_or_default())
                        {
                            emissions.push(bytes);
                        }
                    }
                    "finish" => {
                        if let Some(bytes) = state.finish() {
                            emissions.push(bytes);
                        }
                    }
                    kind => panic!("{} has unknown IME operation {kind}", case.name),
                }
            }
            assert_eq!(emissions, case.expected_emissions, "{}", case.name);
        }
        for case in fixture.url_cases {
            assert_eq!(
                visible_http_urls(
                    &case.text,
                    fixture.limits.max_url_bytes,
                    fixture.limits.max_urls,
                ),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn terminal_interaction_conformance_mouse_and_scroll() {
        for case in fixture().mouse_cases {
            let mode = match case.mode.as_str() {
                "none" => MouseProtocolMode::None,
                "press" => MouseProtocolMode::Press,
                "press_release" => MouseProtocolMode::PressRelease,
                "button_motion" => MouseProtocolMode::ButtonMotion,
                "any_motion" => MouseProtocolMode::AnyMotion,
                other => panic!("{}: unknown mouse mode {other}", case.name),
            };
            let encoding = match case.encoding.as_str() {
                "default" => MouseProtocolEncoding::Default,
                "utf8" => MouseProtocolEncoding::Utf8,
                "sgr" => MouseProtocolEncoding::Sgr,
                other => panic!("{}: unknown mouse encoding {other}", case.name),
            };
            let kind = match case.kind.as_str() {
                "press" => MouseEventKind::Press,
                "release" => MouseEventKind::Release,
                "move" => MouseEventKind::Move {
                    dragging: case.dragging,
                },
                "wheel" => MouseEventKind::Wheel {
                    delta: ScrollDelta::Lines(gpui::point(0.0, case.wheel_lines)),
                },
                other => panic!("{}: unknown mouse event {other}", case.name),
            };
            let actual = encode_mouse_report(
                mode,
                encoding,
                kind,
                TerminalCellPos {
                    row: case.row,
                    col: case.col,
                },
                Modifiers {
                    shift: case.shift,
                    control: case.control,
                    alt: case.alt,
                    ..Modifiers::none()
                },
            );
            assert_eq!(actual, case.expected, "{}", case.name);
        }

        for case in fixture().scroll_cases {
            assert_eq!(
                alternate_scroll_keys(case.lines, case.application_cursor),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    /// The shortcuts are held with the platform's primary modifier, so this runs the same table
    /// on macOS, Linux and Windows.
    #[test]
    fn terminal_interaction_conformance_shortcuts_and_drops() {
        for case in fixture().shortcut_cases {
            let keystroke = Keystroke::parse(&case.keystroke)
                .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
            let actual = match case.scope.as_str() {
                "app" => app_shortcut(&keystroke).map(shortcut_name),
                "terminal" => terminal_shortcut(&keystroke)
                    .map(terminal_shortcut_name)
                    .map(str::to_owned),
                other => panic!("{}: unknown scope {other}", case.name),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }

        for case in fixture().drop_cases {
            let paths = case
                .paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>();
            assert_eq!(dropped_paths_text(&paths), case.expected, "{}", case.name);
        }
    }

    #[test]
    fn wheel_toward_earlier_output_reports_wheel_up() {
        let report = |y: f32| {
            encode_mouse_report(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
                MouseEventKind::Wheel {
                    delta: ScrollDelta::Lines(gpui::point(0.0, y)),
                },
                TerminalCellPos { row: 4, col: 9 },
                Modifiers::none(),
            )
        };
        assert_eq!(report(1.0), Some(b"\x1b[<64;10;5M".to_vec()));
        assert_eq!(report(-1.0), Some(b"\x1b[<65;10;5M".to_vec()));
        assert_eq!(report(0.0), None);
    }

    #[test]
    fn alternate_screen_wheel_sends_one_arrow_per_line_in_the_cursor_key_mode() {
        assert_eq!(alternate_scroll_keys(2, false), b"\x1b[A\x1b[A".to_vec());
        assert_eq!(alternate_scroll_keys(-1, false), b"\x1b[B".to_vec());
        assert_eq!(alternate_scroll_keys(1, true), b"\x1bOA".to_vec());
        assert_eq!(
            alternate_scroll_keys(-3, true),
            b"\x1bOB\x1bOB\x1bOB".to_vec()
        );
    }
}
