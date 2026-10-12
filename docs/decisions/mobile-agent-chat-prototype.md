# Mobile chat view for agent terminals

Status: prototype / future work. Not part of the native mobile app or v0.0.14.

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
