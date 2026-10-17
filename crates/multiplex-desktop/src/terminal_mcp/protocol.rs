//! MCP over JSON-RPC: `initialize`, `ping`, `tools/list` and `tools/call` for the terminal
//! tools. Stateless, so any connection may send any request once it has a valid token.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use multiplex_domain::HostedSessionId;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    InputChunk, OpError, ParsedKey, ReadRegion, ScrollRequest, TerminalBackend, TerminalOp,
};

/// Protocol versions this server answers in, newest first.
const SUPPORTED_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "multiplex-terminals";
const OP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_KEYS: usize = 256;
const DEFAULT_BUFFER_LINES: usize = 200;
const MAX_LINES: usize = 5_000;
const DEFAULT_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 15_000;
const MAX_WAIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_IDLE_MS: u64 = 1_000;
const WAIT_POLL: Duration = Duration::from_millis(100);

const INSTRUCTIONS: &str = "Controls the terminals open in the Multiplex desktop app: local \
shells, SSH sessions and durable sessions. Start with list_terminals and use a terminal_id from \
it. To run a command: send_input with text and submit=true, keep the output_cursor it returns, \
then wait_for_output with since=that cursor and either contains=<text you expect> or idle_ms. \
read_terminal shows the screen or scrollback; full-screen programs such as vim, less or top are \
driven with send_input keys (Up, Ctrl+C, Escape, F1 ...). Everything you type is seen by the \
person at this computer.";

pub struct McpProtocol {
    backend: Arc<dyn TerminalBackend>,
    /// The lines each terminal was last typed, and the output cursor they were typed at. The
    /// shell echoes them, so a wait from that cursor does not find its pattern in the command.
    typed: Mutex<HashMap<HostedSessionId, (u64, Vec<String>)>>,
}

struct ToolOutput {
    text: String,
    structured: Value,
}

type ToolResult = Result<ToolOutput, String>;

impl McpProtocol {
    pub fn new(backend: Arc<dyn TerminalBackend>) -> Self {
        Self {
            backend,
            typed: Mutex::new(HashMap::new()),
        }
    }

    /// Answers one posted body: a message or a batch of them. `None` when nothing in it
    /// expects an answer.
    pub fn handle_body(&self, body: Value) -> Option<Value> {
        match body {
            Value::Array(messages) if !messages.is_empty() => {
                let responses: Vec<Value> = messages
                    .into_iter()
                    .filter_map(|message| self.handle(message))
                    .collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            Value::Array(_) => Some(error_response(Value::Null, -32600, "empty batch")),
            message => self.handle(message),
        }
    }

    pub fn handle(&self, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_response(Value::Null, -32600, "invalid request"));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response or something unrecognisable; a response needs no answer.
            return id
                .filter(|_| !object.contains_key("result") && !object.contains_key("error"))
                .map(|id| error_response(id, -32600, "invalid request"));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let id = id?;
        Some(match method {
            "initialize" => success(id, initialize(&params)),
            "ping" => success(id, json!({})),
            "tools/list" => success(id, json!({ "tools": tool_definitions() })),
            "tools/call" => match self.call_tool(&params) {
                Ok(result) => success(id, result),
                Err(message) => error_response(id, -32602, &message),
            },
            _ => error_response(id, -32601, "method not found"),
        })
    }

