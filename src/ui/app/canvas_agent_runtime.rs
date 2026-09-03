//! Bounded structured-agent runtime and transcript state for Canvas nodes.

use gpui::{FocusHandle, ScrollHandle};

use crate::agents::{
    AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState, CodexSessionHandle,
    HeadlessSessionHandle, RemoteHeadlessSessionHandle,
};
use crate::ui::render_terminal::{SelectionRange, normalized_selection};

pub(super) enum StructuredAgentHandle {
    Codex(Box<CodexSessionHandle>),
    Headless(Box<HeadlessSessionHandle>),
    RemoteHeadless(Box<RemoteHeadlessSessionHandle>),
}

impl StructuredAgentHandle {
    pub(super) fn try_recv(&self) -> Result<AgentEvent, std::sync::mpsc::TryRecvError> {
        match self {
            Self::Codex(handle) => handle.event_rx.try_recv(),
            Self::Headless(handle) => handle.event_rx.try_recv(),
            Self::RemoteHeadless(handle) => handle.event_rx.try_recv(),
        }
    }

    pub(super) fn send_prompt(&self, prompt: String) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.send_prompt(prompt),
            Self::Headless(handle) => handle.send_prompt(prompt),
            Self::RemoteHeadless(handle) => handle.send_prompt(prompt),
        }
    }

    pub(super) fn cancel(&self) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.cancel(),
            Self::Headless(handle) => handle.cancel(),
            Self::RemoteHeadless(handle) => handle.cancel(),
        }
    }

    pub(super) fn respond_to_approval(&self, request_id: &str, allow: bool) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.respond_to_approval(request_id, allow),
            Self::Headless(_) | Self::RemoteHeadless(_) => {
                anyhow::bail!("This provider did not expose an approval request")
            }
        }
    }
}

pub(super) struct StructuredAgentRuntime {
    pub handle: StructuredAgentHandle,
    pub state: AgentRunState,
    pub transcript: String,
    pub context_messages: Vec<(AgentRole, String)>,
    pub approval: Option<AgentApprovalRequest>,
    pub diagnostic: Option<String>,
    pub queued_prompt: Option<String>,
    pub unread_output: bool,
    pub transcript_scroll: ScrollHandle,
    pub follow_transcript: bool,
    pub selection: Option<SelectionRange>,
    pub dragging_selection: bool,
    pub transcript_focus: FocusHandle,
}

impl StructuredAgentRuntime {
    const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

    pub(super) fn new(
        handle: StructuredAgentHandle,
        initial_prompt: Option<&str>,
        transcript_focus: FocusHandle,
    ) -> Self {
        let mut runtime = Self {
            handle,
            state: AgentRunState::Starting,
            transcript: String::new(),
            context_messages: Vec::new(),
            approval: None,
            diagnostic: None,
            queued_prompt: None,
            unread_output: false,
            transcript_scroll: ScrollHandle::new(),
            follow_transcript: true,
            selection: None,
            dragging_selection: false,
            transcript_focus,
        };
        if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
            runtime.push_context_message(AgentRole::User, prompt.trim());
        }
        runtime
    }

    pub(super) fn push_context_message(&mut self, role: AgentRole, text: &str) {
        if let Some((last_role, last_text)) = self.context_messages.last_mut()
            && *last_role == role
        {
            last_text.push_str(text);
        } else {
            self.context_messages.push((role, text.to_string()));
            if self.context_messages.len() > 100 {
                self.context_messages.remove(0);
            }
        }
        let mut excess = self
            .context_messages
            .iter()
            .map(|(_, text)| text.len())
            .sum::<usize>()
            .saturating_sub(Self::MAX_TRANSCRIPT_BYTES);
        while excess > 0 && !self.context_messages.is_empty() {
            let first_len = self.context_messages[0].1.len();
            if first_len <= excess {
                excess -= first_len;
                self.context_messages.remove(0);
                continue;
            }
            let mut drain_end = excess;
            while !self.context_messages[0].1.is_char_boundary(drain_end) {
                drain_end += 1;
            }
            self.context_messages[0].1.drain(..drain_end);
            excess = 0;
        }
    }

    pub(super) fn push_text(&mut self, text: &str) {
        self.transcript.push_str(text);
        if self.transcript.len() > Self::MAX_TRANSCRIPT_BYTES {
            let mut start = self.transcript.len() - Self::MAX_TRANSCRIPT_BYTES;
            while !self.transcript.is_char_boundary(start) {
                start += 1;
            }
            self.transcript.drain(..start);
            self.selection = None;
            self.dragging_selection = false;
        }
        if self.follow_transcript {
            self.transcript_scroll.scroll_to_bottom();
        }
    }
}

pub(super) fn structured_transcript_lines(transcript: &str) -> Vec<&str> {
    let visible = transcript.trim_end_matches(['\r', '\n']);
    if visible.is_empty() {
        vec![""]
    } else {
        visible
            .split('\n')
            .map(|line| line.trim_end_matches('\r'))
            .collect()
    }
}

pub(super) fn structured_transcript_selected_text(
    transcript: &str,
    selection: SelectionRange,
) -> Option<String> {
    let selection = normalized_selection(selection)?;
    let lines = structured_transcript_lines(transcript);
    let start_row = usize::from(selection.anchor.row);
    let end_row = usize::from(selection.head.row);
    if start_row >= lines.len() || end_row >= lines.len() {
        return None;
    }

    let mut selected = String::new();
    for (row, line) in lines
        .iter()
        .enumerate()
        .take(end_row.saturating_add(1))
        .skip(start_row)
    {
        if row > start_row {
            selected.push('\n');
        }
        let start_col = if row == start_row {
            usize::from(selection.anchor.col)
        } else {
            0
        };
        let end_col = if row == end_row {
            usize::from(selection.head.col)
        } else {
            line.chars().count()
        };
        selected.extend(
            line.chars()
                .skip(start_col)
                .take(end_col.saturating_sub(start_col)),
        );
    }

    (!selected.is_empty()).then_some(selected)
}
