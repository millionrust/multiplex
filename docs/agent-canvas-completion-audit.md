# Agent Canvas Completion Audit

Date: 2026-07-15

This audit compares the current `test` branch with the Definition of Done in
`goal.md`. It is intentionally stricter than a test summary. `Proved` means the
current source and a directly relevant test or runtime check support the claim.
`Partial` means implementation exists but the required verification scope has
not been completed.

## Definition of Done evidence

| Requirement | Status | Current evidence | Remaining evidence |
| --- | --- | --- | --- |
| Existing TermiRust behavior and tests pass | Proved | `cargo test -q`: 295 passed, 0 failed, 3 opt-in live checks ignored; Docker SSH, SFTP, forwarding, jump-host, restore, and tmux tests ran. | None for automated regression scope. |
| Old state loads without action and defaults to Split | Proved | Model migration tests cover legacy defaults, round trips, repair, and future-schema fallback. | Human opening a real pre-feature state file remains in the release matrix. |
| Split remains the default and keeps its runtime identity | Proved | GPUI layout-switch and over-capacity chooser tests retain pane IDs and hidden sessions. | Human baseline comparison remains. |
| Canvas reuses existing terminal runtimes | Proved | `CanvasNodeKind` references pane IDs; GPUI tests verify local and SSH panes coexist and mode switching does not respawn them. | None for ownership design. |
| Layout, stable IDs, edges, and viewport restore | Proved | Serialization, validation/repair, geometry, and explicit restored-agent restart tests pass. | Human visual restore of a populated canvas remains. |
| Terminal interaction is correct at supported zoom | Partial | End-to-end Canvas local-shell tests cover typing, deterministic selection copy, clipboard shortcuts, guarded multiline paste, search, Shift-PageUp/PageDown scrollback, xterm mouse-report bytes, and live PTY resize confirmed by `stty size` at zoom 0.5 and 2.0. Mouse ownership, transforms, culling, keyboard reveal, and node-header action isolation are also automated. | Human pointer selection/copy, wheel scrollback, resize-handle dragging, and visual behavior at multiple zoom levels. |
| Interactive presets avoid shell concatenation | Proved | Local launches use executable plus argument arrays; the single SSH boundary is centrally quoted and injection-tested. Unsafe bypass flags are rejected. | Live interactive Claude and Gemini checks require installed, authorized CLIs. |
| Structured state comes from documented events | Partial | Codex fake app-server coverage is comprehensive and Codex 0.144.4 passed a live smoke. Claude/Gemini JSONL fixtures and child processes are deterministic. | Successful live Claude and Gemini calls require authorized accounts/installations. |
| Context handoff is directed, bounded, reviewed, and redacted | Proved | Context model, redaction, bounds, disabled-edge, persistence, guarded-paste, same-host remote handoff, and cross-host refusal tests pass. | Human preview editing and visual copy review remain. |
| Concurrent writers default to isolated worktrees | Proved | Creation defaults local agents to `Isolated`; real Git tests cover creation, branch/path uniqueness, status, committed/dirty refusal, and root boundaries. | Human two-agent workflow and restart rediscovery remain. |
| Destructive actions and approvals are explicit | Partial | tmux kill uses a two-stage confirmation; worktree removal refuses unsafe targets; Codex approval allow/deny is rendered and protocol-tested. | Human dialog review remains. Claude/Gemini one-shot headless protocols do not expose an approval callback; see `agent-protocols.md`. |
| Secrets do not enter canvas persistence or handoff output | Proved for modeled paths | Agent definitions contain no credential material; host locations reference saved profiles; password sessions use credential IDs; context redaction and bounded diagnostics are tested. | Release security review should still inspect logs while using real provider credentials. |
| Automated and manual matrix results are recorded | Partial | Automated results and the manually completed tmux scenario are in `agent-canvas-qa.md`. | The manual release checklist is not complete. |
| Setup, missing CLI, tmux, worktree, security, and limits are documented | Proved | `agent-canvas.md`, architecture, threat model, protocol notes, and QA record cover these topics. | Keep provider-version notes current before release. |
| Git history is phase-oriented with no co-author trailers | Proved | Feature work is split across focused commits; repository history contains no `Co-authored-by` trailer for this work. | Do not stage the unrelated user `.gitignore` change. |

## Required protocol deviations

### Claude and Gemini approvals

The goal asks every structured adapter to expose interactive approval cards.
That is implemented for Codex app-server. It is not claimed for the current
Claude and Gemini one-shot headless adapters:

- Claude CLI headless mode supports predefined permission behavior. Its
  callback approval surface is part of the official TypeScript/Python Agent SDK,
  not a Rust or language-neutral child-process protocol.
- Gemini documents that an `ask_user` policy decision becomes `deny` in
  non-interactive mode.

TermiRust keeps both capabilities false rather than auto-approving, scraping a
terminal prompt, or inventing an unsupported wire response.

### Native accessibility semantics

The published GPUI 0.2.2 and gpui-component 0.5.1 sources resolved by this
repository contain no AccessKit integration or per-element semantic label API.
Canvas controls therefore use visible labels or tooltips, and keyboard node
navigation reveals off-screen selection.

Upstream GPUI `main` now documents AccessKit integration and exposes roles,
`aria_label`, and accessible-action handlers in
[`crates/gpui/src/_accessibility.rs`](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/_accessibility.rs).
That code is newer than the published crate while retaining the same package
version, and the application also depends on gpui-component's published GPUI
types. Native screen-reader labels therefore require a coordinated GPUI and
gpui-component migration, followed by macOS VoiceOver verification; they are
not complete in this branch.

## Completion decision

The implementation is not yet eligible for `complete` status under `goal.md`.
The mandatory human matrix, successful live Claude/Gemini verification, native
screen-reader semantics, and the documented provider approval deviations remain
unresolved. No automated result should be used to claim those items passed.
