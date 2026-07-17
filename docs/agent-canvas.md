# Agent Canvas

The Agent Canvas is a native TermiRust workspace layout for local terminals,
saved SSH hosts, and coding-agent sessions. It reuses the existing terminal,
SSH, restore, and tmux runtimes; changing layout does not reconnect a pane.

## Start a canvas

1. Open a local terminal or connect to a saved host.
2. In the workspace header, choose `Canvas` instead of `Split`.
3. Select `Add` to add a local terminal, saved SSH host, Codex, Claude Code,
   Gemini CLI, or a custom executable.
4. Select `Project: <folder>` to choose the working directory for new local
   terminals and coding agents. Existing running terminals are not restarted or
   moved when the project folder changes.
5. Drag a node by its header. Double-click the header to rename it. Drag its
   lower-right handle to resize it.
6. Pan empty space and use the zoom and fit buttons in the canvas toolbar. The
   bottom-right overview shows every node and the visible viewport; select a
   point in it to jump there.
7. Use the toolbar undo and redo actions for node moves, resizes, renames, and
   collapse changes. These actions only restore layout metadata; they never
   close running terminals, remove nodes, or roll back agent work.

Right-click empty canvas space to open the same terminal and agent add menu.

Use `Cmd+Shift+Arrow` on macOS (the platform secondary modifier elsewhere) to
cycle node selection without consuming ordinary terminal arrow keys.
Use `Links` to inspect every directed context/dependency edge, disable it, or
delete it without closing either connected node.

Closing a persistent SSH node asks whether to detach it from the canvas, keep a
disconnected node for later reconnection, or permanently kill its tmux session.
Killing requires a second confirmation and uses a separate SSH control channel.
Closing any other connected terminal asks for confirmation before ending its
active local process or SSH connection.

Node positions, sizes, collapsed state, links, and viewport are persisted with
the workspace. The selected project folder is persisted too, and restored local
terminals and newly created agents continue to use it. A restored structured node
does not silently launch a process; select `Restart` inside that node.

When more than four sessions exist, switching to Split opens a chooser. Pick up
to four sessions to display; every unselected session keeps running and remains
available when you return to Canvas.

## Agent modes

`Interactive terminal` launches the provider's normal TUI in an existing local
or SSH terminal pane. It is the correct mode when the user wants full manual
control. Interactive state is reported as terminal connectivity, not guessed
from provider output.

`Structured` uses machine-readable provider output and normalized lifecycle
events. Codex uses `codex app-server`. Claude Code and Gemini CLI use their
official streaming headless modes. Structured sessions run either locally or
on a selected saved SSH host. Codex supports in-session approvals and thread
continuity. The one-shot Claude and Gemini headless adapters support
cancellation but do not claim interactive approval responses or session resume.

TermiRust never installs a provider CLI. The creation panel reports the resolved
executable or gives provider-specific installation guidance. An official
provider executable whose version check fails is shown as unusable with the
captured diagnostic. Use `Check again` after installing or repairing it outside
the app.

Supported defaults intentionally exclude Codex danger/full-access options,
Claude permission bypass, Gemini `--yolo`, and shell evaluation of custom CLI
arguments. Custom executables receive an argument array and remain interactive.

## Isolated worktrees

Write-capable local agents default to `Isolated worktree`. TermiRust creates a
linked Git worktree and a unique `termirust/agent/...` branch under its app data
directory. `Shared directory` and `Read only` require an explicit selection.

The `Worktrees` toolbar action lists every app-managed worktree, including one
whose agent node has been closed. Its actions can:

- inspect dirty/committed state, changed-path count, and a Git diff summary;
- copy its path;
- open a terminal in it;
- mark its task complete while keeping the branch and files;
- explicitly mark the worktree and branch to keep;
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
provider executable, verifies that its version command succeeds, and validates
the selected working directory exists with read and execute access. A failed
check prints a specific message and leaves the user in a normal shell. Nothing
is installed or uploaded automatically.

tmux is owned by the existing SSH persistence feature, not by the canvas. A
canvas move or layout switch cannot restart tmux. Closing the SSH client leaves
a persistent remote tmux session running so a later connection can reattach.
The existing `detach_others` option remains opt-in.

