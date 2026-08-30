# Read-only TUI dependency and lifecycle decision

Date: 2026-08-30

## Decision

Goal 15.1 uses exact `ratatui 0.30.2` with its exact locked `ratatui-crossterm 0.1.2` backend
and `crossterm 0.29.0`. `signal-hook 0.3.18` provides Unix signal delivery and
`unicode-width 0.2.2` remains the workspace width contract. The TUI is a separate
`termirust-tui` crate and binary whose normal dependency graph contains only presentation,
signal, typed domain, and typed store-read dependencies.

Primary sources reviewed:

- Ratatui initialization and restoration guidance:
  <https://docs.rs/ratatui/0.30.2/ratatui/init/index.html>
- Ratatui 0.30.2 release and repository:
  <https://github.com/ratatui/ratatui/releases/tag/ratatui-v0.30.2>
- Crossterm 0.29 event and blocking-read contract:
  <https://docs.rs/crossterm/0.29.0/crossterm/event/index.html>
- Crossterm repository, cross-platform scope, dependency rationale, and MIT license:
  <https://github.com/crossterm-rs/crossterm>

Ratatui 0.30.2 declares Rust 1.88 and MIT, matching this workspace's Rust floor and allowed
licenses. The Ratatui facade, core, widgets, and Crossterm adapter forbid unsafe code. Crossterm
contains reviewed platform FFI for TTY detection, termios, terminal sizing, and Windows console
operations; TermiRust does not add another unsafe terminal implementation around it. The new crate's
only unsafe block is the required `ManuallyDrop` destruction call used to prevent duplicate cursor
restoration, and it is covered by process-level panic and signal tests.

## Authority boundary

`termirust-store::load_fleet_read_only` reads existing bounded, validated project and Session
documents without opening a mutable repository. It does not create the store directory, metadata
files, lock files, backups, or Session data directories. The TUI receives projected IDs, labels,
status, activity, unread/archive state, and revisions only. It has no Host client, SSH, PTY, process,
session mutation, project mutation, artifact, transcript, or raw state-file dependency.

The inspector intentionally excludes terminal output, transcripts, full paths, argv, conversation
IDs, credentials, and artifact bytes. User labels have C0/C1, ANSI escape, and bidi-control input
removed before projection; rendering adds bounded directional isolation. Recording-friendly mode
replaces all project, group, and Session labels.

## Resource and event model

- At most 1,000 Projects and 10,000 visible Sessions are projected.
- Filter input is capped at 128 Unicode scalars and user labels at 256 scalars.
- Input, signal, and one active refresh feed one fixed 64-event synchronous channel.
- The main loop blocks on that channel. Input and signal threads block in their platform APIs; no
  timer, animation, background polling, or automatic refresh runs while idle.
- Refresh generations reject stale results. Escape cancels the active generation and keeps the last
  safe snapshot. Worker panic becomes one allowlisted failure result.
- Rendering visits only visible viewport rows after a bounded precomputed filter projection.

## Terminal restoration

Fullscreen mode owns raw mode, cursor visibility, and the alternate screen. Inline mode owns raw
mode and cursor visibility but never enters the alternate screen. One atomic restoration claim is
shared by normal drop and the panic hook. Normal drop first lets Ratatui clear its tracked cursor
state, then disables raw mode and leaves the alternate screen. Panic restores immediately and
suppresses the later terminal destructor. SIGINT, SIGTERM, and SIGHUP enter the same normal drop
path. Process-level pseudo-terminal tests compare termios before/after and assert exact restoration
sequences.

## Platform scope

Crossterm supports macOS, Linux, and Windows, but this leaf records live pseudo-terminal evidence
only on the reference Apple-silicon macOS environment. The model and TestBackend tests are
platform-neutral. Windows/Linux terminal and screen-reader claims remain release-platform work,
not inferred completion.