    /// A tool result, or a protocol error for a call that names no known tool.
    fn call_tool(&self, params: &Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tools/call needs a tool name")?;
        let arguments = params
            .get("arguments")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| json!({}));
        let result = match name {
            "list_terminals" => self.list_terminals(),
            "read_terminal" => self.read_terminal(arguments),
            "send_input" => self.send_input(arguments),
            "wait_for_output" => self.wait_for_output(arguments),
            "scroll_terminal" => self.scroll_terminal(arguments),
            "focus_terminal" => {
                self.simple(arguments, |terminal_id| TerminalOp::Focus { terminal_id })
            }
            "open_terminal" => self.open_terminal(arguments),
            "close_terminal" => {
                self.simple(arguments, |terminal_id| TerminalOp::Close { terminal_id })
            }
            _ => return Err(format!("unknown tool `{name}`")),
        };
        Ok(match result {
            Ok(output) => json!({
                "content": [{ "type": "text", "text": output.text }],
                "structuredContent": output.structured,
                "isError": false,
            }),
            Err(message) => json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            }),
        })
    }

    fn op(&self, op: TerminalOp) -> Result<Value, String> {
        let terminal_id = op.terminal_id();
        self.backend
            .call(op, OP_TIMEOUT)
            .map_err(|error| match error {
                OpError::NotFound => match terminal_id {
                    Some(id) => format!(
                        "no open terminal has id {id}; call list_terminals for the current ids"
                    ),
                    None => "no Multiplex window is open".to_owned(),
                },
                OpError::Failed(message) => message,
            })
    }

    fn list_terminals(&self) -> ToolResult {
        let terminals = self.op(TerminalOp::List)?;
        let count = terminals.as_array().map_or(0, Vec::len);
        let structured = json!({ "terminals": terminals });
        let text = if count == 0 {
            "No terminals are open. open_terminal starts a local one.".to_owned()
        } else {
            pretty(&structured)
        };
        Ok(ToolOutput { text, structured })
    }

    fn read_terminal(&self, arguments: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            terminal_id: String,
            region: Option<String>,
            start_line: Option<usize>,
            max_lines: Option<usize>,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let region = match arguments.region.as_deref().unwrap_or("screen") {
            "screen" => ReadRegion::Screen,
            "viewport" => ReadRegion::Viewport,
            "buffer" | "scrollback" | "all" => ReadRegion::Buffer,
            other => {
                return Err(format!(
                    "region must be screen, viewport or buffer, not `{other}`"
                ));
            }
        };
        let value = self.op(TerminalOp::Read {
            terminal_id: terminal_id(&arguments.terminal_id)?,
            region,
            start_line: arguments.start_line,
            max_lines: arguments
                .max_lines
                .unwrap_or(DEFAULT_BUFFER_LINES)
                .clamp(1, MAX_LINES),
        })?;
        Ok(ToolOutput {
            text: render_read(&value),
            structured: value,
        })
    }

    fn send_input(&self, arguments: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            terminal_id: String,
            text: Option<String>,
            keys: Option<Vec<String>>,
            #[serde(default)]
            submit: bool,
            #[serde(default)]
            paste: bool,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let mut chunks = Vec::new();
        let mut typed_lines = Vec::new();
        if let Some(text) = arguments.text.filter(|text| !text.is_empty()) {
            typed_lines = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            if text.len() > MAX_TEXT_BYTES {
                return Err(format!("text is limited to {MAX_TEXT_BYTES} bytes"));
            }
            chunks.push(if arguments.paste {
                InputChunk::Paste(text)
            } else {
                InputChunk::Text(text)
            });
        }
        let keys = arguments.keys.unwrap_or_default();
        if keys.len() > MAX_KEYS {
            return Err(format!("at most {MAX_KEYS} keys are sent at once"));
        }
        for key in keys {
            chunks.push(InputChunk::Key(ParsedKey::parse(&key)?));
        }
        if arguments.submit {
            chunks.push(InputChunk::Key(ParsedKey::parse("Enter")?));
        }
        if chunks.is_empty() {
            return Err("send_input needs text, keys, or submit".to_owned());
        }
        let terminal_id = terminal_id(&arguments.terminal_id)?;
        let value = self.op(TerminalOp::Input {
            terminal_id,
            chunks,
        })?;
        if let Some(cursor) = value["output_cursor"].as_u64() {
            let mut typed = self.typed.lock().unwrap();
            if typed.len() >= 1_024 {
                typed.clear();
            }
            typed.insert(terminal_id, (cursor, typed_lines));
        }
        Ok(ToolOutput {
            text: format!(
                "Sent {} bytes. Pass since={} to wait_for_output to see what it printed.",
                value["bytes_sent"], value["output_cursor"]
            ),
            structured: value,
        })
    }

    fn wait_for_output(&self, arguments: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            terminal_id: String,
            contains: Option<String>,
            since: Option<u64>,
            idle_ms: Option<u64>,
            timeout_ms: Option<u64>,
            max_bytes: Option<usize>,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let terminal_id = terminal_id(&arguments.terminal_id)?;
        let contains = arguments.contains.filter(|text| !text.is_empty());
        let idle = arguments
            .idle_ms
            .or(contains.is_none().then_some(DEFAULT_IDLE_MS))
            .map(Duration::from_millis);
        let timeout = Duration::from_millis(
            arguments
                .timeout_ms
                .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
                .min(MAX_WAIT_TIMEOUT_MS),
        );
        let max_bytes = arguments
            .max_bytes
            .unwrap_or(DEFAULT_OUTPUT_BYTES)
            .clamp(256, MAX_OUTPUT_BYTES);
        let output = |since: Option<u64>| {
            self.op(TerminalOp::Output {
                terminal_id,
                since,
                max_bytes: MAX_OUTPUT_BYTES,
            })
        };

        let started = Instant::now();
        let first = output(arguments.since)?;
        let since = arguments
            .since
            .unwrap_or_else(|| first["output_cursor"].as_u64().unwrap_or(0));
        let mut last_cursor = first["output_cursor"].as_u64();
        let mut last_change = started;
        let echoed: Vec<String> = self
            .typed
            .lock()
            .unwrap()
            .get(&terminal_id)
            .filter(|(cursor, _)| *cursor == since)
            .map(|(_, lines)| lines.clone())
            .unwrap_or_default();
        let mut latest = first;
        let reason = loop {
            let text = latest["text"].as_str().unwrap_or_default();
            if let Some(pattern) = contains.as_deref()
                && without_echo(text, &echoed).contains(pattern)
            {
                break "matched";
            }
            if latest["state"] == "closed" || latest["state"] == "disconnected" {
                break "terminal_ended";
            }
            let now = Instant::now();
            if idle.is_some_and(|idle| now.duration_since(last_change) >= idle) {
                break "idle";
            }
            if now.duration_since(started) >= timeout {
                break "timeout";
            }
            std::thread::sleep(WAIT_POLL);
            latest = output(Some(since))?;
            let cursor = latest["output_cursor"].as_u64();
            if cursor != last_cursor {
                last_cursor = cursor;
                last_change = Instant::now();
            }
        };

        let full_text = latest["text"].as_str().unwrap_or_default();
        let (text, clipped) = keep_tail(full_text, max_bytes);
        let truncated = clipped || latest["truncated"].as_bool().unwrap_or(false);
        let structured = json!({
            "terminal_id": terminal_id.to_string(),
            "reason": reason,
            "matched": reason == "matched",
            "since": since,
            "output_cursor": latest["output_cursor"],
            "state": latest["state"],
            "truncated": truncated,
            "waited_ms": started.elapsed().as_millis() as u64,
            "output": text,
        });
        let mut rendered = format!(
            "reason: {reason}; waited {} ms; output_cursor {}{}\n---\n",
            started.elapsed().as_millis(),
            latest["output_cursor"],
            if truncated {
                "; earlier output omitted"
            } else {
                ""
            },
        );
        rendered.push_str(text);
        Ok(ToolOutput {
            text: rendered,
            structured,
        })
    }

    fn scroll_terminal(&self, arguments: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            terminal_id: String,
            lines: Option<i32>,
            pages: Option<i32>,
            to: Option<String>,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let scroll = match (arguments.to.as_deref(), arguments.pages, arguments.lines) {
            (Some("top"), None, None) => ScrollRequest::Top,
            (Some("bottom"), None, None) => ScrollRequest::Bottom,
            (Some(other), None, None) => {
                return Err(format!("to must be top or bottom, not `{other}`"));
            }
            (None, Some(pages), None) => ScrollRequest::Pages(pages.clamp(-1_000, 1_000)),
            (None, None, Some(lines)) => ScrollRequest::Lines(lines.clamp(-100_000, 100_000)),
            _ => return Err("give exactly one of lines, pages, or to".to_owned()),
        };
        let value = self.op(TerminalOp::Scroll {
            terminal_id: terminal_id(&arguments.terminal_id)?,
            scroll,
        })?;
        Ok(ToolOutput {
            text: render_read(&value),
            structured: value,
        })
    }

    fn open_terminal(&self, arguments: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            folder: Option<String>,
            title: Option<String>,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let value = self.op(TerminalOp::Open {
            folder: arguments.folder.filter(|value| !value.trim().is_empty()),
            title: arguments.title.filter(|value| !value.trim().is_empty()),
        })?;
        Ok(ToolOutput {
            text: format!(
                "{}\nThe shell is still starting: wait_for_output with idle_ms before typing.",
                pretty(&value)
            ),
            structured: value,
        })
    }

    fn simple(
        &self,
        arguments: Value,
        make: impl FnOnce(HostedSessionId) -> TerminalOp,
    ) -> ToolResult {
        #[derive(Deserialize)]
        struct Arguments {
            terminal_id: String,
        }
        let arguments: Arguments = parse_arguments(arguments)?;
        let value = self.op(make(terminal_id(&arguments.terminal_id)?))?;
        Ok(ToolOutput {
            text: pretty(&value),
            structured: value,
        })
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = SUPPORTED_VERSIONS
        .iter()
        .find(|version| **version == requested)
        .unwrap_or(&SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": SERVER_NAME,
            "title": "Multiplex terminals",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

fn tool_definitions() -> Value {
    let terminal_id = json!({
        "type": "string",
        "description": "A terminal_id from list_terminals.",
    });
    json!([
        {
            "name": "list_terminals",
            "title": "List open terminals",
            "description": "List every terminal pane open in Multiplex, in every window: its id, title, kind (local, ssh, session, tmux), where it is connected, state, size, scroll position, whether it is focused and whether it accepts input.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "read_terminal",
            "title": "Read a terminal",
            "description": "Read the text of a terminal. region=screen (default) is the live screen; viewport is what the pane shows at its current scroll position; buffer is scrollback and screen together, oldest first, where start_line picks the first line (default: the last max_lines lines). Also returns the cursor position.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "terminal_id": terminal_id,
                    "region": { "type": "string", "enum": ["screen", "viewport", "buffer"] },
                    "start_line": { "type": "integer", "minimum": 0, "description": "First buffer line to return, 0 being the oldest kept." },
                    "max_lines": { "type": "integer", "minimum": 1, "maximum": MAX_LINES, "description": "Buffer lines to return; default 200." },
                },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "send_input",
            "title": "Type into a terminal",
            "description": "Type into a terminal as if at its keyboard: text first, then each named key, then Enter when submit is true. A line break in text is sent as Enter. paste=true sends text as one bracketed paste when the program asked for that. Keys: Enter, Tab, Escape, Backspace, Delete, Up, Down, Left, Right, Home, End, PageUp, PageDown, Insert, Space, F1-F12, single characters, with Ctrl+, Alt+ and Shift+ (also C-c, M-x, ^C). Returns output_cursor for wait_for_output.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "terminal_id": terminal_id,
                    "text": { "type": "string", "maxLength": MAX_TEXT_BYTES },
                    "keys": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_KEYS },
                    "submit": { "type": "boolean", "description": "Press Enter after text and keys." },
                    "paste": { "type": "boolean", "description": "Send text as a paste rather than typing it." },
                },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true },
        },
        {
            "name": "wait_for_output",
            "title": "Wait for terminal output",
            "description": "Wait for output that arrives after `since` (an output_cursor from send_input; default: now). Returns when the output contains `contains`, when no output has arrived for idle_ms (default 1000 when contains is not given), when the terminal ends, or at timeout_ms (default 15000, at most 120000). The echo of the command send_input typed at `since` is not matched. Returns that output with escape sequences removed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "terminal_id": terminal_id,
                    "contains": { "type": "string", "description": "Plain text to wait for." },
                    "since": { "type": "integer", "minimum": 0 },
                    "idle_ms": { "type": "integer", "minimum": 0 },
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_TIMEOUT_MS },
                    "max_bytes": { "type": "integer", "minimum": 256, "maximum": MAX_OUTPUT_BYTES, "description": "Most output text to return, keeping the end; default 16384." },
                },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "scroll_terminal",
            "title": "Scroll a terminal",
            "description": "Scroll what the pane shows and return the new viewport. lines or pages are positive toward earlier output; to is top or bottom. A full-screen program (vim, less, tmux) is scrolled the way a mouse wheel would, through the program.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "terminal_id": terminal_id,
                    "lines": { "type": "integer" },
                    "pages": { "type": "integer" },
                    "to": { "type": "string", "enum": ["top", "bottom"] },
                },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
        },
        {
            "name": "focus_terminal",
            "title": "Show a terminal",
            "description": "Bring a terminal's window and tab to the front and focus its pane so the person at the computer sees it.",
            "inputSchema": {
                "type": "object",
                "properties": { "terminal_id": terminal_id },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        },
        {
            "name": "open_terminal",
            "title": "Open a local terminal",
            "description": "Open a new local shell in its own tab, in folder when given (default: the home folder), and return its terminal_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "An absolute folder to start in." },
                    "title": { "type": "string" },
                },
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "close_terminal",
            "title": "Close a terminal",
            "description": "Close a terminal pane as its close button does: an SSH session is disconnected and a local shell is ended.",
            "inputSchema": {
                "type": "object",
                "properties": { "terminal_id": terminal_id },
                "required": ["terminal_id"],
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false },
        },
    ])
}

