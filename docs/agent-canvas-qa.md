# Agent Canvas v1 QA Record

Date: 2026-07-17

Environment: macOS development machine, branch `test`, Rust test profile.

## Automated results

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Pass | No formatting differences. |
| `cargo check -q` | Pass | Existing macOS `objc` cfg and dead-code warnings remain. |
| `cargo test -q` | Pass | 315 passed, 0 failed, 3 ignored; Docker-backed and local tmux integration tests exercised. |
| Agent adapter focused tests | Pass | Local and remote Codex fake app-server, Claude/Gemini stream fixtures, literal arguments, cancellation, malformed data, and remote exit-status ordering. |
| Worktree integration tests | Pass | Real temporary Git repositories cover create, status, dirty refusal, committed refusal, and path boundaries. |
| Canvas capacity test | Pass | 20 nodes, 40 edges, finite fit geometry and supported zoom. |
| `cargo clippy --all-targets` | Pass with warnings | Repository reports existing warnings. |
| `cargo clippy --all-targets -- -D warnings` | Blocked | 121 existing warnings include old `objc` cfg macros and unrelated UI lints; not changed in this feature. |
| `git diff --check` | Pass | No whitespace errors. |
| Development artifact storage | Pass | After `cargo clean` removed 21.6 GiB, non-incremental symbol-free development/test profiles rebuilt the full suite into a 1.7 GiB `target/`; guarded verification finished with 18 GiB free. |

Docker Desktop 29.5.3 was available for the final run. All three
`docker_ssh_persistent_tmux` tests exercised real SSH containers and passed in
40.87s: reconnect preserved the session without replaying startup, missing tmux
fell back with guidance, and the remote kill control path removed the session.
The full suite also exercised the other Docker-backed SSH, SFTP, forwarding,
jump-host, restore, and application integration tests instead of their
Docker-unavailable early returns.

## Live provider checks

Live checks are ignored during normal tests because they require installed,
authenticated CLIs, network access, and may consume provider quota. Run one at a
time with the explicit opt-in flag:

```bash
TERMIRUST_RUN_LIVE_AGENT_TESTS=1 cargo test live_codex_app_server_smoke -- --ignored --nocapture
TERMIRUST_RUN_LIVE_AGENT_TESTS=1 cargo test live_claude_headless_smoke -- --ignored --nocapture
TERMIRUST_RUN_LIVE_AGENT_TESTS=1 cargo test live_gemini_headless_smoke -- --ignored --nocapture
```

| Provider | Result | Notes |
| --- | --- | --- |
| Codex CLI 0.144.4 | Pass | Authenticated app-server completed a read-only, no-tools marker request in 5.27s. |
| Claude Code 2.1.126 | Account blocked | The adapter launched and normalized the provider error, but the organization disables subscription access to Claude Code. An API key or administrator change is required. |
| Gemini CLI | Not exercised | The executable is not installed on this machine; TermiRust's missing-provider guidance remains covered deterministically. |

## Native launch smoke

`cargo run` built and launched `target/debug/termirust`. CoreGraphics reported
one on-screen TermiRust window with bounds `1175x946`; the process remained alive
until it was intentionally interrupted. macOS denied automated `screencapture`,
so no screenshot or automated click-through is claimed.

## Covered behavior

- Legacy state defaults to Split and new canvas state round-trips.
- Invalid geometry, duplicate IDs, dangling edges, and dependency cycles repair
  or reject deterministically.
- Pan/zoom transforms, cursor anchoring, placement, fit, minimum resize, and
  stable restore mapping are unit tested. Keyboard node selection cycles and
  wraps in both directions. Layout-only undo/redo preserves newly created
  runtimes, and the clickable minimap maps back to world coordinates.
- Project-folder selection persists with the workspace and drives new local
  terminal and agent working directories. The rendered workflow opens Files,
  edits and saves a UTF-8 file, refuses a stale overwrite after an external
  agent change, and restores the project path.
- Project file access is canonical-root bounded, rejects escaping symlinks,
  binary data, and files over 1 MB. Git status and selected-file diff use direct
  argument arrays, bounded output, and inspect/copy-only controls.
- Node titles normalize predictably, link enable/delete actions preserve nodes,
  and canvas/node mouse ownership prevents terminal clicks from panning the
  background.
- Node headers expose location and lifecycle text, classify actionable
  attention states, mark unread output, and keep drag handling on the
  title/status region. Agent Activity summarizes running, queued, attention,
  and unread counts with context/dependency visibility. A rendered test opens
  Activity for an off-screen failed agent, focuses and reveals it, and clears
  its unread marker. A
  rendered GPUI click test verifies the More menu opens and closes without the
  action being intercepted as a node drag.
- The shared workspace shell renders exactly one guarded-paste banner in Canvas.
  A rendered GPUI test clicks Cancel and Paste against a real local shell, then
  verifies cancelled text is not sent, confirmed multiline input executes, and
  terminal search finds the resulting output.
