# Terminal MCP server

The desktop app can run an MCP server that lets an agent on this computer — a chat bot, a
coding agent, an orchestrator — drive the terminals the app has open: local shells, SSH
sessions and durable sessions, in every window.

It is off by default. Turn it on in **Remote Devices → This computer** (or **Settings → Remote
Devices**) under **Terminal MCP server**, or start the app with `MULTIPLEX_TERMINAL_MCP=1`.

## Connecting a client

The server speaks MCP's Streamable HTTP transport at `http://127.0.0.1:47913/mcp`
(`MULTIPLEX_TERMINAL_MCP_PORT` changes the port; a busy port falls back to a free one, and the
card shows the real URL). Every request carries `Authorization: Bearer <token>`.
**Copy client config** gives a block like this:

```json
{
  "mcpServers": {
    "multiplex-terminals": {
      "type": "http",
      "url": "http://127.0.0.1:47913/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    },
    "multiplex-terminals-stdio": {
      "command": "/Applications/Multiplex.app/Contents/MacOS/multiplex",
      "args": ["terminal-mcp", "--stdio"]
    }
  }
}
```

Clients that only start commands use the second entry: `multiplex terminal-mcp --stdio` reads
the running server's address and token from `terminal-mcp.json` in the app's data folder and
forwards each line to it.

The server listens on loopback only. An agent hosted elsewhere (for example a cloud bot calling
remote MCP servers) needs a tunnel you set up yourself, and then anyone with the URL and token
controls your terminals.

## Tools

| Tool | What it does |
| --- | --- |
| `list_terminals` | Every pane in every window: `terminal_id`, title, kind (`local`, `ssh`, `session`, `tmux`), endpoint, state, size, scroll position, focus, whether it takes input, and its `output_cursor`. |
| `read_terminal` | `region=screen` (live screen), `viewport` (what the pane shows at its scroll position) or `buffer` (scrollback and screen, `start_line`/`max_lines`), with the cursor. |
| `send_input` | Types `text`, then each of `keys` (`Enter`, `Ctrl+C`, `C-d`, `^Z`, `Shift+Tab`, `Up`, `PageDown`, `F5`, single characters), then Enter when `submit`. `paste=true` sends text as one bracketed paste. Returns `output_cursor`. |
| `wait_for_output` | Waits for output after `since` until it `contains` some text, has been quiet for `idle_ms`, the terminal ends, or `timeout_ms` (at most 120 s). The shell's echo of the command typed at `since` is not matched. |
| `scroll_terminal` | `lines`, `pages` (positive is toward earlier output) or `to=top/bottom`. Full-screen programs (less, vim, tmux) are scrolled through the program, as the mouse wheel does. |
| `focus_terminal` | Brings the pane's window and tab forward. |
| `open_terminal` | Opens a local shell in a new tab, optionally in `folder`. |
| `close_terminal` | Closes a pane as its close button does. |

A typical command: `send_input {terminal_id, text: "cargo test", submit: true}` →
`wait_for_output {terminal_id, since: <output_cursor>, contains: "test result"}`.

## Security

- Anything holding the token can type into every terminal in the app. The token lives in
  `terminal-mcp-token` (mode 0600) in the app's data folder and survives restarts so clients
  stay configured; **New token** replaces it.
- Only loopback connections are accepted, and a request carrying a browser `Origin` other than
  the server's own is refused, so a web page cannot reach it through DNS rebinding.
- Input goes through the same path as the keyboard: a durable session another viewer is typing
  in refuses it, and what the agent types is visible in the pane.
- Panes keep the last 256 KiB of their output for `wait_for_output` only while the server runs.

The code is in `crates/multiplex-desktop/src/terminal_mcp/` (transport, protocol, hub) and
`ui/app/terminal_mcp.rs` (each window's answers on its own loop).