Remote structured Codex, Claude Code, and Gemini sessions use a non-PTY SSH exec
channel and the saved profile's authentication, jump-host chain, TOFU pinning,
keepalive, and explicit environment entries. Before every process launch,
TermiRust checks the provider executable and version and verifies the working
directory is readable and searchable. Failures remain in the node with the
provider's installation or repair guidance. TermiRust does not install or
upload anything.

tmux startup actions are deliberately disabled for structured SSH processes;
their lifecycle is owned by the structured adapter. Automatic isolated Git
worktrees are local-only, so a remote structured agent must use `Read only` or
`Shared directory`. Context and dependencies work between agents mapped to the
same saved host and are refused across different hosts.

## Security boundaries

- Provider output and handed-off context are untrusted data.
- Context is byte-, line-, and message-bounded and common credential patterns
  are redacted. Source labels are normalized before entering the handoff.
- Context previews and provider transcripts are not stored in `state.json`.
- Credentials remain in the existing credential store and saved host model.
- Structured approval cards provide one-time allow or deny; approvals are not
  remembered automatically. They appear only when the provider protocol exposes
  an approval request; v1 currently exercises this through Codex app-server.
- Worktree cleanup canonicalizes paths and refuses targets outside the managed
  app directory.
- Structured channels and displayed transcripts are bounded.
- Distant off-screen nodes are not rendered, so they do not create terminal
  snapshots; their processes, canvas geometry, and links remain alive.

See [agent-canvas-threat-model.md](agent-canvas-threat-model.md) for the abuse
cases and mitigations and [agent-canvas-architecture.md](agent-canvas-architecture.md)
for ownership decisions. Recorded automated and manual release coverage is in
[agent-canvas-qa.md](agent-canvas-qa.md).

## Current limits

- The validated v1 geometry target is 20 visible nodes and 40 edges. The pure
  layout path is automated; eight simultaneous live-output panes still require
  manual profiling on the release machine.
- Automatic isolated worktree creation is local-only. Remote structured agents
  operate in a user-selected existing remote directory.
- Node position and dimensions scale with Canvas zoom, while terminal text keeps
  the configured readable font size. PTY rows and columns follow the effective
  on-screen node body. Nodes retain minimum interactive dimensions at low zoom.
- GPUI 0.2.2 does not expose native per-element accessibility semantics.
  Canvas controls therefore provide visible labels or tooltips and keyboard
  node navigation, but native screen-reader labels require framework support.
- Cross-host links are visible only if present in imported state and are refused
  at review/run time.
- Groq API agents, ACP, mobile canvas editing, live team canvases, arbitrary
  autonomous loops, and automatic merge/rebase are deferred.

## Reviewer smoke test

1. Run `cargo test -q` and `cargo run`.
2. Open a local terminal, switch to Canvas, and add another local terminal.
3. Drag, resize, collapse, undo, redo, zoom, fit, use the overview to jump, then
   switch to Split and return to Canvas.
4. Add an interactive Custom CLI using `/bin/echo` with arguments containing
   spaces and quotes; verify the text is literal.
5. Add two structured provider nodes only when those CLIs are installed and
   authenticated; queue tasks and create one dependency.
6. Create a context link, review the redactions, edit it, and confirm delivery.
7. In a disposable Git repository, launch two isolated agents and verify their
   branch and path differ. Mark one complete, mark the other to keep, then
   inspect and remove only a clean unused one.
8. Restart TermiRust and verify geometry and links restore while structured
   processes remain stopped until `Restart` is selected.
9. On a disposable saved SSH host, choose `Structured`, `Read only`, and an
   installed provider. Verify completion, cancellation, missing-provider
   guidance, and same-host context delivery.

Do not use a production repository for the first manual worktree test.

Live provider adapter smokes are opt-in and ignored by the normal suite. Their
exact commands, prerequisites, and latest results are recorded in
[agent-canvas-qa.md](agent-canvas-qa.md). Run them only with disposable prompts
and an account whose quota and organization policy permit CLI use.
