# Agent Canvas

The Agent Canvas is a native TermiRust workspace layout for local terminals,
saved SSH hosts, and coding-agent sessions. It reuses the existing terminal,
SSH, restore, and tmux runtimes; changing layout does not reconnect a pane.

## Start a canvas

1. Open a local terminal or connect to a saved host.
2. In the workspace header, choose `Canvas` instead of `Split`.
3. Select `Add` to add a local terminal, saved SSH host, Codex, Claude Code,
   Gemini CLI, a custom executable, a sticky note, or a group frame.
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

## SSH fleets

For a team that primarily works over SSH, organize related saved hosts into a
host group such as `Production`, `Staging`, or `Customers`. A group with at
least two visible hosts has an `Open Fleet` action in the host library. It
validates every host first, then opens all of them as independent SSH terminal
nodes in one persisted Canvas workspace. Existing credentials, jump hosts,
port forwards, environment, startup settings, and persistent tmux settings are
reused; the fleet path does not create a second connection configuration.

To add a fleet to an existing Canvas, select `Add` and choose the group under
`Host Groups`. Hosts already present with the same username, address, and port
are skipped so repeating the action does not create duplicates.

The `Fleet connected/total` toolbar action opens the fleet panel. It shows each
SSH endpoint, live connection state, and persistent tmux session name. From the
panel the user can:

- reconnect every offline or failed host without restarting clients already
  connected or connecting;
- reconnect or disconnect one host while keeping its canvas node;
- enable the workspace's existing Broadcast Input mode; and
- disconnect all active SSH clients after an explicit confirmation.

Broadcast Input sends typed and pasted bytes to every connected terminal pane
in the workspace, so review the target list before enabling it. `Disconnect
All` closes TermiRust's SSH clients and disables automatic reconnect for those
panes. It does not kill persistent tmux sessions on the remote hosts; reconnect
reattaches through each saved host's normal tmux bootstrap. Closing the fleet
panel has no effect on any connection.

Sticky notes keep project instructions beside the work they describe. Select
`Edit` to change a note and use its menu to cycle the note color. A note can be
the source of a reviewed context link, so its bounded, redacted text can be sent
to a terminal or agent; notes cannot run or become dependency targets.

`Group Frame` creates a visual work area. If a non-group node is selected, the
new frame wraps it immediately. Drag any node into or out of a frame to update
membership, and drag the frame header to move all members together. Removing a
frame asks for confirmation and keeps its member nodes. Groups do not execute,
receive context, or participate in dependency runs.

`Persistent Local Terminal` attaches to an app-named local tmux session with
tmux's attach-or-create behavior. The session and its processes survive closing
the node or TermiRust, and workspace restore reattaches to the same name.
Closing one of these nodes uses the same explicit Detach, Disconnect, or
confirmed Kill choices as a persistent SSH node. If local tmux is unavailable,
TermiRust does not open a broken node; it shows platform-specific installation
guidance instead. Ordinary `Local Terminal` remains the default and does not
require tmux.

`Files` opens a local project panel for the selected project folder. It provides
folder navigation, UTF-8 text viewing and editing, explicit Save/Revert actions,
Git status, and the unstaged diff for the selected file. The panel refuses files
outside the selected project root (including escaping symlinks), binary files,
and files larger than 1 MB. Save also refuses to overwrite a file that changed
on disk after it was opened, which protects concurrent agent edits. Git controls
are inspect/copy-only; they never stage, discard, commit, or execute a shell
command.

`Activity` summarizes every structured agent in the active canvas: running,
queued, attention, and unread-output counts. Agents that need approval, are
blocked, failed, or disconnected are listed first. Each row also shows its
enabled incoming context and dependency links; selecting a row pans to that
node, focuses it, and clears its unread marker. Agent node headers keep a
warning icon for actionable states and a bell for unseen output.

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
- iOS and Android currently provide direct SSH terminal access to saved remote
  hosts and can reattach those hosts' named tmux sessions. They do not expose or
  control desktop-local canvas nodes, project files, links, or dependency runs.
  That requires an authenticated shared canvas protocol or gateway and is not
  implied by terminal continuity.

## Reviewer smoke test

1. Run `cargo test -q` and `cargo run`.
2. Create two disposable saved SSH hosts in one group. Enable persistent tmux
   on one, select `Open Fleet`, and verify both terminal nodes and the Fleet
   panel appear. Disconnect and reconnect the tmux host and verify its remote
   process continues.
3. Enable Broadcast Input only on disposable shells, send a harmless marker,
   verify both receive it, then use the confirmed Disconnect All action and
   reconnect the fleet.
4. Open a local terminal, switch to Canvas, and add another local terminal.
5. Drag, resize, collapse, undo, redo, zoom, fit, use the overview to jump, then
   switch to Split and return to Canvas.
6. Add a sticky note, edit and recolor it, wrap it in a group, move the group,
   and link the note as reviewed context to a terminal. Remove the group and
   verify the note remains.
7. Add an interactive Custom CLI using `/bin/echo` with arguments containing
   spaces and quotes; verify the text is literal.
8. Add two structured provider nodes only when those CLIs are installed and
   authenticated; queue tasks and create one dependency.
9. Create a context link, review the redactions, edit it, and confirm delivery.
10. In a disposable Git repository, launch two isolated agents and verify their
   branch and path differ. Mark one complete, mark the other to keep, then
   inspect and remove only a clean unused one.
11. Restart TermiRust and verify geometry, groups, notes, links, and fleet nodes
   restore while structured processes remain stopped until `Restart` is
   selected.
12. On a disposable saved SSH host, choose `Structured`, `Read only`, and an
   installed provider. Verify completion, cancellation, missing-provider
   guidance, and same-host context delivery.

Do not use a production repository for the first manual worktree test.

Live provider adapter smokes are opt-in and ignored by the normal suite. Their
exact commands, prerequisites, and latest results are recorded in
[agent-canvas-qa.md](agent-canvas-qa.md). Run them only with disposable prompts
and an account whose quota and organization policy permit CLI use.
