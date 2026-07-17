# TermiRust

Native desktop SSH client built with `gpui`, `gpui-component`, `russh`, and `vt100`.

## Current product shape

- Host library UI inspired by Terminus-style launchers, with groups, tags, vaults, batch selection, and bulk actions.
- Single-row top chrome with custom in-app traffic lights (close / minimize / zoom); the macOS OS title-bar drag is taken over so the chrome stays draggable from its empty area.
- Draggable workspace tabs that scroll horizontally when they overflow; double-click a tab to rename it; right-click a tab for Duplicate / Duplicate in a new window / Rename / Split / Close.
- Each workspace tab can contain split panes arranged as a recursive binary tree: dropping a tab onto a pane splits that pane, with arbitrary nesting and resizable dividers.
- Each pane is its own SSH session and PTY; a native local terminal can also be opened and behaves like any other pane.
- Quick connect: type `user@host` or `ssh user@host:port` in the search bar.
- Reconnect button on disconnected/errored panes; optional automatic reconnect after non-user-initiated SSH drops, configurable in Settings.
- Configurable SSH keep-alive ping interval to keep idle sessions alive across NAT/load-balancer timeouts.
- Per-host SFTP remote-files view (browse, upload, download, delete) available from any workspace.
- Per-host port-forwarding rules (local, remote reverse, dynamic SOCKS) that start automatically on connect.
- Per-host jump-host chains.
- Saved snippets plus a per-workspace command palette for snippets, recent commands, and built-in tasks.
- Snippet commands accept {{HOST}}, {{USER}}, {{PORT}}, {{TITLE}}, {{ADDRESS}} placeholders that expand against the active pane on send.
- Per-host color tag, environment variables, description/notes, startup directory, and startup command.
- Right-click context menu on terminal panes; per-pane Clear and Duplicate; Detach moves a pane into its own workspace tab.
- Multi-line clipboard pastes are held behind a confirmation banner by default to prevent accidental script execution.
- Per-workspace Broadcast Input toggle that fans typed/pasted bytes out to every connected pane.
- Window size and position — including which monitor — persist across launches.
- Diagnostic logging (all stderr output and panic traces) is written to a log file in the app data directory.
- Encrypted-vault shared-folder sync through Dropbox / iCloud Drive / Google Drive / Syncthing, plus portable and passphrase-encrypted JSON export/import.
- Keyboard shortcuts: Cmd+D / Cmd+Shift+B / Cmd+Shift+L / Cmd+Alt+arrow, library/section switching, search focus, and new-host flow.
- Terminal surface supports:
  - raw VT rendering, PTY resize, local scrollback, terminal search
  - text selection and clipboard copy, optional copy-on-select for mouse selections
  - clipboard paste with bracketed paste when requested
  - xterm mouse reporting for terminal apps that enable it
  - configurable terminal font size and font family
- Persistent session history with timestamps and duration in the Logs view; host cards surface a relative "Last connected" badge.
- Per-workspace Split or Agent Canvas layout. Canvas mode places existing local
  and SSH terminals, interactive or structured coding agents, sticky notes, and
  group frames in persisted, draggable, resizable nodes with reviewed context
  links and bounded dependency orchestration.
- Saved host groups can open directly as SSH Fleet canvases. The fleet panel
  summarizes connection and tmux state and provides guarded reconnect,
  broadcast-input, and disconnect controls without removing canvas nodes.
- Local Codex, Claude Code, and Gemini structured jobs normalize provider events;
  interactive remote agents reuse saved SSH and tmux settings.
- Write-capable local coding agents default to app-managed Git worktrees with
  conservative status inspection and clean-only removal.
- Settings view for appearance theme, terminal font, default local shell, workspace restore, history limits, and import/export.
- TOFU known-host pinning; Known Hosts view supports deleting pinned host keys.
- Keychain view shows imported key type, public key availability, and an "Add Key File" picker.
- Build/distribution metadata for cargo-bundle (macOS .app, Linux deb/rpm) lives in `Cargo.toml`; per-platform release flow is in `docs/building.md`.

## Explicitly out of scope right now

- drag-reordering split panes
- remote team / multiplayer features (shared-folder vault sync is the only sync that exists)

## Important architecture

- [src/main.rs](src/main.rs)
  - Bootstraps GPUI, redirects logs to a file, restores the saved window bounds/display, registers the embedded asset source, and opens the main window.
- [src/platform_mac.rs](src/platform_mac.rs)
  - macOS window-control interop: disables the OS title-bar drag so the chrome tabs stay usable, and starts native window drags from the chrome's empty area.
