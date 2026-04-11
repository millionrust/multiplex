# Terminal

A native desktop SSH client. Save your servers, connect in a click, split terminals side by side. No Electron, no browser -- just a Rust binary talking to your boxes over SSH.

![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)
![Status](https://img.shields.io/badge/status-early%20alpha-yellow)

---

## What you get

Terminal has a host library where you save connection details once. Click a host, get a terminal. Want to watch logs on one server while poking at another? Split the workspace into up to 4 panes, each running its own SSH session.

There's also a quick-connect bar. Type `user@host` or `ssh user@host:port` and you're in, no need to save anything first.

The terminal itself does VT100 rendering, scrollback, in-terminal search, text selection, clipboard copy/paste, and xterm mouse reporting (so `htop`, `vim`, and friends work). It's rendered through [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the GPU-accelerated framework from the Zed editor.

Other things worth knowing about:

- Your `~/.ssh/config` hosts and `~/.ssh` keys are imported automatically at launch, so your servers should already be there when you first open it.
- Workspace tabs are draggable. You can reorder them.
- TOFU host key pinning, with a Known Hosts view where you can review or delete pinned keys.
- A keychain view shows imported key types and lets you add key files from disk.
- Session logs track your connection history with timestamps and duration across app restarts.
- One-click reconnect when a session drops.
- Reopenable workspaces are restored after relaunch for sessions that can reconnect without storing passwords.

## macOS only (for now)

GPUI's Linux support is in progress upstream. Once that lands, Linux builds should follow. Windows is further out.

## Not built yet

This is early alpha. The following are on the radar but don't exist yet:

- SFTP / file transfers
- Port forwarding
- Snippets or command templates
- Securely restoring password-auth tabs after relaunch
- Drag-reordering split panes
- Linux and Windows builds

## License

MIT
