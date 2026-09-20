use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticActionRouter, SemanticError,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticTree,
    SemanticValue,
};

pub const MAX_TERMINAL_ACCESSIBILITY_BYTES: usize = 64 * 1024;
pub const MAX_TERMINAL_ACCESSIBILITY_LINES: usize = 2_000;
pub const TERMINAL_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const TERMINAL_ROOT_NODE: u64 = 220_000;
const TERMINAL_TITLE_NODE: u64 = 220_001;
const TERMINAL_STATE_NODE: u64 = 220_002;
const TERMINAL_INPUT_NODE: u64 = 220_003;
const TERMINAL_REVIEW_NODE: u64 = 220_004;
const TERMINAL_EXIT_NODE: u64 = 220_005;
const TERMINAL_PREVIOUS_NODE: u64 = 220_006;
const TERMINAL_NEXT_NODE: u64 = 220_007;
const TERMINAL_CURSOR_NODE: u64 = 220_008;
const TERMINAL_ANNOUNCEMENT_NODE: u64 = 220_009;
const TERMINAL_TEXT_NODE_BASE: u64 = 221_000;
const MAX_SEMANTIC_CHUNK_CHARS: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSequenceRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalLifecycle {
    Connecting,
    Live,
    Replaying,
    Gap,
    Offline,
    Backpressured,
    Error,
    Detached,
    PermissionDenied,
    Recovery,
    Exited,
}