- Live Canvas local-shell tests verify Cmd-C selection copy, Cmd-V guarded-paste
  cancellation, Shift-PageUp/PageDown scrollback, and xterm mouse-report bytes.
  A zoom test changes the Canvas from 0.5 to 2.0, verifies the computed grid
  grows, and confirms both resize commands inside the child PTY with `stty size`.
- Rendered GPUI input tests send a wheel event through the terminal surface and
  verify scrollback changes without moving the Canvas. A rendered resize-handle
  drag grows the node, updates live terminal rows and columns, and persists the
  new geometry.
- Entering Split with more than four sessions preserves hidden live sessions;
  a hidden-session Canvas tab cannot be merged into another Split and orphan
  those sessions. Active terminal closure uses a rendered confirmation flow.
- Local and SSH canvas nodes reuse `SessionPane` requests and persistence.
- Persistent local terminal requests round-trip through workspace restore.
  A live local tmux test records the inner shell PID, disconnects the first PTY,
  confirms the session remains, reattaches through a second PTY with the same
  PID, and kills the named session through the runtime control path. A rendered
  Add-menu test covers the same creation and cleanup path; missing installations
  receive platform-specific guidance without creating a broken pane.
- A real Docker SSH server shutdown leaves the disconnected SSH pane and its
  canvas node in place, and the production host-identity guard rejects an
  executable local-to-SSH edge.
- Structured agents run through bounded local child-process or remote non-PTY
  SSH transports. Real SSH integration covers independent stdout/stderr,
  stdin, cancellation, exit status, and the legal EOF-before-exit-status
  ordering used by OpenSSH.
- Interactive launch arguments remain literal locally and quoted at the one SSH
  shell boundary; unsafe provider bypass flags are rejected.
- Remote interactive bootstrap generation checks executable presence, version
  command success, and working-directory access before launch. Spaces, quotes,
  and shell metacharacters remain single-quoted at the SSH boundary.
- Structured Codex handshake, request correlation, streaming, approval,
  cancellation, malformed JSON, unknown fields, and abrupt process behavior use
  a deterministic fake server.
- Claude and Gemini documented stream shapes normalize through deterministic
  fixtures and a real local child fixture without provider/API use.
- Remote structured integration covers headless output normalization, Codex's
  bidirectional app-server protocol, provider installation guidance, TERM
  cancellation, and reviewed same-host context delivery.
- Structured process exits report a final failed, cancelled, or disconnected
  lifecycle event in deterministic order instead of leaving a node running.
- Structured input lines, diagnostics, command queues, and displayed transcripts
  are hard-bounded; saturated cancellation and command paths remain nonblocking.
- Context is byte-, line-, and message-bounded, redacted, marked untrusted,
  reviewable, and not persisted. Metadata labels are normalized before entering
  the handoff envelope.
- Official provider installations whose version checks fail are reported as
  unusable with actionable guidance instead of being treated as available.
- Dependency scheduling covers cycle rejection, topological readiness,
  concurrency two, transitive blocked dependants, repeat runs, and strict
  workspace isolation. Closing a workspace drops its structured runtimes and
  stops only that workspace's scheduler.
- Codex process ownership clears exited child IDs and terminates an active
  app-server when its session handle is dropped.
- Managed worktrees persist ownership and active/complete/kept lifecycle state,
  report changed paths and a diff summary, and refuse active, dirty, committed,
  unregistered, or out-of-root cleanup.
- Canvas menus and confirmation overlays are constrained to 90% of the viewport
  width so their fixed content cannot overflow narrow windows.
- Off-screen node culling avoids terminal snapshot work without stopping
  runtimes. Keyboard node selection pans the selected node into view.
- Restored structured nodes remain inert until the rendered `Restart` action is
  invoked; the restart path is covered with a real fake provider process.

## Manual results available

The repository owner previously verified the SSH tmux flow against the Docker
fixture: `$TMUX` was populated, session `manual-startup-test` survived client
disconnect, `manual-startup-count` contained exactly one `startup-ran` line after
reattach, and a second SSH client attached to the same session.

## Manual release checks still required

These checks require a human-visible app, provider accounts, hardware, or remote
hosts and were not claimed by automation:

- human-visible pointer selection/copy, xterm mouse application, and the visual
  feel of drag, resize, wheel scrollback, and guarded paste at multiple zoom
  levels;
- restart and visual restore of a populated canvas;
- eight simultaneously active output-producing panes and Retina/multi-monitor
  responsiveness;
- live successful Claude Code and Gemini structured calls with authorized test
  accounts;
- password SSH, jump-host, forwarding, and disconnect/reconnect in Canvas mode;
- visual approval cards, worktree manager actions, context preview editing, and
  cross-host rejection copy;
- visual tmux close choices; the underlying kill-session execution is covered
  against both the Docker SSH fixture and a real local tmux server;
- native screen-reader semantics cannot be validated with the published GPUI
  0.2.2 resolved by this branch. Upstream GPUI `main` now has AccessKit roles
  and labels, but adopting them requires a coordinated GPUI/gpui-component
  migration; visible labels, tooltips, and keyboard reveal behavior are covered
  instead.

Use the reviewer smoke test in [agent-canvas.md](agent-canvas.md). Do not use a
production repository or paid provider account for the first pass.
