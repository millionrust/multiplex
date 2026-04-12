# Terminal

A native desktop SSH client. Save your servers, connect in a click, split terminals side by side. No Electron, no browser -- just a Rust binary talking to your boxes over SSH.

![Windows](https://img.shields.io/badge/platform-Windows-lightgrey)
![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Linux](https://img.shields.io/badge/platform-Linux-lightgrey)
![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)
![Status](https://img.shields.io/badge/status-early%20alpha-yellow)

---

## What you get

Terminal has a host library where you save connection details once. Click a host, get a terminal. Want to watch logs on one server while poking at another? Split the workspace into up to 4 panes, each running its own SSH session.

There's also a quick-connect bar. Type `user@host` or `ssh user@host:port` and you're in, no need to save anything first.

The terminal itself does VT100 rendering, scrollback, in-terminal search, text selection, clipboard copy/paste, and xterm mouse reporting (so `htop`, `vim`, and friends work). It's rendered through [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the GPU-accelerated framework from the Zed editor.

Other things worth knowing about:

- Your user SSH config and key directory are imported automatically at launch, so your servers should already be there when you first open it.
- Workspace tabs are draggable. You can reorder them.
- Hosts can be organized into groups in the library.
- Reusable identities can be imported and reused across hosts.
- Saved snippets can be reused and sent into the active terminal.
- Hosts now support tags, and the host library search matches labels, groups, tags, vaults, jump hosts, and endpoints.
- Hosts can now save multiple local TCP forwarding rules and start them automatically when they connect.
- Hosts can optionally connect through saved jump-host chains.
- Every workspace can switch into an SFTP remote-files view for browse, upload, download, and delete against the active host.
- A native local terminal can be opened directly from the chrome and behaves like a normal workspace tab or split pane.
- Vaults are now first-class local containers for hosts, snippets, and identities, with a dedicated library view plus local shared-vault member/role management.
- The active terminal now gets keyboard-selectable, target-aware inline command autocomplete, ranked from current-host history, snippets, and built-in shell shortcuts.
- Every terminal workspace now has a searchable command palette for snippets, recent commands, and built-in tasks, with keyboard navigation and direct execution.
- A dedicated Settings view now persists global appearance theme and terminal font-size preferences, and applies them live.
- Settings also define the default local terminal shell executable and startup directory for new local sessions.
- Settings now include portable JSON export/import for hosts, vaults, identities, snippets, and known-host trust records, with credentials intentionally excluded.
- Settings also include passphrase-encrypted local backup export/import for the same bundle format, as groundwork for future sync without relying on plaintext files.
- TOFU host key pinning, with a Known Hosts view where you can review or delete pinned keys.
- A keys view shows imported key types and lets you add key files from disk.
- Session logs track your connection history with timestamps and duration across app restarts.
- One-click reconnect when a session drops.
- Password-backed hosts can store credentials in the system credential manager.
- Reopenable workspaces are restored after relaunch, including password sessions that can reconnect through the system credential manager.

## Platforms

The app is being built as a native Rust desktop client for Windows, macOS, and Linux. Secure credentials use each platform's native credential backend through `keyring`, and config/state storage already uses cross-platform user directories.

Cross-platform parity work is still in progress in the UI and packaging layers, so expect rough edges outside the primary development environment.

The tracked parity target is documented in [docs/termius-parity.md](docs/termius-parity.md).
The working backlog for the remaining parity push is tracked in [docs/parity-todo.md](docs/parity-todo.md).

## Not built yet

This is early alpha. The following are on the radar but don't exist yet:

- Drag-reordering split panes
- Vault sync / remote team features
- Deeper command autocomplete / shell intelligence
- Platform-specific packaging polish

## License

MIT
