# TermiRust Native Agent Canvas and Orchestration - Implementation Goal

## 1. Role and mission

You are a senior Rust, GPUI, terminal, SSH, and agent-integration engineer working inside the existing TermiRust repository.

Implement a production-quality native **Agent Canvas** inside TermiRust. The canvas must let a user place multiple local terminals, SSH terminals, and coding-agent sessions on a pan-and-zoom workspace, connect them with explicit context or dependency links, and coordinate coding work without damaging the existing terminal product.

This is not a request to clone or embed NodeTerm. Use NodeTerm only as product research. Build an original, clean-room implementation that follows TermiRust's native Rust/GPUI architecture, visual language, state model, security model, and existing terminal runtime.

The result must feel like one coherent TermiRust feature, not a second application inserted into it.

## 2. Client outcome

The client should be able to:

1. Open any TermiRust workspace in either `Split` or `Canvas` layout.
2. Add local terminals, saved SSH hosts, and coding-agent nodes to the canvas.
3. Drag and resize nodes without breaking terminal text selection, scrolling, keyboard input, or mouse reporting.
4. Pan and zoom a large workspace while retaining a stable spatial layout after restart.
5. Run Claude Code, Codex, Gemini CLI, or a safe custom CLI in agent nodes.
6. See reliable agent state such as running, waiting for approval, completed, failed, disconnected, or cancelled.
7. Connect agents with explicit context links and send bounded, inspectable context between them.
8. Run write-capable agents in separate Git worktrees by default so concurrent agents do not overwrite each other's work.
9. Use local and remote SSH terminals in the same product, reusing the existing tmux persistence feature.
10. Understand and control every security-sensitive action. No agent may silently receive credentials, bypass permissions, install hooks, delete work, or execute another agent's output.

## 3. Non-negotiable engineering rules

### 3.1 Preserve the existing product

- Read `AGENTS.md` and the relevant source files before editing.
- Preserve all existing host library, SSH, local PTY, split pane, SFTP, port forwarding, jump host, snippets, broadcast input, terminal search, paste confirmation, workspace restore, tmux persistence, and mobile-related behavior.
- Do not rewrite `SessionPane`, `WorkspaceTab`, `SplitNode`, `TerminalState`, SSH runtime, or local PTY runtime merely to make the canvas easier.
- Extend existing types and flows conservatively.
- Do not remove, rename, or reformat unrelated code.
- Never discard uncommitted changes made by the user.
- Do not alter `.gitignore` unless the feature genuinely requires it and the existing user change has first been understood.
- Existing split-pane workspaces must deserialize and render exactly as before.
- The default for old saved state must be `Split`, not `Canvas`.
- The existing maximum of four split panes applies only to split layout. Canvas capacity must have its own measured limit and must not silently reuse `MAX_SPLIT_PANES`.

### 3.2 Clean-room requirement

NodeTerm is currently licensed under BUSL-1.1 with restrictions relevant to competing hosted, embedded, or standalone products. Therefore:

- Do not copy NodeTerm source code, tests, CSS, assets, strings, internal type names, or implementation-specific algorithms.
- Do not add NodeTerm as a dependency, submodule, bundled component, or runtime service.
- Do not reproduce its branding or UI pixel-for-pixel.
- Derive requirements only from public behavior and independently design the TermiRust implementation.
- Record any new third-party dependency, its license, and why it is necessary.
- Prefer no new graph/canvas dependency for the first implementation. GPUI already exposes low-level `canvas`, `PathBuilder`, and `Window::paint_path` APIs.

### 3.3 Code quality

- Keep persisted data, runtime state, rendering, process integration, and orchestration in separate ownership boundaries.
- Use typed Rust models and enums. Do not represent important state as loosely interpreted strings.
- Use structured process arguments (`executable` plus `Vec<OsString>` or equivalent), never concatenated shell commands.
- Parse JSON/JSONL with `serde`, not string matching.
- Do not screen-scrape terminal output as the control protocol for a structured agent integration.
- Keep comments short and only where behavior or safety is not self-evident.
- Avoid broad abstractions until at least two real implementations need them.
- Keep UI code out of SSH, PTY, Git, and provider protocol modules.
- Keep provider-specific event types out of canvas rendering code by normalizing them at the adapter boundary.
- All public behavior added in one phase must have tests in the same phase.

### 3.4 Git discipline

- Work only on the current test/feature branch. Confirm the branch before the first edit.
- Inspect `git status` before every commit.
- Stage only files belonging to the current phase.
- Make small, understandable commits after each complete, tested milestone.
- Every commit must build and pass the focused tests for that milestone.
- Use imperative commit messages, for example `feat(canvas): persist canvas workspace state`.
- Do not add `Co-authored-by` or any other co-author trailer.
- Do not amend, squash, force-push, reset, or rewrite history unless explicitly requested.
- Do not commit generated build directories, credentials, logs, temporary worktrees, or provider transcripts.

## 4. Research conclusions that must shape the implementation

### 4.1 NodeTerm evaluation

NodeTerm demonstrates that a spatial terminal workspace can be useful: terminals and agent CLIs live in draggable nodes, tmux preserves sessions, and context links can expose one agent's transcript to another. Its public behavior also exposes several design lessons:

- Spatial placement must avoid immediate node overlap.
- Context links need a clear direction and a visible action; a line alone must not imply that data is continuously shared.
- A terminal preset and a true structured agent integration are different products.
- Agent status should come from structured events or explicit hooks, not prompt/output heuristics.
- Session persistence, canvas persistence, and task orchestration are separate concerns.

TermiRust should adopt those product lessons but implement them independently.

### 4.2 GPUI rendering

The repository currently uses GPUI 0.2.2. This version provides:

- `gpui::canvas(prepaint, paint)` for low-level drawing.
- `PathBuilder::stroke`, `move_to`, `line_to`, `curve_to`, and `cubic_bezier_to`.
- `Window::paint_path` for drawing graph edges.
- Normal GPUI elements and event handlers for node surfaces and controls.