- [src/assets.rs](src/assets.rs)
  - Embedded SVG asset source for the app chrome and custom Phosphor-style icons.
- `src/ui/app/` — main application state and UI, split across modules:
  - [mod.rs](src/ui/app/mod.rs) — `TermiRustApp` state, event loop, recursive split tree, window-bounds persistence.
  - [chrome.rs](src/ui/app/chrome.rs) — top chrome: tab strip, traffic lights, tab context menu.
  - [workspace.rs](src/ui/app/workspace.rs) — terminal pane rendering, split layout, SFTP files view.
  - [editor.rs](src/ui/app/editor.rs) / [hosts.rs](src/ui/app/hosts.rs) / [library.rs](src/ui/app/library.rs) — host editor and library.
  - [connect.rs](src/ui/app/connect.rs) / [sftp.rs](src/ui/app/sftp.rs) / [palette.rs](src/ui/app/palette.rs) / [overlay.rs](src/ui/app/overlay.rs) / [types.rs](src/ui/app/types.rs).
  - [canvas.rs](src/ui/app/canvas.rs) — canvas geometry, interaction, terminal and
    agent nodes, links, worktree controls, and orchestration UI.
- `src/agents/` — safe process launch, normalized protocols, provider adapters,
  context redaction, worktree ownership, and dependency scheduling.
- [src/ui/theme.rs](src/ui/theme.rs)
  - App color system and layout constants.
- [src/terminal.rs](src/terminal.rs)
  - VT state wrapper around `vt100`: snapshot generation, scrollback access, selection extraction, bracketed-paste and mouse-mode inspection.
- [src/ssh.rs](src/ssh.rs)
  - SSH runtime thread and Tokio event loop: shell open, PTY allocation, raw input/output, and remote resize.
- [src/local.rs](src/local.rs)
  - Local PTY shell sessions (started in the user's home directory).
- [src/sftp.rs](src/sftp.rs)
  - SFTP runtime backing the remote-files view.
- [src/credentials.rs](src/credentials.rs)
  - System credential-store (keyring) access for saved passwords.
- [src/models.rs](src/models.rs)
  - Saved host models, draft parsing, connect-request generation, and persisted window bounds.
- [src/storage.rs](src/storage.rs)
  - Saved state persistence, TOFU known-host pinning, startup import of local `~/.ssh` identities, and host import from `~/.ssh/config`.

## State model notes

- A `WorkspaceTab` is the top-level unit shown in the chrome bar.
- A workspace's panes are arranged by a recursive `SplitNode` binary tree (`Leaf` / `Split { axis, ratio, a, b }`); dropping a tab on a pane splits that leaf. The cap is `MAX_SPLIT_PANES` (4).
- A `SessionPane` owns one SSH (or local PTY) runtime and one `TerminalState`.
- Split panes are separate SSH sessions to the same host, not a single PTY split.
- Unread tab activity is tracked per workspace and is used for tab badges.
- `SessionLogEntry` records connect/disconnect/error events with timestamps; session logs persist in `state.json` (capped at 200) and survive restarts. Each `SessionPane` carries a `log_id` linking it to its entry.
- `QuickConnect::parse` extracts `user@host:port` from search bar input.
- The window frame and its display id are persisted in `state.json` and reapplied on the next launch.

## Build and run

```bash
cargo fmt
cargo check
cargo run            # debug build; use --release for performance testing
```

On macOS, GPUI may need access to the system shader cache during first compile/run.

Runtime logs (all `eprintln!` output and panic backtraces) are written to
`<data dir>/termirust/termirust.log`, truncated on each launch.

## UI behavior details

- Clicking a host card loads it into the editor panel.
- Connecting opens a new workspace tab; the tab strip auto-scrolls to keep the active tab visible.
- Dragging a workspace tab reorders tabs; dropping onto a tab inserts before it; dropping in the empty strip after the last tab moves the tab to the end.
- Dropping a tab onto a terminal pane splits that pane.
- Double-clicking the empty chrome area opens a new local terminal.
- Active workspace search is local to the active pane; search and unread badges are per workspace tab.

## Known implementation limits

- Mouse reporting is practical, not exhaustive protocol coverage.
- Search is plain substring search over terminal text, not regex.
- The `Keychain` imports keys from `~/.ssh` and allows picking files from disk, but does not generate keys.
- SSH config hosts are imported at startup (shown with an `SSH Config` badge) and runtime-synced, not written back into the app state file.
- Quick connect uses the first available SSH key; for password-only auth, use the host editor form.
- The terminal re-renders the whole app on any output and rebuilds a full snapshot per frame; the performance backlog is tracked in GitHub issue #2.
