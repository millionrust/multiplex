//! An MCP server inside the desktop app for the terminals it has open.
//!
//! An agent such as a chat bot or a coding agent lists the panes every window holds, reads
//! their screen and scrollback, types into them, scrolls them, waits for output, and opens or
//! closes local terminals. It speaks Streamable HTTP on loopback only, behind a bearer token
//! (`http.rs`); `multiplex terminal-mcp --stdio` bridges stdio clients to it (`stdio.rs`).
//!
//! The panes belong to each window's `MultiplexApp` on the GPUI main thread, so nothing here
//! touches them. The hub (`hub.rs`) hands each window a [`WindowCalls`] queue; the window
//! answers a [`TerminalOp`] on its own loop (`ui/app/terminal_mcp.rs`) and the hub merges the
//! answers. The protocol (`protocol.rs`) is written against [`TerminalBackend`] so it is tested
//! without a window.
//!
//! It is off unless Settings → Terminal MCP server or `MULTIPLEX_TERMINAL_MCP=1` turns it on.

mod http;
mod hub;
mod keys;
mod output;
mod protocol;
pub mod stdio;

use std::time::Duration;

use multiplex_domain::HostedSessionId;
use serde_json::Value;

pub use hub::{ServerInfo, WindowCalls, hub, read_discovery};
pub use keys::ParsedKey;
pub use output::{OutputLog, strip_terminal_controls};

/// The port the server asks for first; another free one is taken when it is busy.
pub const DEFAULT_PORT: u16 = 47913;
/// The path MCP clients post to.
pub const ENDPOINT_PATH: &str = "/mcp";

/// One request a window answers for the panes it holds.
#[derive(Clone, Debug, PartialEq)]
pub enum TerminalOp {
    List,
    Read {
        terminal_id: HostedSessionId,
        region: ReadRegion,
        start_line: Option<usize>,
        max_lines: usize,
    },
    /// Output that arrived after `since`, with control sequences removed.
    Output {
        terminal_id: HostedSessionId,
        since: Option<u64>,
        max_bytes: usize,
    },
    Input {
        terminal_id: HostedSessionId,
        chunks: Vec<InputChunk>,
    },
    Scroll {
        terminal_id: HostedSessionId,
        scroll: ScrollRequest,
    },
    Focus {
        terminal_id: HostedSessionId,
    },
    Open {
        folder: Option<String>,
        title: Option<String>,
    },
    Close {
        terminal_id: HostedSessionId,
    },
}

impl TerminalOp {
    /// The pane this op is about, when it is about one.
    pub fn terminal_id(&self) -> Option<HostedSessionId> {
        match self {
            Self::List | Self::Open { .. } => None,
            Self::Read { terminal_id, .. }
            | Self::Output { terminal_id, .. }
            | Self::Input { terminal_id, .. }
            | Self::Scroll { terminal_id, .. }
            | Self::Focus { terminal_id }
            | Self::Close { terminal_id } => Some(*terminal_id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadRegion {
    /// The live screen, whatever the view is scrolled to.
    Screen,
    /// What the pane shows right now, following its scroll position.
    Viewport,
    /// History and screen together, oldest line first.
    Buffer,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputChunk {
    /// Typed text; a line break is sent as Enter.
    Text(String),
    /// Pasted text, bracketed when the program asked for bracketed paste.
    Paste(String),
    Key(ParsedKey),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollRequest {
    /// Positive moves toward earlier output.
    Lines(i32),
    Pages(i32),
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpError {
    /// No pane in this window has that id.
    NotFound,
    Failed(String),
}

pub type OpResult = Result<Value, OpError>;

/// Where tool calls are answered: the hub in the app, a fake in tests.
pub trait TerminalBackend: Send + Sync {
    fn call(&self, op: TerminalOp, timeout: Duration) -> OpResult;
}

/// Whether the server is enabled by the environment, whatever Settings says.
pub fn enabled_by_environment() -> bool {
    multiplex_env::var("MULTIPLEX_TERMINAL_MCP")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "on" | "yes"))
}

/// The port to listen on: `MULTIPLEX_TERMINAL_MCP_PORT`, or [`DEFAULT_PORT`].
pub fn configured_port() -> u16 {
    multiplex_env::var("MULTIPLEX_TERMINAL_MCP_PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}