/// A read result as text: one header line, then the lines.
fn render_read(value: &Value) -> String {
    let lines = value["lines"].as_array().map_or_else(Vec::new, |lines| {
        lines
            .iter()
            .map(|line| line.as_str().unwrap_or_default())
            .collect::<Vec<_>>()
    });
    let first = value["first_line"].as_u64().unwrap_or(0);
    let mut text = format!(
        "{} \"{}\" {}x{}; lines {}-{} of {}; cursor row {} col {}; scrolled back {}{}",
        value["terminal_id"].as_str().unwrap_or_default(),
        value["title"].as_str().unwrap_or_default(),
        value["columns"],
        value["rows"],
        first,
        (first + lines.len() as u64).saturating_sub(1),
        value["total_lines"],
        value["cursor"]["row"],
        value["cursor"]["col"],
        value["scrolled_back"],
        if value["alternate_screen"] == true {
            "; full-screen program"
        } else {
            ""
        },
    );
    if let Some(method) = value["scrolled_with"].as_str() {
        text.push_str("; scrolled with ");
        text.push_str(method);
    }
    text.push_str("\n---\n");
    text.push_str(&lines.join("\n"));
    text
}

/// `output` without the echo of the lines just typed. A shell may echo a line more than once
/// as it redraws its prompt, so every copy goes.
fn without_echo(output: &str, typed: &[String]) -> String {
    typed.iter().fold(output.to_owned(), |output, line| {
        output.replace(line.as_str(), "")
    })
}

