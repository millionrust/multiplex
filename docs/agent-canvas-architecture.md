# Native Agent Canvas Architecture

Status: Accepted for implementation

Date: 2026-07-15

## Context

TermiRust currently renders each workspace as a recursive split tree. Each leaf
references a `SessionPane`, and each pane owns one local or SSH terminal
runtime. Workspace restoration serializes restorable connections and maps the
split tree to pane indices.

The agent-canvas feature adds a spatial workspace for terminals and coding
agents. It must preserve the existing split layout and terminal runtime rather
than introducing a second terminal implementation.

NodeTerm was evaluated only as product research. Its BUSL-1.1 license limits
competitive embedded and standalone use, so this implementation is clean-room
and does not copy or depend on NodeTerm code or assets.

## Decision

### Workspace ownership

`WorkspaceTab` remains the owner of live pane identifiers. It gains a persisted
layout mode and runtime canvas state alongside the existing `SplitNode`.

- `SplitNode` remains authoritative in Split mode.
- Canvas state owns node geometry, viewport state, and graph edges.
- Terminal canvas nodes reference existing `SessionPane` identifiers.
- Switching layout does not recreate or reconnect a pane.
- Existing saved workspaces default to Split mode.

### Rendering

Interactive nodes are ordinary GPUI elements. Existing terminal surfaces are
rendered inside those elements so keyboard, selection, paste, scrolling, mouse
reporting, and focus behavior remain on the established path.

Graph edges use GPUI's low-level `canvas`, `PathBuilder`, and
`Window::paint_path` APIs and render behind nodes. Geometry and viewport
transforms remain pure functions with unit tests.

### Agent boundaries

Agents support two explicit backends:

- Interactive PTY: the normal provider CLI runs in an existing terminal pane.
- Structured: an official machine-readable provider protocol emits normalized
  lifecycle, message, tool, approval, and completion events.

Provider adapters normalize events before UI or orchestration consumes them.
Terminal output is not screen-scraped to infer agent completion.

Codex structured integration uses the official app-server protocol. Claude
Code and Gemini use their official structured/headless interfaces. Arbitrary
custom CLIs remain interactive unless they implement a registered structured
protocol. Groq is treated as a future API-backed adapter, not as an assumed
standard CLI.

### Orchestration and context

Context and dependency edges are separate typed relationships.

- Context handoff is directed, bounded, redacted, previewed, and explicitly
  confirmed before sending.
- Dependency edges form a validated DAG.
- Structured provider events drive task state.
- Approval and cancellation remain explicit user actions.
- Automatic cross-host context transport is outside v1.

### Concurrent writes

Write-capable agents use separate Git worktrees by default. Worktree discovery
uses Git's stable porcelain format. Cleanup refuses dirty worktrees and never
uses force by default.

### SSH and persistence

Remote interactive nodes reuse `ConnectRequest`, `SessionPane`, workspace
restore, TOFU host keys, jump hosts, forwarding, keepalive, and existing tmux
persistence. Canvas placement never controls tmux directly. Closing a
persistent node distinguishes detaching the view from killing the session.

### Security

- Process launches use executable and argument arrays, never shell strings.
- Provider bypass/yolo modes are disabled by default.
- Secrets remain in the existing credential store and are not serialized in
  canvas state, transcripts, logs, or context snapshots.
- Hook installation requires a preview, explicit consent, ownership markers,
  and exact rollback.
- Provider output and handed-off context are untrusted data.
- Destructive worktree and tmux actions require explicit confirmation.

## Consequences

The canvas can evolve without destabilizing the split renderer or terminal
runtimes. Structured provider integration has more implementation cost than
terminal presets, but it gives reliable status, approval, cancellation, and
orchestration semantics.

The initial implementation will contain both persisted and runtime mappings
because saved workspaces refer to panes by index while live workspaces refer to
panes by stable runtime IDs. Validation must reject duplicate node IDs, invalid
geometry, dangling edges, and dependency cycles.

## Rejected alternatives

### Embed or port NodeTerm

Rejected because it conflicts with TermiRust's native architecture and creates
licensing risk.

### Replace split layout with a canvas

Rejected because split layout is established user behavior and remains more
efficient for many terminal workflows.

### Make each canvas node own a new terminal runtime

Rejected because it duplicates session state and would break mode switching,
restore, SSH features, and tmux behavior.

### Infer agent state from terminal text

Rejected because prompts and output vary by provider, shell, theme, locale,
and version. Structured events or explicitly installed hooks are required for
reliable orchestration.

### Run concurrent writers in one checkout

Rejected as the default because agents can overwrite files, contend on the
index, and produce ambiguous completion state. Users may explicitly choose a
shared directory for read-only or intentionally coordinated work.
