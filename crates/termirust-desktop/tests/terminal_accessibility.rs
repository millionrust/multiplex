use std::time::{Duration, Instant};

use termirust_ui_contract::{
    MAX_TERMINAL_ACCESSIBILITY_BYTES, MAX_TERMINAL_ACCESSIBILITY_LINES, MessageId, SemanticText,
    ShellSemanticSnapshot, TERMINAL_ANNOUNCEMENT_INTERVAL, TerminalAccessibilityBuffer,
    TerminalAccessibilityCommand, TerminalAnnouncement, TerminalAnnouncementCoalescer,
    TerminalFocusMode, TerminalLifecycle, TerminalSemanticSnapshot,
};

fn terminal_snapshot(
    buffer: &TerminalAccessibilityBuffer,
    focus_mode: TerminalFocusMode,
    input_authorized: bool,
    recording_friendly: bool,
) -> TerminalSemanticSnapshot {
    TerminalSemanticSnapshot {
        terminal: buffer.snapshot(),
        focus_mode,
        input_authorized,
        recording_friendly,
        announcement: None,
    }
}

#[test]
fn bounded_review_routes_escape_and_requires_current_input_authorization() {
    let mut buffer = TerminalAccessibilityBuffer::new(41, "Production shell");
    for index in 0..=MAX_TERMINAL_ACCESSIBILITY_LINES {
        buffer.append(
            format!("logical line {index}\n").as_bytes(),
            Some(index as u64 + 1),
        );
    }
    let snapshot = terminal_snapshot(&buffer, TerminalFocusMode::AccessibleReview, false, false);
    let shell = ShellSemanticSnapshot {
        terminal: Some(snapshot.clone()),
        ..ShellSemanticSnapshot::default()
    };
    let tree = shell.try_tree().expect("terminal semantic tree");
    shell.try_router(&tree).expect("terminal shell routes");
    let routes = snapshot.routes();
    assert!(
        routes
            .iter()
            .any(|(_, command)| { *command == TerminalAccessibilityCommand::ExitToChrome })
    );
    assert!(
        routes
            .iter()
            .any(|(_, command)| { *command == TerminalAccessibilityCommand::EnterInput })
    );
    assert!(buffer.retained_lines() <= MAX_TERMINAL_ACCESSIBILITY_LINES);
    assert!(buffer.retained_bytes() <= MAX_TERMINAL_ACCESSIBILITY_BYTES);
}

#[test]
fn fragmented_ansi_unicode_alternate_screen_and_overflow_remain_bounded() {
    let mut buffer = TerminalAccessibilityBuffer::new(42, "Fixture");
    let fixture = b"before\x1b[?1049h\x1b[2J\x1b[Hafter \xf0\x9f\x99\x82\x1b[?1049l\n";
    for fragment in fixture.chunks(3) {
        buffer.append(fragment, None);
    }
    let before_overflow = buffer.snapshot();
    assert!(before_overflow.text.contains("beforeafter 🙂"));
    buffer.append(&vec![b'x'; MAX_TERMINAL_ACCESSIBILITY_BYTES * 3], None);
    let snapshot = buffer.snapshot();
    assert!(!snapshot.text.contains('\x1b'));
    assert_eq!(snapshot.text.len(), MAX_TERMINAL_ACCESSIBILITY_BYTES);
    assert!(snapshot.truncated);
}

#[test]
fn lifecycle_announcements_are_content_free_coalesced_and_rate_limited() {
    let canary = "SECRET-TOKEN-MUST-NOT-BE-ANNOUNCED";
    let mut buffer = TerminalAccessibilityBuffer::new(43, "Canary");
    buffer.append(canary.as_bytes(), Some(7));
    let mut announcements = TerminalAnnouncementCoalescer::new();
    let start = Instant::now();
    announcements.observe_output(canary.len());
    let first = announcements
        .flush(start)
        .expect("first output announcement");
    assert_eq!(
        first,
        TerminalAnnouncement::OutputAvailable {
            bytes: canary.len()
        }
    );
    assert!(!format!("{first:?}").contains(canary));
    announcements.observe_output(canary.len());
    assert_eq!(announcements.flush(start + Duration::from_millis(1)), None);
    assert!(
        announcements
            .flush(start + TERMINAL_ANNOUNCEMENT_INTERVAL)
            .is_some()
    );
    assert_eq!(
        buffer.set_lifecycle(TerminalLifecycle::Gap),
        Some(TerminalAnnouncement::Gap)
    );
}

#[test]
fn revoke_and_privacy_clear_remove_secret_output_from_semantics() {
    let canary = "SECRET-OUTPUT-CANARY";
    let mut buffer = TerminalAccessibilityBuffer::new(44, "Sensitive host");
    buffer.append(canary.as_bytes(), Some(1));

    let recording = terminal_snapshot(&buffer, TerminalFocusMode::AccessibleReview, false, true);
    let nodes = recording.try_tree().expect("recording-safe semantics");
    let rendered = format!("{:?}", nodes.nodes());
    assert!(!rendered.contains(canary));
    assert!(!rendered.contains("Sensitive host"));
    assert!(
        nodes
            .nodes()
            .values()
            .any(|node| { node.name == Some(SemanticText::Message(MessageId::TerminalPane)) })
    );

    buffer.clear_sensitive_content();
    buffer.set_lifecycle(TerminalLifecycle::PermissionDenied);
    let revoked = terminal_snapshot(&buffer, TerminalFocusMode::Chrome, false, false);
    assert!(revoked.terminal.text.is_empty());
    assert_eq!(revoked.terminal.read_cursor, 0);
}