Use ordinary GPUI elements for interactive node bodies and terminal surfaces. Use a low-level GPUI canvas only for non-interactive background details and edges behind nodes. Do not render terminals into a bitmap canvas.

### 4.3 Coding-agent integration strategy

Implement two distinct backends:

1. **Interactive PTY backend**: launches the normal CLI in an existing `SessionPane`. The user sees and operates the real TUI. This is required for the first usable canvas milestone.
2. **Structured backend**: launches or connects to an official machine-readable protocol and produces normalized lifecycle, message, tool, approval, usage, and completion events. This powers reliable orchestration.

Provider direction:

- **Codex**: prefer the official Codex app-server protocol for deep integration. It supports initialization, threads, turns, streaming events, approvals, interruption, and schema generation. Generate schemas for the installed version instead of hand-maintaining guessed JSON. `codex exec --json` may be used for bounded non-interactive jobs, but it is not the primary interactive backend.
- **Claude Code**: use the official Agent SDK when it fits the Rust process boundary, or the official headless CLI mode with `--output-format stream-json` for structured jobs. Official hooks may provide lifecycle signals, but installing or changing hooks requires explicit user consent and a reversible scoped configuration.
- **Gemini CLI**: use official headless `json` or `stream-json` output for structured jobs. Hooks are optional and consent-gated. Never enable `--yolo` by default.
- **Custom CLI**: support safe executable-and-arguments presets as interactive PTY nodes. A custom CLI has status `Interactive` unless the user supplies a separately validated protocol adapter. Do not pretend arbitrary terminal output is structured orchestration.
- **Groq**: Groq is an API/model platform, not a canonical coding CLI equivalent to Codex, Claude Code, or Gemini CLI. A generic custom CLI may launch a user-installed Groq-related executable. A first-party `Groq Agent` must instead be an API-backed adapter using Groq tool calling with a tightly controlled local tool layer. Defer this adapter until after the canvas and structured CLI adapters are stable unless the client explicitly prioritizes it.

### 4.4 Agent Client Protocol

ACP is a public JSON-RPC protocol designed to connect coding agents and clients. It has a Rust library and uses protocol-version and capability negotiation.

- Design TermiRust's normalized agent event model so a future ACP adapter can map into it cleanly.
- Do not make ACP mandatory for the initial release.
- Do not force Codex, Claude, or Gemini through an unofficial adapter when their official structured interface is more direct and better supported.
- If ACP is added later, negotiate `protocolVersion` and capabilities. Never infer wire compatibility from a package version alone.

### 4.5 Git worktrees

Git worktrees provide separate working directories, indexes, and `HEAD` state while sharing repository objects. They are the correct default isolation boundary for concurrent write-capable coding agents.

- Use `git worktree list --porcelain -z` for machine parsing.
- Create a unique branch and worktree for each write-capable agent unless the user explicitly chooses read-only or shared-directory mode.
- Never force-remove a dirty worktree.
- Never delete an agent branch automatically after useful commits exist.
- Provide an inspectable cleanup action and clear dirty/clean status.
- Treat repositories with submodules conservatively because multiple-worktree support has documented limitations.

### 4.6 tmux and SSH

TermiRust already implements tmux persistence for SSH profiles. Reuse that behavior:

- A canvas node is a view and placement record. It does not own a second tmux implementation.
- Closing or hiding a canvas node must distinguish detaching the TermiRust client from killing the tmux session.
- Multiple clients may attach to one tmux session. `detach_others` remains an explicit opt-in because it disconnects other clients.
- Structured remote orchestration is separate from an interactive SSH terminal. It requires the provider CLI or helper to exist on the remote host and must not be silently installed.

## 5. Scope and delivery boundaries

### 5.1 Required client-demo scope

The first complete client-demo release must include:

- Persisted `Split` / `Canvas` workspace layout mode.
- Smooth pan, cursor-anchored zoom, fit-to-content, node drag, and node resize.
- Local terminal and saved SSH host nodes backed by existing `SessionPane` runtimes.
- Agent node creation for Codex, Claude Code, Gemini CLI, and Custom CLI using interactive PTYs.
- Safe executable detection and a useful missing-CLI state.
- Stable node identifiers, selection, focus, z-order, close behavior, and restore.
- Directed context links with an explicit, review-before-send handoff.
- Normalized agent state for at least Codex through its official structured interface.
- Git worktree isolation for write-capable local agent nodes.
- Focused automated tests and a documented manual QA pass.

### 5.2 Complete v1 scope

After the client-demo milestone, v1 adds:

- Structured Claude Code and Gemini CLI adapters.
- Approval cards and cancellation for structured agents.
- Dependency links and a bounded DAG task runner.
- Remote capability checks and same-host remote context handoff.
- Canvas keyboard navigation and accessibility labels.
- Performance validation with the agreed supported node count.

### 5.3 Deferred scope

Defer these until v1 is stable:

- Groq API-backed coding agent.
- ACP adapter.
- Notes, group frames, editor nodes, diff nodes, browser/video nodes, minimap, and source-control panel.
- Cross-device shared live canvases or team multiplayer.
- Cross-host automatic context transport.
- Arbitrary autonomous agent-to-agent loops.
- Visual workflow scripting language.
- tmux control mode.
- Mobile canvas editing. Mobile terminals may remain a separate product surface.

Do not quietly implement deferred features during cleanup or refactoring.

## 6. UX specification

### 6.1 Entering the canvas

- Add a compact segmented layout control to the workspace toolbar: `Split` and `Canvas`.
- Keep the currently selected mode per workspace and persist it.
- Switching modes must not reconnect or recreate sessions.
- The first switch from Split to Canvas should place existing panes in a readable non-overlapping grid.
- Switching back to Split must preserve the prior recursive split layout. If a canvas has more sessions than Split supports, show an explicit chooser for which panes enter Split; do not discard the others.

### 6.2 Empty canvas

The empty state should be operational, not a marketing page. Show a centered add menu or command surface with:

