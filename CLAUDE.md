# TermiRust

Native desktop SSH client built with `gpui`, `gpui-component`, `russh`, and `vt100`.

## Current product shape

- Host library UI inspired by Terminus-style launchers.
- Dark top chrome with draggable workspace tabs.
- Each workspace tab can contain split panes.
- Each pane is its own SSH session and PTY.
- Quick connect: type `user@host` or `ssh user@host:port` in the search bar.
- Reconnect button on disconnected/errored panes and workspace toolbar.
- Optional automatic reconnect after non-user-initiated SSH drops, configurable in Settings.
- Configurable SSH keep-alive ping interval to keep idle sessions alive across NAT/load-balancer timeouts.
- Snippet commands accept {{HOST}}, {{USER}}, {{PORT}}, {{TITLE}}, {{ADDRESS}} placeholders that expand against the active pane on send.
- Per-host color tag flows into the host card avatar and the connected pane status dot.
- Per-workspace Broadcast Input toggle that fans typed/pasted bytes out to every connected pane.
- Per-pane Clear and Duplicate actions, plus Cmd+D / Cmd+Shift+B / Cmd+Shift+L / Cmd+Alt+arrow keyboard shortcuts.
- Terminal surface supports:
  - raw VT rendering
  - PTY resize
  - local scrollback
  - terminal search
  - text selection and clipboard copy
  - optional copy-on-select for mouse selections
  - clipboard paste with bracketed paste when requested
  - xterm mouse reporting for terminal apps that enable it
  - configurable terminal font size and font family
- Persistent session history with timestamps and duration in the Logs view; host cards surface a relative "Last connected" badge derived from this history.
- Per-host description/notes field surfaced on cards and searchable from the Hosts library.
- Keychain shows imported key type, public key availability, and an "Add Key File" picker.
- Known Hosts view supports deleting pinned host keys.

## Explicitly out of scope right now

- `SFTP`
- port forwarding
- snippets
- file transfer UI
- persistent restoration of open tabs/workspaces after relaunch
- drag-reordering split panes

## Important architecture

- [src/main.rs](src/main.rs)
  - Bootstraps GPUI, registers the embedded asset source, and opens the main window.
- [src/assets.rs](src/assets.rs)
  - Embedded SVG asset source for the app chrome and custom Phosphor-style icons.
- [src/ui/app.rs](src/ui/app.rs)
  - Main application state and UI shell.
  - Host library, workspace tabs, split panes, search, clipboard, mouse handling, tab reordering, and icon usage live here.
- [src/ui/theme.rs](src/ui/theme.rs)
  - App color system and layout constants.
- [src/terminal.rs](src/terminal.rs)
  - VT state wrapper around `vt100`.
  - Snapshot generation, scrollback access, selection extraction, bracketed paste and mouse mode inspection.
- [src/ssh.rs](src/ssh.rs)
  - SSH runtime thread and Tokio event loop.
  - Shell open, PTY allocation, raw input/output, and remote resize.
- [src/models.rs](src/models.rs)
  - Saved host models, draft parsing, and connect request generation.
- [src/storage.rs](src/storage.rs)
  - Saved state persistence, TOFU known-host pinning, startup import of local `~/.ssh` identities, and host import from `~/.ssh/config`.

## State model notes

- A `WorkspaceTab` is the top-level unit shown in the chrome bar.
- A workspace contains one or more pane ids.
- A `SessionPane` owns one SSH runtime and one `TerminalState`.
- Split panes are separate SSH sessions to the same host, not a single PTY split.
- Unread tab activity is tracked per workspace and is used for tab badges.

## Build and run

```bash
cargo fmt
cargo check
cargo run
```

On macOS, GPUI may need access to the system shader cache during first compile/run.

## UI behavior details

- Clicking a host card loads it into the editor panel.
- Connecting opens a new workspace tab.
- Dragging a workspace tab reorders tabs in the top chrome.
- Dropping onto a tab inserts before that tab.
- Dropping into the empty strip after the last tab moves the tab to the end.
- Active workspace search is local to the active pane.
- Search badges and unread badges are per workspace tab.

## State model notes (continued)

- `SessionLogEntry` records connect/disconnect/error events with timestamps.
- Session logs are persisted in `state.json` and survive app restarts, capped at 200.
- Each `SessionPane` carries a `log_id` linking it to its `SessionLogEntry`.
- `QuickConnect::parse` extracts `user@host:port` from search bar input.

## Known implementation limits

- Mouse reporting is practical, not exhaustive protocol coverage.
- Search is plain substring search over terminal text, not regex.
- `Keychain` imports keys from ~/.ssh and allows picking files from disk, but does not generate keys.
- SSH config hosts are imported at startup and shown in the Hosts library with an `SSH Config` badge.
- Imported SSH config hosts are runtime-synced, not written back into the app state file.
- Quick connect uses the first available SSH key; for password-only auth, use the host editor form.
- Transfers are not implemented.
