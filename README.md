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
- Hosts can be starred, organized into groups, and surfaced in the library by priority.
- The host library now supports batch selection, bulk star/unstar, and bulk group assignment for fleet cleanup.
- The Hosts library now also includes saved-group management cards and group-level actions, so you can select an entire visible group, target it for bulk reassignment, and load its saved defaults into the editor without hopping host by host.
- Hosts can also save a startup directory and startup command, so SSH sessions open directly into the right project context.
- Host groups can now save inheritable defaults for username, tags, identity, jump host, startup behavior, and saved forwarding rules, and hosts can load or inherit those defaults when their own fields are blank.
- Hosts now also persist session preferences like `Connect View` and `Scrollback Rows`, so a server can open straight into Files view after connect and keep a host-specific amount of terminal history locally.
- Reusable identities can be imported and reused across hosts.
- Saved snippets can be reused, pinned, and sent into the active terminal.
- Hosts now support tags, and the host library search matches labels, groups, tags, vaults, jump hosts, and endpoints.
- Hosts can now save multiple port-forwarding rules, including local TCP tunnels, remote reverse tunnels, and dynamic SOCKS5 proxies, and start them automatically when they connect.
- Hosts can optionally connect through saved jump-host chains.
- Every workspace can switch into an SFTP remote-files view for browse, upload, download, and delete against the active host.
- When a host has a saved startup directory, the remote Files view also opens there by default.
- A native local terminal can be opened directly from the chrome and behaves like a normal workspace tab or split pane.
- Library and workspace empty states now include direct actions, so dead ends turn into obvious next steps like creating a host, adding a key, opening local terminal, or returning to terminal view.
- The Hosts library now has a dismissible first-run welcome panel with import counts, quick-start actions, and shortcut guidance, and it automatically gets out of the way once you save a host or open a session.
- Vaults are now first-class local containers for hosts, snippets, and identities, with a dedicated library view plus local shared-vault member/role management.
- Command history capture is prompt-aware enough to ignore alternate-screen apps and avoid learning obviously incomplete shell continuations.
- The command palette carries argument-aware command templates for common Git, Docker, Kubernetes, and systemd workflows through search and execution.
- The command palette lifts live shell context out of recent terminal output, so branch names, container targets, Kubernetes pods, and systemd units can be suggested from what the session just printed, ranked against the active working directory.
- Every terminal workspace now has a searchable command palette for snippets, recent commands, and built-in tasks, with keyboard navigation and direct execution.
- Cross-platform shortcuts now cover library section switching, settings, host-search focus, new-host flow, and Files/Terminal workspace toggles without colliding with common shell keys.
- Workspace chrome now shows aggregate runtime health for split tabs, so mixed live/connecting/error states are visible without hunting through individual panes.
- A dedicated Settings view now persists global appearance theme and terminal font-size preferences, and applies them live.
- Settings also define the default local terminal shell executable and startup directory for new local sessions.
- Settings now also control whether saved workspaces reopen on launch, how many session-history entries are retained locally, and whether to surface the first-run welcome panel again.
- Settings now include portable JSON export/import for hosts, vaults, identities, snippets, and known-host trust records, with credentials intentionally excluded.
- Settings also include passphrase-encrypted local backup export/import for the same bundle format, as groundwork for future sync without relying on plaintext files.
- TOFU host key pinning, with a Known Hosts view where you can review or delete pinned keys.
- A keys view shows imported key types and lets you add key files from disk.
- Session logs track your connection history with timestamps and duration across app restarts.
- One-click reconnect when a session drops.
- Password-backed hosts can store credentials in the system credential manager.
- Reopenable workspaces are restored after relaunch, including password sessions that can reconnect through the system credential manager.
- Window size and position are remembered across launches, including which monitor on multi-display setups.
- The top chrome is a single row with custom window controls; workspace tabs scroll horizontally when they overflow, can be renamed by double-clicking, and have a right-click menu for duplicate, rename, and close.
- Splits are a recursive tree -- drop a tab onto any pane to split that pane, nest freely, and drag the dividers to resize.
- Local terminal sessions start in your home directory rather than wherever the app was launched.
- Runtime logs are written to a file in the app data directory, so issues can be diagnosed without a console.

## Platforms

The app is being built as a native Rust desktop client for Windows, macOS, and Linux. Secure credentials use each platform's native credential backend through `keyring`, and config/state storage already uses cross-platform user directories.

Cross-platform parity work is still in progress in the UI and packaging layers, so expect rough edges outside the primary development environment.

The tracked parity target is documented in [docs/termius-parity.md](docs/termius-parity.md).
The working backlog for the remaining parity push is tracked in [docs/parity-todo.md](docs/parity-todo.md).
Automated smoke testing is documented in [docs/testing.md](docs/testing.md); run `./scripts/auto-test.sh` before release checks or larger manual QA passes.

## Not built yet

This is early alpha. The following are on the radar but don't exist yet:

- Drag-reordering split panes
- Vault sync / remote team features
- Deeper library/layout polish across all major screens
- Platform-specific packaging polish

## License

MIT