- Local Terminal
- Saved Host
- Claude Code
- Codex
- Gemini CLI
- Custom CLI

The same options must be available from a `+` toolbar menu and the canvas context menu.

### 6.3 Canvas navigation

- Drag empty canvas space with the primary mouse button to pan.
- Trackpad two-finger scrolling pans.
- `Cmd` plus wheel/pinch zooms when GPUI exposes the event reliably; otherwise use wheel zoom only with a clearly consistent modifier.
- Zoom must be anchored at the cursor, not at the top-left corner.
- Clamp zoom to an initial range of `0.35..=2.0` and keep constants centralized.
- Add icon buttons with tooltips for zoom out, reset to 100%, zoom in, and fit to content.
- Double-click empty canvas to create a local terminal only if this does not conflict with existing workspace behavior. Otherwise open the add menu.
- Persist world-space node positions and viewport state.

### 6.4 Node interaction

- A node has a compact header and a content body.
- Dragging begins only from the header or a dedicated drag handle.
- Terminal body events must remain owned by the terminal: keyboard input, text selection, mouse reporting, local scrollback, paste confirmation, search, and context menu.
- Provide resize handles on node edges/corners. Respect minimum terminal dimensions and PTY resize after the visual size stabilizes.
- Clicking a node selects, focuses, and raises it.
- Multi-select is optional for the demo but required before adding group movement.
- Closing a normal nonpersistent terminal asks for confirmation if a process is active.
- Closing a persistent tmux node offers clear choices: `Detach from canvas`, `Disconnect client`, and `Kill tmux session`. Killing is destructive and must be confirmed.
- A minimized/collapsed node keeps its process running and displays status in the header.

### 6.5 Node header

Show only useful operational information:

- Provider or terminal icon.
- Editable node title.
- Host name or `Local`.
- Agent status with text plus color/icon, never color alone.
- Unread/needs-attention indicator.
- Minimal icon actions with tooltips: link, collapse, more, close.

Avoid decorative nested cards, oversized headings, one-color palettes, and text that explains obvious controls.

### 6.6 Placement and overlap

- New nodes must not stack exactly on top of one another.
- Start near the viewport center, then search outward on a deterministic grid for the first non-overlapping position.
- Keep at least a small gutter between nodes.
- A user-dragged position always wins over automatic layout.
- `Fit to content` must include all visible nodes with padding and handle a single-node canvas sensibly.

### 6.7 Links

Use distinct visual and semantic link types:

- `Context` link: directed, solid line, carries a bounded snapshot only when explicitly sent or when a reviewed workflow step runs.
- `Dependency` link: directed, visually distinct, means target waits for source completion.
- `Reference` link: optional later, dotted and non-executable.

Requirements:

- Edge direction must be visible.
- Hover or selection reveals source, target, type, and enabled state.
- Creating a context link must not immediately send data.
- Dependency links must reject cycles before persistence.
- Deleting an edge never deletes nodes or sessions.
- Edges render behind nodes and remain hit-testable through a wider invisible interaction region if GPUI supports it cleanly.

### 6.8 Agent creation flow

The creation panel must include:

- Provider.
- Local or saved SSH host.
- Working directory/repository.
- Interactive or structured mode when supported.
- Permission policy with a safe default.
- Worktree isolation toggle, enabled by default for write-capable local agents in Git repositories.
- Optional initial prompt, shown in full before launch.

Detect the selected executable and version before launch. If missing, show exact provider-specific installation guidance and a `Check again` action. Do not open Mail, execute copied shell text, or install software automatically.

## 7. Architecture and ownership

### 7.1 Core principle

`WorkspaceTab` continues to own live panes and workspace-level UI state. `SessionPane` continues to own each local or SSH terminal runtime. Canvas state owns only spatial layout, graph relationships, and references to those sessions. Agent services own provider processes and structured events.

No canvas node should contain a second copy of terminal state.

### 7.2 Recommended module structure

Follow the existing app module style and introduce only the modules needed for real complexity:

```text
src/
  agents.rs                     # top-level agent service and normalized public types
  agents/
    adapter.rs                  # AgentAdapter trait and capability model
    process.rs                  # safe child-process and JSONL lifecycle helpers
    codex.rs                    # official app-server integration
    claude.rs                   # official structured Claude integration
    gemini.rs                   # official structured Gemini integration
    custom.rs                   # interactive custom CLI launch validation
    orchestrator.rs             # state machine, approvals, cancellation, DAG execution
    context.rs                  # bounded context snapshots and redaction
    worktree.rs                 # Git worktree discovery, create, status, cleanup
    protocol.rs                 # normalized events, requests, errors, provider versions
  ui/app/
    canvas.rs                   # workspace canvas entry point and composition
    canvas/
      geometry.rs               # world/screen transforms, placement, hit-test math
      interaction.rs            # pan, zoom, drag, resize, selection state
      nodes.rs                  # node shell/header/body composition
      edges.rs                  # GPUI path generation and edge interactions
      toolbar.rs                # layout and canvas command controls
```

Do not move existing files into these directories unless there is a concrete compilation or ownership reason.

Persisted canvas and agent configuration types belong in `src/models.rs` or a small serializable model module re-exported by it, matching the repository's established storage pattern. GPUI entities, focus handles, subscriptions, tasks, channels, and live process handles must never be serialized.

### 7.3 Runtime boundaries

```text
GPUI canvas UI
  -> canvas controller/state
     -> existing WorkspaceTab and SessionPane terminal runtimes
     -> AgentService
        -> AgentAdapter (Codex/Claude/Gemini/Custom)
        -> Orchestrator
        -> WorktreeManager
        -> ContextBuilder

AgentAdapter
  -> structured child process or protocol
  -> normalized AgentEvent stream

Orchestrator
  -> consumes normalized events
  -> requests user approvals through UI
  -> never manipulates GPUI elements directly
```

The UI may subscribe to service events. Service code must not import `ui::app`.

## 8. Data model

Use the following as a design target, not as permission to paste types without adapting them to existing conventions.