impl TerminalLifecycle {
    pub const ALL: [Self; 11] = [
        Self::Connecting,
        Self::Live,
        Self::Replaying,
        Self::Gap,
        Self::Offline,
        Self::Backpressured,
        Self::Error,
        Self::Detached,
        Self::PermissionDenied,
        Self::Recovery,
        Self::Exited,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Connecting => MessageId::TerminalStateConnecting,
            Self::Live => MessageId::TerminalStateLive,
            Self::Replaying => MessageId::TerminalStateReplaying,
            Self::Gap => MessageId::TerminalStateGap,
            Self::Offline => MessageId::TerminalStateOffline,
            Self::Backpressured => MessageId::TerminalStateBackpressured,
            Self::Error => MessageId::TerminalStateError,
            Self::Detached => MessageId::TerminalStateDetached,
            Self::PermissionDenied => MessageId::TerminalStatePermissionDenied,
            Self::Recovery => MessageId::TerminalStateRecovery,
            Self::Exited => MessageId::TerminalStateExited,
        }
    }

    const fn is_error(self) -> bool {
        matches!(
            self,
            Self::Gap | Self::Offline | Self::Backpressured | Self::Error | Self::PermissionDenied
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalFocusMode {
    Chrome,
    Input,
    AccessibleReview,
}

impl TerminalFocusMode {
    pub const fn accepts_input(self, authorized: bool) -> bool {
        matches!(self, Self::Input) && authorized
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalAnnouncement {
    Lifecycle(TerminalLifecycle),
    Attention,
    Gap,
    Truncated,
    OutputAvailable { bytes: usize },
}

impl TerminalAnnouncement {
    const fn message(self) -> MessageId {
        match self {
            Self::Lifecycle(lifecycle) => lifecycle.message(),
            Self::Attention => MessageId::TerminalAnnouncementAttention,
            Self::Gap => MessageId::TerminalAnnouncementGap,
            Self::Truncated => MessageId::TerminalAnnouncementTruncated,
            Self::OutputAvailable { .. } => MessageId::TerminalAnnouncementOutput,
        }
    }

    const fn immediate(self) -> bool {
        matches!(self, Self::Attention | Self::Gap)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalAccessibilitySnapshot {
    pub session_id: u64,
    pub title: String,
    pub lifecycle: TerminalLifecycle,
    pub sequence_range: Option<TerminalSequenceRange>,
    pub text: String,
    pub truncated: bool,
    pub read_cursor: usize,
}

#[derive(Clone, Debug)]
struct AccessibleLine {
    text: String,
    start: usize,
    sequence: u64,
}

impl AccessibleLine {
    fn new(sequence: u64) -> Self {
        Self {
            text: String::new(),
            start: 0,
            sequence,
        }
    }

    fn visible(&self) -> &str {
        &self.text[self.start..]
    }

    fn visible_len(&self) -> usize {
        self.text.len().saturating_sub(self.start)
    }

    fn compact(&mut self) {
        if self.start >= MAX_TERMINAL_ACCESSIBILITY_BYTES
            || self.start.saturating_mul(2) >= self.text.len()
        {
            self.text.drain(..self.start);
            self.start = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeParserState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    StringControl,
    StringControlEscape,
}

#[derive(Clone, Debug)]
pub struct TerminalAccessibilityBuffer {
    session_id: u64,
    title: String,
    lifecycle: TerminalLifecycle,
    lines: VecDeque<AccessibleLine>,
    retained_bytes: usize,
    next_sequence: u64,
    truncated: bool,
    read_cursor: usize,
    parser_state: EscapeParserState,
    utf8_pending: Vec<u8>,
}

impl TerminalAccessibilityBuffer {
    pub fn new(session_id: u64, title: impl Into<String>) -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(AccessibleLine::new(0));
        Self {
            session_id,
            title: title.into(),
            lifecycle: TerminalLifecycle::Connecting,
            lines,
            retained_bytes: 0,
            next_sequence: 0,
            truncated: false,
            read_cursor: 0,
            parser_state: EscapeParserState::Ground,
            utf8_pending: Vec::with_capacity(4),
        }
    }

    pub fn append(&mut self, bytes: &[u8], sequence: Option<u64>) {
        let sequence = sequence.unwrap_or_else(|| {
            self.next_sequence = self.next_sequence.saturating_add(1);
            self.next_sequence
        });
        self.next_sequence = self.next_sequence.max(sequence);

        for chunk in bytes.chunks(8 * 1024) {
            for &byte in chunk {
                self.process_byte(byte, sequence);
            }
            self.enforce_bounds();
        }
        self.enforce_bounds();
        self.read_cursor = self.read_cursor.min(self.lines.len().saturating_sub(1));
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn set_lifecycle(&mut self, lifecycle: TerminalLifecycle) -> Option<TerminalAnnouncement> {
        if self.lifecycle == lifecycle {
            return None;
        }
        self.lifecycle = lifecycle;
        Some(match lifecycle {
            TerminalLifecycle::Gap => TerminalAnnouncement::Gap,
            TerminalLifecycle::Backpressured
            | TerminalLifecycle::Error
            | TerminalLifecycle::PermissionDenied => TerminalAnnouncement::Attention,
            _ => TerminalAnnouncement::Lifecycle(lifecycle),
        })
    }

    pub fn move_read_cursor(&mut self, delta: isize) -> usize {
        let maximum = self.lines.len().saturating_sub(1);
        self.read_cursor = if delta.is_negative() {
            self.read_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.read_cursor.saturating_add(delta as usize).min(maximum)
        };
        self.read_cursor
    }

    pub fn move_read_cursor_to_end(&mut self) -> usize {
        self.read_cursor = self.lines.len().saturating_sub(1);
        self.read_cursor
    }

    pub fn clear_sensitive_content(&mut self) {
        self.lines.clear();
        self.lines
            .push_back(AccessibleLine::new(self.next_sequence));
        self.retained_bytes = 0;
        self.truncated = false;
        self.read_cursor = 0;
        self.parser_state = EscapeParserState::Ground;
        self.utf8_pending.clear();
    }

    pub fn snapshot(&self) -> TerminalAccessibilitySnapshot {
        let mut text = String::with_capacity(self.retained_bytes);
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            text.push_str(line.visible());
        }
        let sequence_range = self.lines.front().and_then(|first| {
            (self.next_sequence > 0 && self.retained_bytes > 0).then_some(TerminalSequenceRange {
                start: first.sequence,
                end: self.next_sequence,
            })
        });
        TerminalAccessibilitySnapshot {
            session_id: self.session_id,
            title: self.title.clone(),
            lifecycle: self.lifecycle,
            sequence_range,
            text,
            truncated: self.truncated,
            read_cursor: self.read_cursor,
        }
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn retained_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    fn process_byte(&mut self, byte: u8, sequence: u64) {
        match self.parser_state {
            EscapeParserState::Ground => self.process_ground_byte(byte, sequence),
            EscapeParserState::Escape => {
                self.flush_invalid_utf8(sequence);
                self.parser_state = match byte {
                    b'[' => EscapeParserState::Csi,
                    b']' => EscapeParserState::Osc,
                    b'P' | b'^' | b'_' => EscapeParserState::StringControl,
                    0x1b => EscapeParserState::Escape,
                    _ => EscapeParserState::Ground,
                };
            }
            EscapeParserState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.parser_state = EscapeParserState::Ground;
                } else if byte == 0x1b {
                    self.parser_state = EscapeParserState::Escape;
                }
            }
            EscapeParserState::Osc => match byte {
                0x07 => self.parser_state = EscapeParserState::Ground,
                0x1b => self.parser_state = EscapeParserState::OscEscape,
                _ => {}
            },
            EscapeParserState::OscEscape => {
                self.parser_state = if byte == b'\\' {
                    EscapeParserState::Ground
                } else if byte == 0x1b {
                    EscapeParserState::OscEscape
                } else {
                    EscapeParserState::Osc
                };
            }
            EscapeParserState::StringControl => {
                if byte == 0x1b {
                    self.parser_state = EscapeParserState::StringControlEscape;
                }
            }
            EscapeParserState::StringControlEscape => {
                self.parser_state = if byte == b'\\' {
                    EscapeParserState::Ground
                } else if byte == 0x1b {
                    EscapeParserState::StringControlEscape
                } else {
                    EscapeParserState::StringControl
                };
            }
        }
    }

    fn process_ground_byte(&mut self, byte: u8, sequence: u64) {
        if byte == 0x1b {
            self.flush_invalid_utf8(sequence);
            self.parser_state = EscapeParserState::Escape;
            return;
        }
        if byte >= 0x80 || !self.utf8_pending.is_empty() {
            self.process_utf8_byte(byte, sequence);
            return;
        }
        match byte {
            b'\n' => self.push_newline(sequence),
            b'\r' => {}
            b'\t' => self.push_char('\t', sequence),
            0x08 => self.pop_char(),
            0x20..=0x7e => self.push_char(char::from(byte), sequence),
            _ => {}
        }
    }

    fn process_utf8_byte(&mut self, byte: u8, sequence: u64) {
        if byte < 0x80 && !self.utf8_pending.is_empty() {
            self.flush_invalid_utf8(sequence);
            self.process_ground_byte(byte, sequence);
            return;
        }
        self.utf8_pending.push(byte);
        match std::str::from_utf8(&self.utf8_pending) {
            Ok(value) => {
                let characters = value.chars().collect::<Vec<_>>();
                self.utf8_pending.clear();
                for character in characters {
                    self.push_char(character, sequence);
                }
            }
            Err(error) if error.error_len().is_some() || self.utf8_pending.len() >= 4 => {
                self.flush_invalid_utf8(sequence);
            }
            Err(_) => {}
        }
    }

    fn flush_invalid_utf8(&mut self, sequence: u64) {
        if !self.utf8_pending.is_empty() {
            self.utf8_pending.clear();
            self.push_char('\u{fffd}', sequence);
        }
    }

    fn push_char(&mut self, character: char, sequence: u64) {
        let line = self
            .lines
            .back_mut()
            .expect("terminal accessibility buffer always has a current line");
        if line.visible().is_empty() {
            line.sequence = sequence;
        }
        line.text.push(character);
        self.retained_bytes = self.retained_bytes.saturating_add(character.len_utf8());
    }

    fn pop_char(&mut self) {
        let Some(line) = self.lines.back_mut() else {
            return;
        };
        let Some(character) = line.visible().chars().next_back() else {
            return;
        };
        let next_len = line.text.len().saturating_sub(character.len_utf8());
        line.text.truncate(next_len);
        self.retained_bytes = self.retained_bytes.saturating_sub(character.len_utf8());
    }

    fn push_newline(&mut self, sequence: u64) {
        self.lines.push_back(AccessibleLine::new(sequence));
        self.retained_bytes = self.retained_bytes.saturating_add(1);
    }

    fn enforce_bounds(&mut self) {
        while self.lines.len() > MAX_TERMINAL_ACCESSIBILITY_LINES
            || (self.retained_bytes > MAX_TERMINAL_ACCESSIBILITY_BYTES && self.lines.len() > 1)
        {
            let removed = self.lines.pop_front().expect("line bound checked");
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(removed.visible_len().saturating_add(1));
            self.truncated = true;
            self.read_cursor = self.read_cursor.saturating_sub(1);
        }

        if self.retained_bytes > MAX_TERMINAL_ACCESSIBILITY_BYTES
            && let Some(line) = self.lines.front_mut()
        {
            let overflow = self.retained_bytes - MAX_TERMINAL_ACCESSIBILITY_BYTES;
            let visible = line.visible();
            let mut removed = overflow.min(visible.len());
            while removed < visible.len() && !visible.is_char_boundary(removed) {
                removed += 1;
            }
            line.start = line.start.saturating_add(removed);
            line.sequence = self.next_sequence;
            line.compact();
            self.retained_bytes = self.retained_bytes.saturating_sub(removed);
            self.truncated = true;
        }
    }
}

#[derive(Clone, Debug)]
pub struct TerminalAnnouncementCoalescer {
    last_announcement: Option<Instant>,
    pending_output_bytes: usize,
    truncation_announced: bool,
}

impl TerminalAnnouncementCoalescer {
    pub fn new() -> Self {
        Self {
            last_announcement: None,
            pending_output_bytes: 0,
            truncation_announced: false,
        }
    }

    pub fn observe_output(&mut self, bytes: usize) {
        self.pending_output_bytes = self.pending_output_bytes.saturating_add(bytes);
    }

    pub fn observe_truncation(&mut self, truncated: bool) -> Option<TerminalAnnouncement> {
        if truncated && !self.truncation_announced {
            self.truncation_announced = true;
            Some(TerminalAnnouncement::Truncated)
        } else {
            None
        }
    }

    pub fn flush(&mut self, now: Instant) -> Option<TerminalAnnouncement> {
        if self.pending_output_bytes == 0
            || self.last_announcement.is_some_and(|last| {
                now.saturating_duration_since(last) < TERMINAL_ANNOUNCEMENT_INTERVAL
            })
        {
            return None;
        }
        let bytes = std::mem::take(&mut self.pending_output_bytes);
        self.last_announcement = Some(now);
        Some(TerminalAnnouncement::OutputAvailable { bytes })
    }

    pub fn clear(&mut self) {
        self.pending_output_bytes = 0;
        self.truncation_announced = false;
    }
}

impl Default for TerminalAnnouncementCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalAccessibilityCommand {
    FocusChrome,
    EnterInput,
    EnterReview,
    ExitToChrome,
    PreviousReviewLine,
    NextReviewLine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSemanticSnapshot {
    pub terminal: TerminalAccessibilitySnapshot,
    pub focus_mode: TerminalFocusMode,
    pub input_authorized: bool,
    pub recording_friendly: bool,
    pub announcement: Option<TerminalAnnouncement>,
}

impl TerminalSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        let root = semantic_id(TERMINAL_ROOT_NODE);
        let mut root_node = named_node(
            TERMINAL_ROOT_NODE,
            SemanticRole::Group,
            MessageId::TerminalPane,
        );
        root_node.parent = Some(parent);
        root_node.state.selected = self.focus_mode == TerminalFocusMode::Chrome;
        root_node.actions.insert(SemanticAction::Focus);

        let mut title = SemanticNode::new(semantic_id(TERMINAL_TITLE_NODE), SemanticRole::Heading);
        title.parent = Some(root);
        title.name = Some(if self.recording_friendly {
            SemanticText::Message(MessageId::TerminalPane)
        } else {
            bounded_user_text(&self.terminal.title)?
        });

        let mut state = named_node(
            TERMINAL_STATE_NODE,
            if self.terminal.lifecycle.is_error() {
                SemanticRole::Alert
            } else {
                SemanticRole::Status
            },
            self.terminal.lifecycle.message(),
        );
        state.parent = Some(root);
        state.state.live = Some(if self.terminal.lifecycle.is_error() {
            LiveRegionPoliteness::Immediate
        } else {
            LiveRegionPoliteness::Polite
        });
        state.state.busy = matches!(
            self.terminal.lifecycle,
            TerminalLifecycle::Connecting
                | TerminalLifecycle::Replaying
                | TerminalLifecycle::Recovery
        );

        let mut input = named_node(
            TERMINAL_INPUT_NODE,
            SemanticRole::Button,
            MessageId::TerminalInputAction,
        );
        input.parent = Some(root);
        input.state.disabled = !self.input_authorized;
        input.state.selected = self.focus_mode == TerminalFocusMode::Input;
        input.actions.insert(SemanticAction::Activate);

        let mut review = named_node(
            TERMINAL_REVIEW_NODE,
            SemanticRole::Button,
            MessageId::TerminalReviewAction,
        );
        review.parent = Some(root);
        review.state.selected = self.focus_mode == TerminalFocusMode::AccessibleReview;
        review.actions.insert(SemanticAction::Activate);

        let mut exit = named_node(
            TERMINAL_EXIT_NODE,
            SemanticRole::Button,
            MessageId::TerminalExitAction,
        );
        exit.parent = Some(root);
        exit.state.disabled = self.focus_mode == TerminalFocusMode::Chrome;
        exit.actions.insert(SemanticAction::Activate);

        let mut nodes = vec![root_node, title, state, input, review, exit];
        if let Some(announcement) = self.announcement {
            let mut announcement_node = named_node(
                TERMINAL_ANNOUNCEMENT_NODE,
                if announcement.immediate() {
                    SemanticRole::Alert
                } else {
                    SemanticRole::Status
                },
                announcement.message(),
            );
            announcement_node.parent = Some(root);
            announcement_node.state.live = Some(if announcement.immediate() {
                LiveRegionPoliteness::Immediate
            } else {
                LiveRegionPoliteness::Polite
            });
            nodes.push(announcement_node);
        }
        if self.focus_mode == TerminalFocusMode::AccessibleReview && !self.recording_friendly {
            let mut previous = named_node(
                TERMINAL_PREVIOUS_NODE,
                SemanticRole::Button,
                MessageId::TerminalReviewPrevious,
            );
            previous.parent = Some(root);
            previous.state.disabled = self.terminal.read_cursor == 0;
            previous.actions.insert(SemanticAction::Activate);
            nodes.push(previous);

            let line_count = logical_line_count(&self.terminal.text);
            let mut next = named_node(
                TERMINAL_NEXT_NODE,
                SemanticRole::Button,
                MessageId::TerminalReviewNext,
            );
            next.parent = Some(root);
            next.state.disabled = self.terminal.read_cursor.saturating_add(1) >= line_count;
            next.actions.insert(SemanticAction::Activate);
            nodes.push(next);

            let mut cursor = named_node(
                TERMINAL_CURSOR_NODE,
                SemanticRole::Status,
                MessageId::TerminalReviewCursor,
            );
            cursor.parent = Some(root);
            cursor.value = Some(SemanticValue::Number {
                current: self.terminal.read_cursor.saturating_add(1) as i64,
                minimum: 1,
                maximum: line_count as i64,
            });
            nodes.push(cursor);

            for (index, chunk) in semantic_text_chunks(&self.terminal.text)
                .into_iter()
                .enumerate()
            {
                let mut text = SemanticNode::new(
                    semantic_id(TERMINAL_TEXT_NODE_BASE + index as u64),
                    SemanticRole::StaticText,
                );
                text.parent = Some(root);
                text.name = Some(SemanticText::user_text(chunk)?);
                nodes.push(text);
            }
        }
        Ok(nodes)
    }

    pub fn routes(
        &self,
    ) -> Vec<(
        (SemanticNodeId, SemanticAction),
        TerminalAccessibilityCommand,
    )> {
        let mut routes = vec![
            (
                (semantic_id(TERMINAL_ROOT_NODE), SemanticAction::Focus),
                TerminalAccessibilityCommand::FocusChrome,
            ),
            (
                (semantic_id(TERMINAL_INPUT_NODE), SemanticAction::Activate),
                TerminalAccessibilityCommand::EnterInput,
            ),
            (
                (semantic_id(TERMINAL_REVIEW_NODE), SemanticAction::Activate),
                TerminalAccessibilityCommand::EnterReview,
            ),
            (
                (semantic_id(TERMINAL_EXIT_NODE), SemanticAction::Activate),
                TerminalAccessibilityCommand::ExitToChrome,
            ),
        ];
        if self.focus_mode == TerminalFocusMode::AccessibleReview {
            routes.extend([
                (
                    (
                        semantic_id(TERMINAL_PREVIOUS_NODE),
                        SemanticAction::Activate,
                    ),
                    TerminalAccessibilityCommand::PreviousReviewLine,
                ),
                (
                    (semantic_id(TERMINAL_NEXT_NODE), SemanticAction::Activate),
                    TerminalAccessibilityCommand::NextReviewLine,
                ),
            ]);
        }
        routes
    }

    pub fn try_tree(&self) -> Result<SemanticTree, SemanticError> {
        let root = semantic_id(TERMINAL_ROOT_NODE);
        SemanticTree::try_new(
            1,
            root,
            self.try_nodes(root)?.into_iter().map(|mut node| {
                if node.id == root {
                    node.parent = None;
                }
                node
            }),
        )
    }

    pub fn try_router(
        &self,
        tree: &SemanticTree,
    ) -> Result<SemanticActionRouter<TerminalAccessibilityCommand>, SemanticError> {
        SemanticActionRouter::try_new(tree, self.routes())
    }
}

fn semantic_text_chunks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut chunk_chars = 0_usize;
    for character in text.chars() {
        if chunk_chars == MAX_SEMANTIC_CHUNK_CHARS {
            chunks.push(std::mem::take(&mut chunk));
            chunk_chars = 0;
        }
        chunk.push(if character.is_control() {
            ' '
        } else {
            character
        });
        chunk_chars += 1;
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

pub fn terminal_semantic_chunk_count(text: &str) -> usize {
    text.chars().count().div_ceil(MAX_SEMANTIC_CHUNK_CHARS)
}

fn logical_line_count(text: &str) -> usize {
    if text.is_empty() {
        1
    } else {
        text.split('\n').count()
    }
}

fn bounded_user_text(text: &str) -> Result<SemanticText, SemanticError> {
    SemanticText::user_text(
        text.chars()
            .take(MAX_SEMANTIC_CHUNK_CHARS)
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>(),
    )
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("terminal semantic IDs are non-zero"))
}

fn named_node(id: u64, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(semantic_id(id), role);
    node.name = Some(SemanticText::Message(name));
    node.state = SemanticState::default();
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_accessibility_bounds_lines_bytes_and_strips_escape_sequences() {
        let mut buffer = TerminalAccessibilityBuffer::new(7, "Build");
        buffer.append(b"plain\x1b[31mred\x1b[0m\n", Some(10));
        for index in 0..=MAX_TERMINAL_ACCESSIBILITY_LINES {
            buffer.append(format!("line-{index}\n").as_bytes(), None);
        }
        buffer.append(&vec![b'x'; MAX_TERMINAL_ACCESSIBILITY_BYTES * 2], None);
        let snapshot = buffer.snapshot();
        assert!(!snapshot.text.contains('\x1b'));
        assert!(
            snapshot
                .text
                .ends_with(&"x".repeat(MAX_TERMINAL_ACCESSIBILITY_BYTES))
        );
        assert!(snapshot.text.len() <= MAX_TERMINAL_ACCESSIBILITY_BYTES);
        assert!(buffer.retained_lines() <= MAX_TERMINAL_ACCESSIBILITY_LINES);
        assert!(snapshot.truncated);
    }

    #[test]
    fn terminal_accessibility_preserves_fragmented_unicode_and_replaces_malformed_input() {
        let mut buffer = TerminalAccessibilityBuffer::new(8, "Unicode");
        let bytes = "A🙂ب".as_bytes();
        for byte in bytes {
            buffer.append(std::slice::from_ref(byte), None);
        }
        buffer.append(&[0xff, b'!'], None);
        assert_eq!(buffer.snapshot().text, "A🙂ب�!");
    }

    #[test]
    fn terminal_review_cursor_and_sensitive_clear_are_independent_from_input() {
        let mut buffer = TerminalAccessibilityBuffer::new(9, "Review");
        buffer.append(b"one\ntwo\nthree", Some(5));
        assert_eq!(buffer.move_read_cursor(2), 2);
        assert_eq!(buffer.snapshot().read_cursor, 2);
        buffer.clear_sensitive_content();
        let snapshot = buffer.snapshot();
        assert!(snapshot.text.is_empty());
        assert_eq!(snapshot.read_cursor, 0);
        assert!(!snapshot.truncated);
    }

    #[test]
    fn terminal_input_requires_input_focus_and_current_authorization() {
        assert!(TerminalFocusMode::Input.accepts_input(true));
        assert!(!TerminalFocusMode::Input.accepts_input(false));
        assert!(!TerminalFocusMode::Chrome.accepts_input(true));
        assert!(!TerminalFocusMode::AccessibleReview.accepts_input(true));
    }

    #[test]
    fn terminal_semantics_use_bounded_chunks_instead_of_cells() {
        let mut buffer = TerminalAccessibilityBuffer::new(10, "Long output");
        buffer.append(&vec![b'a'; MAX_TERMINAL_ACCESSIBILITY_BYTES], Some(2));
        let snapshot = TerminalSemanticSnapshot {
            terminal: buffer.snapshot(),
            focus_mode: TerminalFocusMode::AccessibleReview,
            input_authorized: true,
            recording_friendly: false,
            announcement: None,
        };
        let tree = snapshot.try_tree().expect("bounded semantic tree");
        assert!(tree.nodes().len() < 80);
        snapshot.try_router(&tree).expect("review actions route");
    }

    #[test]
    fn terminal_semantics_accept_trailing_newlines_and_sanitize_titles() {
        let mut buffer = TerminalAccessibilityBuffer::new(12, "unsafe\ntitle");
        buffer.append(b"line\n", Some(1));
        buffer.move_read_cursor_to_end();
        let snapshot = TerminalSemanticSnapshot {
            terminal: buffer.snapshot(),
            focus_mode: TerminalFocusMode::AccessibleReview,
            input_authorized: true,
            recording_friendly: false,
            announcement: None,
        };
        let tree = snapshot.try_tree().expect("trailing newline semantics");
        assert_eq!(terminal_semantic_chunk_count(&snapshot.terminal.text), 1);
        assert!(format!("{:?}", tree.nodes()).contains("unsafe title"));
    }

    #[test]
    fn terminal_announcements_are_content_free_and_rate_limited() {
        let start = Instant::now();
        let mut coalescer = TerminalAnnouncementCoalescer::new();
        coalescer.observe_output(4);
        assert_eq!(
            coalescer.flush(start),
            Some(TerminalAnnouncement::OutputAvailable { bytes: 4 })
        );
        coalescer.observe_output(8);
        assert_eq!(coalescer.flush(start), None);
        assert_eq!(
            coalescer.flush(start + TERMINAL_ANNOUNCEMENT_INTERVAL),
            Some(TerminalAnnouncement::OutputAvailable { bytes: 8 })
        );
    }

    #[test]
    fn terminal_lifecycle_focus_theme_and_recording_states_build_valid_semantics() {
        for theme in crate::ThemeKind::ALL {
            let tokens = crate::DesignTokens::new(theme);
            assert!(tokens.color_bg_terminal().alpha > 0);
            assert!(tokens.focus_ring_width().0 >= 2.0);
        }
        for lifecycle in TerminalLifecycle::ALL {
            let mut buffer = TerminalAccessibilityBuffer::new(11, "State fixture");
            buffer.append(b"sensitive fixture", Some(1));
            buffer.set_lifecycle(lifecycle);
            for focus_mode in [
                TerminalFocusMode::Chrome,
                TerminalFocusMode::Input,
                TerminalFocusMode::AccessibleReview,
            ] {
                let snapshot = TerminalSemanticSnapshot {
                    terminal: buffer.snapshot(),
                    focus_mode,
                    input_authorized: lifecycle == TerminalLifecycle::Live,
                    recording_friendly: true,
                    announcement: Some(TerminalAnnouncement::Lifecycle(lifecycle)),
                };
                let tree = snapshot.try_tree().expect("valid terminal state semantics");
                assert!(!format!("{:?}", tree.nodes()).contains("sensitive fixture"));
            }
        }
    }
}
