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
- Hosts can optionally start a local TCP forward when they connect.
- Hosts can optionally connect through a saved jump host.
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

## Not built yet

This is early alpha. The following are on the radar but don't exist yet:

- SFTP / file transfers
- Drag-reordering split panes
- Platform-specific packaging polish

## License

MIT