### 8.1 Workspace mode and canvas persistence

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLayoutMode {
    #[default]
    Split,
    Canvas,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasState {
    #[serde(default = "current_canvas_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub viewport: SavedCanvasViewport,
    #[serde(default)]
    pub nodes: Vec<SavedCanvasNode>,
    #[serde(default)]
    pub edges: Vec<SavedCanvasEdge>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasViewport {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}
```

Provide explicit, finite defaults. Reject or repair `NaN`, infinity, zero/negative node sizes, duplicate identifiers, dangling edges, and out-of-range zoom during load.

### 8.2 Stable node identity

- Use an opaque stable `CanvasNodeId`, serialized as a string.
- Generate IDs centrally and test uniqueness.
- Never use vector position, pane index, display title, provider name, host ID, or tmux session name as node identity.
- Runtime mappings may associate `CanvasNodeId` with `pane_id: u64`.
- Persisted terminal-node mappings may use a pane index only inside the same `SavedWorkspace` record if that matches the current restore model. Validate the index on load.

Suggested persisted node fields:

```rust
pub struct SavedCanvasNode {
    pub id: CanvasNodeId,
    pub kind: SavedCanvasNodeKind,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedCanvasNodeKind {
    Terminal { pane_index: usize },
    Agent {
        pane_index: Option<usize>,
        definition: SavedAgentDefinition,
    },
}
```

Do not persist provider transcripts in `state.json`. Persist only the minimum IDs needed to ask a provider to resume a conversation, and only when the provider officially supports it.

### 8.3 Edges

```rust
pub struct SavedCanvasEdge {
    pub id: CanvasEdgeId,
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    pub kind: CanvasEdgeKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub context_policy: Option<SavedContextPolicy>,
}

#[serde(rename_all = "snake_case")]
pub enum CanvasEdgeKind {
    Context,
    Dependency,
}
```

Validate:

- Source and target exist.
- No self-link unless a future link type explicitly permits it.
- No duplicate semantic link between the same endpoints.
- Dependency graph remains acyclic.
- Context policy has bounded limits.

### 8.4 Agent definition

```rust
pub struct SavedAgentDefinition {
    pub provider: AgentProvider,
    pub backend: AgentBackendKind,
    pub location: AgentLocation,
    pub working_directory: Option<String>,
    pub executable_override: Option<String>,
    pub arguments: Vec<String>,
    pub permission_policy: AgentPermissionPolicy,
    pub worktree: SavedWorktreePolicy,
}
```

Rules:

- Store arguments as an array, not one shell string.
- Do not store access tokens, passwords, private keys, cookies, or entire environment maps here.
- `AgentLocation` should reference `Local` or an existing saved host/restorable connection without duplicating credentials.
- Environment variables that contain secrets must reference the OS credential store.
- A provider may reject unsupported backend/location combinations before creating a process.

### 8.5 Normalized runtime state

```rust
pub enum AgentRunState {
    Idle,
    Starting,
    Running,
    WaitingForApproval,
    Blocked,
    Succeeded,
    Failed,
    Cancelled,
    Disconnected,
}

pub enum AgentEvent {
    StateChanged { state: AgentRunState },
    MessageDelta { role: AgentRole, text: String },
    ToolStarted { call: NormalizedToolCall },
    ToolFinished { call_id: String, outcome: ToolOutcome },
    ApprovalRequested { request: ApprovalRequest },
    UsageUpdated { usage: AgentUsage },
    Completed { summary: Option<String> },
    Failed { error: AgentError },
}
```

The exact type names may change, but the adapter must normalize provider events before UI/orchestration consumes them. Preserve provider-specific raw payloads only in bounded diagnostic mode with secrets redacted.

## 9. Geometry and rendering requirements

### 9.1 Coordinate system

Use stable world coordinates:

```text
screen_x = world_x * zoom + pan_x
screen_y = world_y * zoom + pan_y

world_x = (screen_x - pan_x) / zoom
world_y = (screen_y - pan_y) / zoom
```

Centralize these operations in a tested transform type. Do not duplicate formulas across event handlers.

Cursor-anchored zoom must preserve the world point under the cursor:

1. Convert cursor screen position to world position using old zoom.
2. Apply clamped new zoom.
3. Recompute pan so the same world point maps back to the same cursor position.

### 9.2 Rendering order

Render in this order:

1. Canvas background.
2. Optional low-contrast grid only when it improves orientation.
3. Unselected edges.
4. Selected/active edge overlays.
5. Node surfaces by z-order.
6. Selection and resize affordances.
7. Menus, dialogs, tooltips, and approval overlays.

Use `gpui::canvas` plus `PathBuilder` for edges. Compute paths in prepaint when geometry is known and paint them in the paint callback. Keep path generation pure and unit tested.

### 9.3 Terminal sizing

- Convert node content bounds into terminal columns and rows using the same font metrics as existing panes.
- Debounce or coalesce PTY resize while dragging to avoid flooding SSH/local runtimes.
- Always send the final resize at drag end.
- Never report zero rows or columns.
- Verify high-DPI behavior and resizing at non-100% canvas zoom.
- Decide whether terminal text scales with canvas zoom or nodes transform spatially while terminal font remains readable. For v1, prefer spatial scale with a minimum effective font size and test input coordinates carefully. If GPUI clipping/input makes that unsafe, keep node content at readable scale and use zoom for positioning until a correct transform is available. Document the decision.

### 9.4 Performance target

Set and measure a concrete v1 target:

- At least 20 visible nodes, including at least 8 actively producing terminal output.
- At least 40 edges.
- Pan and drag should remain responsive on the supported macOS development machine.
- Background or off-screen nodes must not force unnecessary full terminal snapshot generation.

Do not claim virtualization until profiling proves it. Add instrumentation around snapshot generation and canvas rendering before large optimizations.

## 10. Agent adapter contract

Each adapter must expose capabilities instead of requiring UI guesses:

```rust
pub struct AgentCapabilities {
    pub interactive_pty: bool,
    pub structured_events: bool,
    pub approvals: bool,
    pub cancellation: bool,
    pub resume: bool,
    pub context_handoff: bool,
    pub remote: bool,
}
```

An adapter must provide operations equivalent to:

- Detect executable and version.
- Validate configuration.
- Start a session/job.
- Send a user prompt or context handoff.
- Respond to approval.
- Cancel or interrupt.
- Shut down cleanly.
- Emit normalized events.

Use a bounded channel for events. Define behavior when the consumer is slow. Never let unbounded provider output grow memory indefinitely.

### 10.1 Codex adapter

- Launch `codex app-server` as a child process using piped stdin/stdout and separate stderr.
- Implement the required initialize/initialized handshake.
- Support thread start/resume and turn start/steer/interrupt according to negotiated capabilities.
- Use newline-delimited JSON messages and typed serde models.
- Generate or vendor version-matched schemas using official Codex schema generation, and record the Codex version used.
- Treat unknown notification fields as forward-compatible where safe.
- Keep request IDs correlated and reject malformed responses without crashing the app.
- Surface approval requests in TermiRust UI. Never approve automatically unless a narrowly scoped user policy explicitly permits the exact category.
- On child exit, mark the run disconnected/failed with actionable diagnostics.
- Do not mix app-server JSON stdout with terminal rendering.
- If the user wants the full Codex TUI, create a separate interactive PTY node using the existing terminal runtime.

### 10.2 Claude adapter

- Prefer an official structured interface: Agent SDK boundary or `claude -p` with streaming JSON output.
- Keep stderr separate and parse only documented stdout events.
- Map permission/tool requests to TermiRust approvals.
- If hooks are used for an interactive Claude TUI, show the exact hook configuration and scope before installing.
- Support user, project, or local scope deliberately; default to the least invasive scope.
- Keep a manifest of TermiRust-installed hooks so uninstall is exact and does not remove user hooks.
- Never enable bypass-permissions mode by default.

### 10.3 Gemini adapter

- Use official headless `stream-json` for structured runs.
- Parse init, message, tool use, tool result, error, and result events as documented.
- Handle documented exit codes and convert them into actionable errors.
- Hooks are optional and consent-gated.
- Never enable `--yolo` by default.
- Preserve interactive Gemini CLI as a separate PTY mode.

### 10.4 Custom CLI adapter

- Require an executable path/name and argument list.
- Resolve and display the actual executable before launch.
- Do not invoke through `sh -c`, `bash -c`, or equivalent.
- Support a controlled working directory and explicit non-secret environment variables.
- Treat it as interactive-only unless it implements a separately registered structured protocol.
- Give it no automatic context, hooks, worktree deletion, or approval bypass.

## 11. Context handoff design

### 11.1 Pull/review semantics

A context link grants the target permission to request a bounded snapshot from the source. It does not continuously stream all source activity.

For the client-demo implementation:

1. User creates `Source -> Target` context link.
2. User selects `Send context` from the edge or target node.
3. TermiRust builds a preview.
4. User reviews or edits the preview.
5. TermiRust submits it through the target's structured adapter or pastes it through the existing guarded paste path for interactive agents.
6. The edge records only safe metadata such as last-send timestamp and status, not the full context.

### 11.2 Snapshot sources

Support a bounded subset:

- Latest normalized agent messages.
- User-selected terminal text.
- Last N terminal lines, only after explicit confirmation.
- Git diff summary from the source worktree.
- Agent completion summary.

Do not silently include:

- Full terminal scrollback.
- Environment variables.
- Credentials or key material.
- Files outside the selected repository.
- Provider internal hidden reasoning.
- Raw provider transcripts whose terms or protocol do not expose them for this use.

### 11.3 Limits and redaction

- Apply byte and message-count limits before preview.
- Normalize invalid UTF-8 safely.
- Redact common secret patterns and clearly mark redactions.
- Treat all imported context as untrusted data, not instructions from TermiRust.
- Wrap handoff text with source identity, timestamp, scope, and an explicit boundary.
- Do not execute commands found in context.

## 12. Orchestrator design

### 12.1 State machine

The orchestrator is event driven. It must not infer completion from a shell prompt or quiet timeout.

For structured agents:

- `Idle -> Starting -> Running` on validated launch and provider acknowledgement.
- `Running -> WaitingForApproval` on an approval event.
- `WaitingForApproval -> Running` only after explicit response.
- `Running -> Succeeded/Failed/Cancelled` on documented terminal events.
- Process loss without completion becomes `Disconnected` or `Failed`, not `Succeeded`.

Interactive custom CLI nodes remain `Interactive`/connected unless an explicit protocol reports more.

### 12.2 Dependency execution

- Dependency edges form a DAG.
- Validate cycles on every proposed edge and again before a run.
- Run only nodes whose enabled prerequisites have succeeded.
- A failed/cancelled prerequisite blocks dependents and explains why.
- Apply a configurable concurrency cap with a conservative default such as 2.
- Queue excess nodes visibly.
- Retries are manual by default.
- Cancellation propagates only when the user chooses it; do not kill unrelated upstream work.

### 12.3 Approvals

Approval UI must show:

- Requesting agent and provider.
- Tool or operation.
- Command/path/host affected.
- Working directory/worktree.
- Whether approval is one-time or a remembered policy.

Initial release should support one-time allow and deny. Remembered policies require a separate reviewed design and must be narrow, inspectable, editable, and revocable.

## 13. Worktree isolation

### 13.1 Default behavior

When a write-capable local agent starts inside a Git repository:

- Offer and default to an isolated linked worktree.
- Generate a unique branch such as `termirust/agent/<short-node-id>-<slug>`.
- Place managed worktrees under an app-managed directory outside the repository working tree, or under a user-approved directory.
- Persist repository root, worktree path, branch, base revision, and ownership metadata.
- Set the agent working directory to the worktree.

Read-only tasks may use the existing working directory after explicit selection.

### 13.2 Safe command execution

- Run `git` directly with argument arrays.
- Parse `git worktree list --porcelain -z` structurally.
- Canonicalize paths and ensure managed cleanup targets are inside the managed root.
- Do not use `--force` for normal create/remove operations.
- Check `git status --porcelain=v2 -z` before cleanup.
- Refuse automatic removal if tracked or untracked changes exist.
- Preserve branches containing commits not merged into the selected base.
- Surface submodule limitations before creation.

### 13.3 User actions

Provide:

- Open worktree in terminal.
- Show status/diff summary.
- Copy path.
- Mark task complete.
- Remove clean worktree.
- Keep worktree and branch.

Merging, rebasing, or cherry-picking agent work is deferred unless the repository already contains an established safe workflow for it.

## 14. SSH and remote behavior

### 14.1 Interactive remote nodes

- Create them through the existing `ConnectRequest` and `SessionPane` flow.
- Reuse credentials, jump hosts, forwarding, keepalive, TOFU, startup actions, and tmux settings.
- Restore them through existing `RestorableConnection` behavior.
- A canvas position change must never reconnect SSH.
- Network disconnect keeps the node and displays reconnect state using existing semantics.

### 14.2 Structured remote agents

Before launch, check on the selected host:

- Required provider executable and supported version.
- Working directory existence and permissions.
- Git availability when worktrees are requested.
- tmux availability only when interactive persistence is requested.

Do not silently upload a helper. If a helper becomes necessary, provide a reviewed, versioned, checksummed installation flow with explicit destination and uninstall action.

For v1, allow context and dependency edges only between nodes on the same execution host. Cross-host edges may be displayed as unsupported with a clear reason, not partially executed.

## 15. Security and privacy requirements

Treat this feature as a local orchestration control plane with command-execution power.

- Default to provider-native approval prompts and sandboxing.
- Never default to Codex danger/full-access modes, Claude bypass permissions, Gemini `--yolo`, unrestricted Groq tools, or equivalent flags.
- Never put passwords, API keys, private keys, tokens, cookies, or raw auth headers in `state.json`, canvas models, logs, crash reports, command history, context snapshots, or Git commits.
- Store secrets in the existing OS credential store/keychain and inject them only into the exact child process that needs them.
- Redact child-process command displays when arguments can contain secrets.
- Cap stdout, stderr, diagnostic payload, transcript, and context buffer sizes.
- Validate every path crossing a process or cleanup boundary.
- Never delete outside the app-managed worktree directory.
- Preserve existing multiline paste confirmation for context sent into interactive terminals.
- Mark provider and terminal output as untrusted.
- Do not interpret terminal escape sequences outside the existing `vt100` terminal path.
- Do not install hooks or edit provider configuration without preview, consent, ownership markers, and rollback.
- Add a threat-model document before enabling automatic dependency execution.

Threats that tests/review must cover:

- Shell/argument injection through node title, directory, session name, prompt, or custom command.
- Malicious repository paths and symlink traversal.
- Prompt injection in source-agent output.
- Secret leakage through context links and logs.
- Event spoofing from a child process.
- Stale process IDs and cancellation of the wrong process.
- Duplicate/dangling saved node IDs.
- Dependency cycles and runaway agent loops.
- Dirty worktree deletion.
- Remote host confusion and context sent to the wrong machine.

## 16. Persistence and migration

- Add `#[serde(default)]` to every new optional/backward-compatible field.
- Old `state.json` files must load with `WorkspaceLayoutMode::Split` and no canvas state.
- New state must survive save/load without coordinate drift or identity changes.
- Unknown future enum variants should fail safely or use a deliberate compatibility wrapper; do not silently reinterpret them.
- Add a canvas schema version before release.
- Implement a pure validation/repair pass after deserialization.
- Repair recoverable issues such as invalid zoom or dangling edges and log a concise warning.
- Preserve the original state file through the repository's existing atomic/error-handling behavior if a migration cannot be completed.
- Do not persist transient selection, drag state, open menus, focus handles, child process IDs, channels, or GPUI entities.

## 17. Implementation phases and required commits

Each phase ends with formatting, focused tests, `cargo check`, inspection of the diff, and one or more clean commits. Exact commit count may change when a phase contains independently reviewable work, but do not combine unrelated phases.

### Phase 0 - Baseline and design lock

Tasks:

- Read the current code paths listed in `AGENTS.md`.
- Record baseline `cargo test -q` and `cargo check` results without fixing unrelated warnings.
- Confirm current tmux persistence tests pass.
- Confirm the dirty worktree and preserve user changes.
- Write a short architecture decision record under `docs/` covering clean-room design, runtime boundaries, structured adapters, and security defaults.

Commit:

```text
docs(agents): define native agent canvas architecture
```

### Phase 1 - Persisted canvas model

Tasks:

- Add layout mode and saved canvas models.
- Add stable node and edge IDs.
- Add validation/repair.
- Extend `SavedWorkspace` without breaking existing JSON.
- Round-trip workspace mode, viewport, nodes, edges, agent definitions, and pane references.

Tests:

- Old state JSON defaults to Split.
- New state round-trips.
- Invalid floats/sizes/zoom are repaired.
- Duplicate IDs and dangling edges are handled deterministically.
- Dependency cycles are rejected.

Commit:

```text
feat(canvas): persist canvas workspace state
```

### Phase 2 - Geometry and canvas shell

Tasks:

- Add pure transform, placement, bounds, overlap, resize, and fit-to-content functions.
- Add workspace Split/Canvas segmented control.
- Render an empty canvas, toolbar controls, and placeholder nodes.
- Implement pan, anchored zoom, selection, drag, resize, z-order, and persistence updates.
- Add keyboard escape/delete behavior only when terminal focus is not active.

Tests:

- World/screen round-trip across zoom levels.
- Cursor-anchored zoom invariant.
- Zoom clamps.
- Deterministic non-overlapping placement.
- Fit-to-content for zero, one, and many nodes.
- Resize minimums.

Commits:

```text
feat(canvas): add pan zoom and node geometry
feat(canvas): add canvas workspace controls
```

### Phase 3 - Existing terminal sessions as nodes

Tasks:

- Render existing `SessionPane` terminal surfaces inside canvas nodes.
- Convert split panes to initial canvas placement without reconnecting.
- Add local terminal and saved host node creation.
- Preserve focus, input, selection, paste confirmation, terminal context menu, search, mouse reporting, unread activity, reconnect, and PTY resize.
- Restore canvas terminal nodes across app restart.
- Define safe mode switching when canvas session count exceeds split capacity.

Tests:

- Switching mode preserves pane/runtime identity.
- Node drag header does not consume terminal body mouse events.
- Terminal focus receives keyboard input.
- Canvas pan does not start from terminal body.
- Saved SSH/local nodes restore with stable IDs and positions.

Commit:

```text
feat(canvas): render terminal sessions as canvas nodes
```

### Phase 4 - Agent registry and interactive agent nodes

Tasks:

- Add normalized provider/backend/location/configuration types.
- Add executable/version detection.
- Add safe process launch specifications without shell concatenation.
- Add Codex, Claude Code, Gemini CLI, and Custom CLI presets.
- Launch interactive agents through existing local or SSH terminal sessions.
- Add missing CLI UI with exact guidance and recheck.
- Add agent node statuses that are truthful for interactive mode.

Tests:

- Argument arrays preserve spaces and quotes without shell injection.
- Missing executable and unsupported version states.
- Provider capability matrix.
- No secrets serialize into saved definitions.
- Interactive agent launch uses correct working directory and host.

Commit:

```text
feat(agents): add safe interactive agent nodes
```

### Phase 5 - Structured Codex adapter

Tasks:

- Implement app-server child lifecycle and handshake.
- Add version-matched typed protocol models.
- Correlate requests/responses and normalize notifications.
- Add thread/turn lifecycle, streaming messages, approvals, interruption, and process-exit handling.
- Render concise status and approval UI on the node.
- Keep the full interactive Codex TUI available as the separate PTY backend.

Tests:

- Use a deterministic fake app-server fixture for handshake, stream, approval, cancellation, malformed JSON, unknown fields, and abrupt exit.
- Add an opt-in real Codex smoke test that is skipped without an installed/authenticated CLI and never consumes paid API usage during default tests.

Commit:

```text
feat(agents): integrate structured Codex sessions
```

### Phase 6 - Context links and review-before-send

Tasks:

- Add directed context edge creation/deletion/rendering.
- Build bounded source snapshots.
- Add secret redaction and source boundary metadata.
- Add preview/edit/confirm UI.
- Send through structured adapters or existing protected paste flow.
- Record safe last-handoff metadata.

Tests:

- Direction and endpoint validation.
- Byte/message limits.
- Secret redaction fixtures.
- Interactive paste still triggers multiline confirmation.
- Context content is not persisted in state.
- Disabled edges cannot send.

Commit:

```text
feat(orchestration): add reviewed context handoffs
```

### Phase 7 - Git worktree isolation

Tasks:

- Detect repository root and worktree support.
- Parse worktree/status output structurally.
- Create unique managed worktree and branch.
- Launch write-capable agents in the worktree.
- Show branch/path/dirty status.
- Add safe clean removal and keep actions.

Tests:

- Temporary real Git repository integration tests.
- Spaces and non-ASCII repository paths where supported by existing project conventions.
- Existing branch checked out elsewhere.
- Dirty/untracked worktree refuses removal.
- Unmerged commits preserve branch/worktree.
- Managed-root path traversal is rejected.

Commit:

```text
feat(agents): isolate coding agents with git worktrees
```

### Phase 8 - Claude and Gemini structured adapters

Tasks:

- Implement official structured modes behind the same normalized contract.
- Add provider-specific version detection, event parsing, error mapping, approvals, and cancellation.
- Add optional hook installation only if structured lifecycle gaps require it.
- Include exact hook preview and rollback.

Tests:

- Provider JSONL fixtures from documented schemas/events.
- Forward-compatible unknown event handling.
- Exit code and process-loss mapping.
- Hook install does not overwrite unrelated user configuration.
- Hook uninstall removes only TermiRust-owned entries.

Commits:

```text
feat(agents): integrate structured Claude sessions
feat(agents): integrate structured Gemini sessions
```

### Phase 9 - Dependency orchestration

Tasks:

- Add dependency edge UI and DAG validation.
- Add task queue, concurrency cap, prerequisite state, blocked reasons, and explicit run action.
- Add approval propagation rules and cancellation UI.
- Add a threat-model document before enabling execution.

Tests:

- Topological ordering.
- Cycle rejection.
- Concurrency cap.
- Failure/cancellation blocks dependents.
- Retry does not duplicate successful prerequisites.
- Restart does not silently resume an incomplete autonomous workflow.

Commit:

```text
feat(orchestration): run bounded agent dependency graphs
```

### Phase 10 - Remote support, polish, and release validation

Tasks:

- Add remote capability checks and same-host structured launch where supported.
- Verify tmux attach/detach/kill distinctions.
- Add accessibility labels, keyboard navigation, reduced-motion behavior if animations exist, and useful diagnostics.
- Profile and optimize measured hotspots only.
- Update user documentation and release notes.

Tests:

- Existing Docker SSH/tmux persistence tests.
- Remote missing CLI/tmux/Git states.
- Disconnect/reconnect while node remains on canvas.
- Same-host edge restrictions.
- Full regression suite.

Commits:

```text
feat(agents): support remote agent canvas sessions
test(canvas): cover agent canvas workflows
docs(canvas): document agent canvas usage
```

## 18. Automated verification

Run at minimum:

```bash
cargo fmt --check
cargo check
cargo test -q
```

Also run focused tests after each phase. Do not wait until the end to discover serialization or interaction regressions.

Where GUI behavior cannot be covered by ordinary unit tests:

- Add GPUI-focused tests if the repository's current test harness supports them.
- Keep geometry and state transitions pure so most behavior remains unit-testable.
- Add deterministic fake provider executables under test fixtures rather than invoking paid/live providers.
- Make Docker/integration tests self-skipping with a clear reason only when a required external tool is unavailable.
- A skipped integration test is not proof of success; record the skip in the final report.

Do not "fix" existing macOS `objc` cfg warnings or unrelated dead-code warnings as part of this feature unless they become errors or directly block the work.

## 19. Manual QA checklist

Perform and record this matrix before declaring v1 complete:

### Workspace and canvas

- New and old state files open.
- Split workspace behaves exactly as baseline.
- Switch Split -> Canvas -> Split without reconnecting.
- Add 20 nodes and verify placement, focus, drag, resize, zoom, fit, and restore.
- Restart app and verify world positions, titles, viewport, links, and active sessions.
- Test small and large windows, Retina scaling, and multiple monitors.

### Terminal behavior

- Local terminal typing, resize, selection, copy, paste, search, and scrollback.
- SSH terminal with key and password auth.
- xterm mouse-reporting application inside a node.
- Multiline paste confirmation inside a canvas terminal.
- SSH disconnect and reconnect.
- tmux process continues after client disconnect and reattaches without rerunning startup command.
- Closing tmux node detaches unless user explicitly chooses kill.

### Agents

- Installed and missing Codex/Claude/Gemini CLIs.
- Interactive PTY mode for each installed provider.
- Structured Codex stream, approval, denial, interrupt, success, malformed event, and child crash.
- Structured Claude/Gemini equivalent after their phases.
- Custom CLI with paths/arguments containing spaces and quotes.
- No bypass/yolo flags appear by default.

### Context and orchestration

- Create/delete directed context edge.
- Preview, edit, cancel, and send handoff.
- Verify secrets are redacted and full context is not persisted.
- Dependency cycle is rejected with a clear explanation.
- Failed prerequisite blocks target.
- Concurrency cap queues extra work.
- Cancelling one node does not kill unrelated nodes.

### Worktrees

- Create two agents from one repository and confirm distinct paths, branches, indexes, and changes.
- Dirty worktree cannot be removed.
- Clean worktree removal succeeds.
- Existing user worktrees remain untouched.
- App restart rediscovers managed worktrees safely.

### Remote behavior

- Local and SSH nodes coexist on one canvas.
- Remote provider missing state gives install guidance without automatic changes.
- Unsupported cross-host link cannot execute.
- Remote disconnect does not destroy canvas layout.

## 20. Definition of done

Do not call the feature done until all of the following are true:

- Existing TermiRust features and tests still pass.
- Old state files load with no user action.
- Split layout remains the default and behaves unchanged.
- Canvas sessions reuse existing terminal runtimes instead of duplicating them.
- Canvas layout, node identities, edges, and viewport restore reliably.
- Terminal interaction inside nodes is correct at supported zoom levels.
- Interactive agent presets work without shell concatenation.
- Structured agent status is based on documented protocol events.
- Context handoff is directed, bounded, reviewable, and redacted.
- Write-capable concurrent agents are isolated by worktrees by default.
- Destructive actions and approvals are explicit.
- Secrets do not enter persisted state or logs.
- Automated tests and the manual matrix have recorded results.
- Documentation explains setup, missing CLI behavior, tmux behavior, worktree cleanup, security defaults, and current limitations.
- Git history is clean, phase-oriented, and has no co-author trailers.

## 21. Final implementation report

At completion, provide a concise report with:

1. Architecture implemented and any deliberate deviations from this goal.
2. Files added and materially changed.
3. Commits created, in order.
4. Automated commands run with pass/fail/skip counts.
5. Manual scenarios completed and their results.
6. Security checks performed.
7. Known limitations and deferred work.
8. Exact steps for a reviewer to run the client demo.

Do not claim a provider integration was tested if only mocks ran. State clearly which live CLIs, versions, remote hosts, and authentication paths were actually verified.

## 22. Primary research sources

These sources were reviewed on 2026-07-15. Recheck official documentation during implementation because agent protocols change quickly.

- NodeTerm repository and public behavior: https://github.com/eneskirca/nodeterm
- NodeTerm license: https://github.com/eneskirca/nodeterm/blob/main/LICENSE
- GPUI 0.2.2 API: https://docs.rs/gpui/0.2.2/gpui/
- GPUI `PathBuilder`: https://docs.rs/gpui/0.2.2/gpui/struct.PathBuilder.html
- Codex app-server: https://developers.openai.com/codex/app-server
- Claude Code headless/programmatic mode: https://code.claude.com/docs/en/headless
- Claude Code hooks reference: https://code.claude.com/docs/en/hooks
- Claude Code permissions: https://code.claude.com/docs/en/permissions
- Gemini CLI headless mode: https://geminicli.com/docs/cli/headless/
- Gemini CLI hooks reference: https://geminicli.com/docs/hooks/reference/
- Gemini CLI configuration: https://geminicli.com/docs/reference/configuration/
- Groq tool use overview: https://console.groq.com/docs/tool-use/overview
- Groq local tool calling: https://console.groq.com/docs/tool-use/local-tool-calling
- Agent Client Protocol architecture: https://agentclientprotocol.com/get-started/architecture
- Agent Client Protocol Rust implementation: https://github.com/agentclientprotocol/agent-client-protocol
- Git worktree manual: https://git-scm.com/docs/git-worktree
- tmux getting started and session semantics: https://github.com/tmux/tmux/wiki/Getting-Started
- tmux control mode reference: https://github.com/tmux/tmux/wiki/Control-Mode

## 23. Final instruction to the implementation agent

Implement this in order. Do not jump directly to autonomous orchestration before the canvas, persistence, terminal interaction, process safety, and structured event boundaries are correct. When the current codebase disagrees with a proposed type name or file placement, preserve the existing architecture and document the adjustment. When a provider's current official protocol disagrees with this document, follow the current official protocol, add compatibility tests, and record the change.

The standard is not a flashy prototype. The standard is a dependable TermiRust capability that a client can use on real repositories and remote systems without losing terminal sessions, code, credentials, or trust.
