# Agent Canvas

The Agent Canvas is a native TermiRust workspace layout for local terminals,
saved SSH hosts, and coding-agent sessions. It reuses the existing terminal,
SSH, restore, and tmux runtimes; changing layout does not reconnect a pane.

## Start a canvas

1. Open a local terminal or connect to a saved host.
2. In the workspace header, choose `Canvas` instead of `Split`.
3. Select `Add` to add a local terminal, saved SSH host, Codex, Claude Code,
   Gemini CLI, or a custom executable.
4. Drag a node by its header. Drag its lower-right handle to resize it.
5. Pan empty space and use the zoom and fit buttons in the canvas toolbar.

Use `Cmd+Shift+Arrow` on macOS (the platform secondary modifier elsewhere) to
cycle node selection without consuming ordinary terminal arrow keys.

Node positions, sizes, collapsed state, links, and viewport are persisted with
the workspace. A restored structured node does not silently launch a process;
select `Restart` inside that node.

## Agent modes

`Interactive terminal` launches the provider's normal TUI in an existing local
or SSH terminal pane. It is the correct mode when the user wants full manual
control. Interactive state is reported as terminal connectivity, not guessed
from provider output.

`Structured` uses machine-readable provider output and normalized lifecycle
events. Codex uses `codex app-server`. Claude Code and Gemini CLI use their
official streaming headless modes. Structured mode currently runs locally.

TermiRust never installs a provider CLI. The creation panel reports the resolved
executable or gives provider-specific installation guidance. Use `Check again`
after installing it outside the app.

Supported defaults intentionally exclude Codex danger/full-access options,
Claude permission bypass, Gemini `--yolo`, and shell evaluation of custom CLI
arguments. Custom executables receive an argument array and remain interactive.

## Isolated worktrees

Write-capable local agents default to `Isolated worktree`. TermiRust creates a
linked Git worktree and a unique `termirust/agent/...` branch under its app data
directory. `Shared directory` and `Read only` require an explicit selection.

The `Worktrees` toolbar action lists every app-managed worktree, including one
whose agent node has been closed. Its actions can:

- inspect whether the worktree is dirty or contains commits after its base;
- copy its path;
- open a terminal in it;
- remove it only when it is unused, clean, and has no later commits.

TermiRust does not force-remove dirty worktrees, delete useful branches, or
manage repositories with submodules automatically. Close all canvas agents and
terminals using a worktree before cleanup.

## Context and dependencies

The arrow action on two nodes creates a directed context link. Select the source
first and target second. Linking does not send data. Select the target and use
`Review context`; TermiRust creates a bounded, redacted preview that can be
edited or cancelled. Structured targets receive the confirmed prompt directly.
Interactive targets use the existing guarded multiline-paste confirmation.

The dependency action creates a directed prerequisite. Queue a prompt in each
structured target and select `Run DAG`. Cycles are rejected, at most two ready
tasks run concurrently, and failed or cancelled prerequisites block dependants.
Incomplete dependency runs are never resumed automatically after restart.

Context and dependency execution is restricted to nodes on the same local or
SSH host. Automatic cross-host context transport is not implemented.

## Remote agents and tmux

Remote interactive agents use saved SSH host settings, including credentials,
jump hosts, forwarding, keepalive, TOFU host keys, startup directory, and tmux
persistence. On connection, the remote startup command checks for the selected
provider executable and prints installation guidance if it is missing. Nothing
is installed or uploaded automatically.

tmux is owned by the existing SSH persistence feature, not by the canvas. A
canvas move or layout switch cannot restart tmux. Closing the SSH client leaves
a persistent remote tmux session running so a later connection can reattach.
The existing `detach_others` option remains opt-in.

## Security boundaries

- Provider output and handed-off context are untrusted data.
- Context is byte- and line-bounded and common credential patterns are redacted.
- Context previews and provider transcripts are not stored in `state.json`.
- Credentials remain in the existing credential store and saved host model.
- Structured approval cards provide one-time allow or deny; approvals are not
  remembered automatically.
- Worktree cleanup canonicalizes paths and refuses targets outside the managed
  app directory.
- Structured channels and displayed transcripts are bounded.

See [agent-canvas-threat-model.md](agent-canvas-threat-model.md) for the abuse
cases and mitigations and [agent-canvas-architecture.md](agent-canvas-architecture.md)
for ownership decisions. Recorded automated and manual release coverage is in
[agent-canvas-qa.md](agent-canvas-qa.md).

## Current limits

- The validated v1 geometry target is 20 visible nodes and 40 edges. The pure
  layout path is automated; eight simultaneous live-output panes still require
  manual profiling on the release machine.
- Structured agents are local. Remote agents use interactive terminal mode.
- Cross-host links are visible only if present in imported state and are refused
  at review/run time.
- Groq API agents, ACP, mobile canvas editing, live team canvases, arbitrary
  autonomous loops, and automatic merge/rebase are deferred.

## Reviewer smoke test

1. Run `cargo test -q` and `cargo run`.
2. Open a local terminal, switch to Canvas, and add another local terminal.
3. Drag, resize, collapse, zoom, fit, switch to Split, and return to Canvas.
4. Add an interactive Custom CLI using `/bin/echo` with arguments containing
   spaces and quotes; verify the text is literal.
5. Add two structured provider nodes only when those CLIs are installed and
   authenticated; queue tasks and create one dependency.
6. Create a context link, review the redactions, edit it, and confirm delivery.
7. In a disposable Git repository, launch two isolated agents and verify their
   branch and path differ. Close them, then inspect and remove only a clean one.
8. Restart TermiRust and verify geometry and links restore while structured
   processes remain stopped until `Restart` is selected.

Do not use a production repository for the first manual worktree test.
