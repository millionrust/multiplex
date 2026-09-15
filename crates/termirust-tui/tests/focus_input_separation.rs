use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termirust_tui::{InputDecision, TerminalInputModel};

#[test]
fn management_shortcuts_are_literal_input_while_terminal_has_focus() {
    let mut input = TerminalInputModel::default();
    input.set_lease(true);

    for shortcut in ['n', 'c', 'e', 'p', 'm', 's', 'a', 'x', 'd', 'f'] {
        assert_eq!(
            input.handle_key(
                KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE),
                Instant::now(),
            ),
            InputDecision::Send(vec![shortcut as u8]),
            "{shortcut} must reach the attached PTY instead of opening management UI"
        );
    }
}
