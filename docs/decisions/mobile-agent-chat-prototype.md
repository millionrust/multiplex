# Mobile chat view for agent terminals

Status: implemented on iPhone and Android (after v0.0.15), reading the provider's own session
log. The sections below the decision are the prototype's original reasoning, kept for context.

## Decision (2026-10-19): read the agent's own session log

A Claude Code or Codex session a phone can see is an ordinary terminal; the structured events in
`agents/protocol.rs` exist only for the desktop canvas's own jobs, which phones are not given.
Rather than parse terminal text, the computer reads the agent's session log, which the owner chose
over the "do not read provider files" rule below. What that means:

- **Finding the agent.** `multiplex-controller-listener/src/agent_chat.rs` walks the processes
  under the terminal's shell (Session Host pid, or tmux `pane_pid`) for one started as `claude` or
  `codex` (the native Claude install names its binary after its version, so `argv[0]` counts; a
  Node or Bun script is matched by path). Its folder is its live current directory.
- **Finding its log.** Claude Code: `~/.claude/projects/<folder with non-alphanumerics as
  dashes>/<session>.jsonl`, the session being the `CLAUDE_CODE_SESSION_ID` in the youngest command
  the agent is running, unless a log begun after that command is newer; otherwise the newest log
  for the folder written since the agent started that no other running Claude Code claims in its
  `~/.claude/sessions/<pid>.json`. That record names only the session the process started in;
  `/clear` and `/resume` do not update it, and long-lived children such as MCP servers keep that
  first id, which is why the youngest command is the witness. Codex: the newest
  `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` whose `session_meta` names the agent's folder,
  written since it started. Codex keeps no process record, so two Codex sessions in one folder
  are told apart only by which wrote last. `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are honoured as
  the listener sees them.
- **Reading it.** The newest 12 MiB of the log, parsed into user, assistant, tool and notice
  items with the log's own ids; cached by size and modification time, so a phone asking every
  1.5 s costs a `stat`. Thinking, sidechains, injected context and records not recognised are
  skipped, not guessed at. Tool output is clipped to 2 KiB and messages to 8 KiB, and an answer
  is trimmed oldest first to fit one control frame.
- **The wire.** `agent_chat { session_id, occupant_generation, before?, since_revision?, limit }`
  answers `agent_chat { chat }`, `null` when no agent runs there. It needs `AttachOutput`, as the
  terminal does. The phones page back with `before` and poll with `since_revision`. Only the
  private-network route carries it; desktop SSH panes answer `null`.
- **Replying.** Send types the draft into the terminal through the existing writer lease: a
  bracketed paste (lines joined when the program has not asked for bracketed paste), then Return.
  Approvals and menus stay in Terminal view; nothing read from a log becomes a button.
- **What can be wrong.** A provider can change its log format in any release; unknown records
  are dropped, so a format change shows as a thinner conversation rather than a wrong one. The
  state (Working or Ready) is inferred from the last records, and a permission prompt shows as a
  running tool.

## Intended experience

Opening a Claude Code or Codex session offers Chat and Terminal views of the same
session. Chat lays out user messages and assistant replies to the phone's width,
keeps code in contained blocks, and collapses tool output into expandable cards.
It shows work, approval, and disconnection states without switching sessions.
The draft stays visible above the existing in-app keyboard. The original terminal
remains available when conversation interpretation is incomplete.

The runnable prototype is
[`design/remote-screens/mobile-agent-chat.html`](../../design/remote-screens/mobile-agent-chat.html).
Open that file directly, or serve its directory with any static HTTP server.
It uses Multiplex's existing mobile prototype palette and requires no external
libraries. Its data and replies are synthetic; sending/approving does not affect
any computer. The transcript preview runs locally and sends nothing over a network.

## Use existing structured events first

`crates/multiplex-desktop/src/agents/protocol.rs` already defines user/assistant
`MessageDelta`, tool start/finish, approval, completion, and failure events. The
Codex app-server adapter and the structured agent adapters are better sources than
rendered TUI cells: events retain roles, boundaries, tool identities and request
identities. Move the shared event model into a GPUI-free contract before exposing
it through the Controller and mobile FFI.

This does not imply that every external CLI session already has structured events.
The candidate transcript reader in `agents/transcript.rs` is dormant in release
builds pending an approved provider/version contract. Keep that gate intact.

## Terminal-text fallback

The prototype parser recognizes explicit speaker labels and user prompt markers,
strips styling/control strings, preserves code fences, and leaves unclassified
output visible. It bounds input and item counts and labels omitted/redrawn output.
It never turns terminal text into an approval or command.

Production fallback should consume the existing terminal model and logical wrapped
lines, rather than claim a regex can reconstruct arbitrary VT output. Full-screen
redraws, spinners, scrolling, pasted prompt markers, menus, alternate-screen
programs and provider changes can make roles or history unrecoverable. Use a
provider/version-specific parser only after fixtures demonstrate its behavior;
otherwise offer the terminal or a view-only, partial transcript. Do not invent
messages, collapse unknown output, or automatically read arbitrary provider files.

## Implementation sequence

1. Add an additive, bounded conversation page/stream contract keyed by the
   original Session UUID and occupant generation. Give messages, tools and
   approvals stable IDs and a monotonic cursor; reconnect resumes that cursor
   without duplicating items. Page older messages instead of retaining unlimited
   conversation history in phone memory.
2. Publish structured projections from managed agent sessions. Leave ordinary
   shells as terminals. Add inspected provider/version contracts for external
   sessions as a separate, optional fallback.
3. Add native iOS/Android chat surfaces beside the existing terminal view. Preserve
   the selected session, draft, scroll position and expanded tool cards across
   updates/reconnects. A combined shared-screen view keeps its picture frame fixed
   while the chat composer and custom keyboard occupy the space below it.
4. Route input through the existing authorization and writer-lease checks. Enable
   structured approvals only for an identified live request and a controller
   allowed to act. A parsed string such as “Allow this command?” is not a request.
   Sending a managed prompt uses the provider adapter; raw TUI input requires a
   separately validated input contract and an available prompt, not blind stdin
   injection while a tool or menu is active.
5. Validate Claude/Codex fixtures with fragmented UTF-8 and message deltas, fenced
   code, interleaved tools, long logs, cancelled/failed turns, approval resolution,
   provider upgrades, read-only permission changes, lease handover, redraws,
   reconnects and missing history. Check real phones in portrait/landscape and
   with the shared picture and custom keyboard active together.

## Prototype validation

- Six model checks pass: role parsing with styling, fenced code, unknown output,
  tool boundaries, persistent approval resolution, bounded/truncated data and
  Unicode/hyperlink text.
- Browser interactions passed at 360/390/430 px without conversation overflow.
- Tool expansion survives streaming updates; a resolved approval does not
  reappear as pending after another message.
- Reconnection retains the selected provider/session; parsed views disable Send.
- Pasted markup is rendered as literal text rather than HTML.

Run the model checks with:

```sh
node --test design/remote-screens/mobile-agent-chat-model.test.cjs
```