/// The end of `text` that fits in `max_bytes`, on a character boundary.
fn keep_tail(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|error| format!("invalid arguments: {error}"))
}

fn terminal_id(value: &str) -> Result<HostedSessionId, String> {
    HostedSessionId::from_str(value.trim())
        .map_err(|_| format!("`{value}` is not a terminal_id; call list_terminals"))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(super) fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::terminal_mcp::OpResult;

    const ID: &str = "0b0c3a52-38ad-4a4b-9a43-0d3f1a9c2e11";

    /// A terminal that, once typed into, prints `building` on three polls and then `done`.
    #[derive(Default)]
    struct FakeBackend {
        ops: Mutex<Vec<TerminalOp>>,
        output: Mutex<String>,
        polls_since_input: Mutex<Option<usize>>,
    }

    impl TerminalBackend for FakeBackend {
        fn call(&self, op: TerminalOp, _timeout: Duration) -> OpResult {
            self.ops.lock().unwrap().push(op.clone());
            if op.terminal_id().is_some_and(|id| id.to_string() != ID) {
                return Err(OpError::NotFound);
            }
            let mut output = self.output.lock().unwrap();
            match op {
                TerminalOp::List => Ok(json!([{ "terminal_id": ID, "title": "zsh" }])),
                TerminalOp::Input { chunks, .. } => {
                    *self.polls_since_input.lock().unwrap() = Some(0);
                    let cursor = output.len();
                    // Echoed twice, as a shell redrawing its prompt does.
                    for chunk in &chunks {
                        if let InputChunk::Text(text) = chunk {
                            output.push_str(&format!("$ {text}\n$ {text}\n"));
                        }
                    }
                    Ok(json!({ "terminal_id": ID, "bytes_sent": 5, "output_cursor": cursor }))
                }
                TerminalOp::Output { since, .. } => {
                    let mut polls = self.polls_since_input.lock().unwrap();
                    if let Some(count) = polls.as_mut() {
                        *count += 1;
                        match *count {
                            1..=3 => output.push_str("building\n"),
                            4 => output.push_str("done\n"),
                            _ => {}
                        }
                    }
                    let since = since.unwrap_or(output.len() as u64) as usize;
                    Ok(json!({
                        "output_cursor": output.len(),
                        "text": output[since.min(output.len())..],
                        "truncated": false,
                        "state": "connected",
                    }))
                }
                TerminalOp::Read { .. } | TerminalOp::Scroll { .. } => Ok(json!({
                    "terminal_id": ID,
                    "title": "zsh",
                    "columns": 80,
                    "rows": 2,
                    "first_line": 0,
                    "total_lines": 2,
                    "lines": ["$ ls", "a b"],
                    "cursor": { "row": 1, "col": 3 },
                    "scrolled_back": 0,
                    "alternate_screen": false,
                })),
                _ => Ok(json!({ "terminal_id": ID })),
            }
        }
    }

    fn server() -> (McpProtocol, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend::default());
        (McpProtocol::new(backend.clone()), backend)
    }

    fn call(server: &McpProtocol, name: &str, arguments: Value) -> Value {
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))
            .expect("a call is answered")
    }

    #[test]
    fn initialize_negotiates_a_supported_version_and_lists_the_tools() {
        let (server, _) = server();
        let response = server
            .handle(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-03-26", "capabilities": {} },
            }))
            .unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
        let unknown = server
            .handle(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "1999-01-01" },
            }))
            .unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);

        assert!(
            server
                .handle(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );
        let tools = server
            .handle(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .unwrap();
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_terminals",
                "read_terminal",
                "send_input",
                "wait_for_output",
                "scroll_terminal",
                "focus_terminal",
                "open_terminal",
                "close_terminal",
            ]
        );
    }

    #[test]
    fn send_input_types_text_then_keys_then_enter() {
        let (server, backend) = server();
        let response = call(
            &server,
            "send_input",
            json!({ "terminal_id": ID, "text": "make", "keys": ["C-c"], "submit": true }),
        );
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"]["output_cursor"], 0);
        let ops = backend.ops.lock().unwrap();
        let TerminalOp::Input { chunks, .. } = &ops[0] else {
            panic!("input op");
        };
        assert_eq!(
            chunks,
            &vec![
                InputChunk::Text("make".to_owned()),
                InputChunk::Key(ParsedKey::parse("Ctrl+C").unwrap()),
                InputChunk::Key(ParsedKey::parse("Enter").unwrap()),
            ]
        );
    }

    #[test]
    fn bad_arguments_are_tool_errors_and_unknown_tools_are_protocol_errors() {
        let (server, backend) = server();
        let bad_key = call(
            &server,
            "send_input",
            json!({ "terminal_id": ID, "keys": ["Hyper+x"] }),
        );
        assert_eq!(bad_key["result"]["isError"], true);
        let nothing = call(&server, "send_input", json!({ "terminal_id": ID }));
        assert_eq!(nothing["result"]["isError"], true);
        let bad_id = call(&server, "read_terminal", json!({ "terminal_id": "../etc" }));
        assert_eq!(bad_id["result"]["isError"], true);
        let missing = call(
            &server,
            "read_terminal",
            json!({ "terminal_id": "00000000-0000-0000-0000-000000000000" }),
        );
        assert!(
            missing["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("list_terminals")
        );
        let scroll = call(
            &server,
            "scroll_terminal",
            json!({ "terminal_id": ID, "lines": 3, "to": "top" }),
        );
        assert_eq!(scroll["result"]["isError"], true);
        assert_eq!(call(&server, "rm_rf", json!({}))["error"]["code"], -32602);
        assert_eq!(backend.ops.lock().unwrap().len(), 1);
    }

    #[test]
    fn wait_for_output_returns_what_arrived_after_the_cursor() {
        let (server, _) = server();
        // The command itself contains what it waits for; only the later output may match.
        let sent = call(
            &server,
            "send_input",
            json!({ "terminal_id": ID, "text": "make && echo done" }),
        );
        let since = sent["result"]["structuredContent"]["output_cursor"].clone();
        let waited = call(
            &server,
            "wait_for_output",
            json!({ "terminal_id": ID, "since": since, "contains": "done\n", "timeout_ms": 5000 }),
        );
        let structured = &waited["result"]["structuredContent"];
        assert_eq!(structured["reason"], "matched");
        let output = structured["output"].as_str().unwrap();
        assert!(output.contains("building\n"), "{output}");
        assert!(output.ends_with("\ndone\n"), "{output}");
    }

    #[test]
    fn wait_for_output_times_out_without_a_match() {
        let (server, _) = server();
        let waited = call(
            &server,
            "wait_for_output",
            json!({ "terminal_id": ID, "contains": "never", "timeout_ms": 250 }),
        );
        assert_eq!(waited["result"]["structuredContent"]["reason"], "timeout");
        assert_eq!(waited["result"]["isError"], false);
    }

    #[test]
    fn reads_render_a_header_and_the_lines() {
        let (server, _) = server();
        let read = call(&server, "read_terminal", json!({ "terminal_id": ID }));
        let text = read["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("80x2; lines 0-1 of 2; cursor row 1 col 3"),
            "{text}"
        );
        assert!(text.ends_with("---\n$ ls\na b"), "{text}");
    }

    #[test]
    fn batches_and_stray_messages_are_answered_by_the_rules() {
        let (server, _) = server();
        let batch = server
            .handle_body(json!([
                { "jsonrpc": "2.0", "id": 1, "method": "ping" },
                { "jsonrpc": "2.0", "method": "notifications/cancelled" },
            ]))
            .unwrap();
        assert_eq!(batch.as_array().unwrap().len(), 1);
        assert!(
            server
                .handle_body(json!({ "jsonrpc": "2.0", "id": 3, "result": {} }))
                .is_none()
        );
        assert_eq!(
            server.handle_body(json!("x")).unwrap()["error"]["code"],
            -32600
        );
        assert_eq!(
            server
                .handle_body(json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }))
                .unwrap()["error"]["code"],
            -32601
        );
    }
}
