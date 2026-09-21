# Multiplex

**Your terminals and screens, wherever you are.**

Multiplex connects your servers, your computers, and your phone. It's an SSH client for the servers you
manage, and it also lets you reach the computers that run it. From your phone or another desktop you can
see every paired Mac, Windows PC, or Linux machine, view its screen, and use the terminals running on it.
It's native on every platform, with no Electron and no browser: a Rust desktop app plus Swift and Kotlin
mobile apps.

![Windows](https://img.shields.io/badge/platform-Windows-lightgrey)
![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Linux](https://img.shields.io/badge/platform-Linux-lightgrey)
![iOS](https://img.shields.io/badge/platform-iOS-lightgrey)
![Android](https://img.shields.io/badge/platform-Android-lightgrey)
![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)
![Status](https://img.shields.io/badge/status-early%20alpha-yellow)

---

## What it does

Multiplex covers three things:

1. **SSH into your servers.** Save your servers in a host library and connect with a click, or type
   `user@host` for a quick connection. Split panes, SFTP file transfer, port forwarding, jump hosts,
   snippets, and a command palette are built in.
2. **Reach every computer that runs Multiplex.** Pair a phone (iOS or Android) or a second desktop
   with a computer running the Multiplex desktop app. The Devices screen lists every paired computer.
   You can reach them over the local network, Tailscale or another VPN, SSH, or a self-hosted relay.
3. **See their screens and use their terminals.** Open a paired computer to view its whole screen,
   and control it with a pointer and keyboard. You can also list, watch, and type into the terminals
   running on it.

### Every terminal on the computer, not only the app's own

An operating system never lets one program adopt a terminal that another app already opened. So
Multiplex uses **tmux**. Once you turn it on, new tabs in Terminal, iTerm2, Ghostty, WezTerm, Zed, and
VS Code start inside tmux. The desktop listens for your paired devices, so every terminal on that
machine is reachable from your phone, including the ones Multiplex didn't open itself.

The setup is opt-in and reversible. It shows the exact startup-file change before making it, never
edits `~/.tmux.conf`, and can be removed from the same screen. On macOS, a background service keeps
the machine reachable after the app quits.

### Where each part runs today

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| SSH client, split panes, SFTP, forwarding | Yes | Yes | Yes |
| Screen viewing from a paired device | Yes | Not yet | Yes |
| Screen control (pointer and keyboard) from a paired device | Yes | Not yet | Not yet |
| Every terminal on the machine reachable from the phone (tmux) | Yes | Yes | Not yet ([plan](docs/remote-terminals.md#windows)) |
| Reachable after the app quits | Yes (LaunchAgent) | Over SSH or relay | Not yet |

This is early alpha, and remote screens still need testing on real devices. On Linux the capture
and input code exists, but the Wayland portal only picks a screen once sharing starts, so the
share-a-screen flow still has to be built; on Windows, input injection exists but is not yet wired
into the app. See
[docs/remote-terminals.md](docs/remote-terminals.md) and
[docs/remote-screens-implementation-plan.md](docs/remote-screens-implementation-plan.md).

## What you get

Multiplex has a host library where you save connection details once. Double-click a host and it connects straight away, showing its progress, the connection log, and any error with a Retry button until the terminal is live. Want to watch logs on one server while poking at another? Split the workspace into up to 4 panes, each running its own SSH session.

There's also a quick-connect bar. Type `user@host` or `ssh user@host:port` and you're in, no need to save anything first.

The terminal is built the way Zed's is. [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) does the emulation: lines re-wrap on resize, programs can set their colors, and cursor-position and device queries are answered. A custom [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) element paints the grid, batching same-style cells into runs shaped to an exact cell width. Output is drawn as soon as it arrives rather than on a timer. You also get scrollback, in-terminal search, text selection, clipboard copy/paste, xterm mouse reporting (so `htop`, `vim`, and friends work), beam and underline cursors, Nerd Font fallback, and Powerline separators drawn as shapes.

## Your terminals on your phone

The details behind reaching a computer's terminals from the iOS and Android apps:

- **Remote access** listens on every private address the computer has (Wi-Fi, Ethernet, and VPNs such as Tailscale) and announces itself with Bonjour on the local network.
- **Pairing** is a six-digit code shown on the desktop and typed on the phone, protected by a PAKE (CPace) so a code cannot be guessed offline. Scanning a QR code still works.
- **Terminals in other apps** can be reached too: turn on "Open new terminals in tmux" and new tabs in Terminal, Zed, iTerm2, Ghostty, WezTerm, and VS Code start inside tmux. The setup previews every file it writes. It configures tmux for those sessions only, with the mouse scrolling and selecting and no status bar, and never edits `~/.tmux.conf`.
- **Control** from the phone is explicit: new devices are view-only until you allow input, and the phone keeps the desktop terminal's size by default.

See [docs/remote-terminals.md](docs/remote-terminals.md).

Other things worth knowing about:

- Every workspace can switch between the recursive Split layout and a persisted
  Agent Canvas containing local terminals, saved SSH hosts, and coding-agent
  nodes. See [docs/agent-canvas.md](docs/agent-canvas.md).
- Codex, Claude Code, Gemini CLI, and safe custom executables can run as normal
  interactive terminal nodes. Local Codex, Claude, and Gemini also have
  structured modes for explicit status, cancellation, approvals, reviewed
  context handoff, and bounded dependency runs.
- Write-capable local agents default to separate app-managed Git worktrees.
  Cleanup refuses active, dirty, or committed worktrees.
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
- One-click reconnect when a session drops, with the error shown in the pane.
- Automatic reconnects in background tabs never take keyboard focus from the terminal you are typing in.
- Password-backed hosts can store credentials in the system credential manager.
- Reopenable workspaces are restored after relaunch, including password sessions that can reconnect through the system credential manager.
- Window size and position are remembered across launches, including which monitor on multi-display setups.
- The top chrome is a single row with custom window controls; workspace tabs scroll horizontally when they overflow, can be renamed by double-clicking, and have a right-click menu for duplicate, rename, and close.
- Splits are a recursive tree -- drop a tab onto any pane to split that pane, nest freely, and drag the dividers to resize.
- Local terminal sessions start in your home directory rather than wherever the app was launched.
- Runtime logs are written to a file in the app data directory, so issues can be diagnosed without a console.

## Platforms

The app is being built as a native Rust desktop client for Windows, macOS, and Linux. Secure credentials use each platform's native credential backend through `keyring`, and config/state storage already uses cross-platform user directories.

The native mobile clients are part of the same repository: the Swift iOS/iPadOS
application lives in [`apps/ios`](apps/ios), and the Kotlin Android
application lives in [`apps/android`](apps/android). Run
`scripts/verify/mobile-mvp.sh` from the repository root to validate the shared
Rust protocol and both mobile projects together.

Cross-platform parity work is still in progress in the UI and packaging layers, so expect rough edges outside the primary development environment.

The tracked parity target is documented in [docs/termius-parity.md](docs/termius-parity.md).
The working backlog for the remaining parity push is tracked in [docs/parity-todo.md](docs/parity-todo.md).
Automated smoke testing is documented in [docs/testing.md](docs/testing.md); run `./scripts/test/auto.sh` before release checks or larger manual QA passes.

## Not built yet

This is early alpha. The following are on the radar but don't exist yet:

- Drag-reordering split panes
- Remote team / multiplayer features (encrypted shared-folder vault sync is the only sync)
- Windows support for reaching terminals from the phone
- Deeper library/layout polish across all major screens
- Platform-specific packaging polish

## Developing on macOS

`cargo run` re-signs the desktop app with a stable identifier through `scripts/dev/run-signed.sh`, so Keychain and Local Network permissions granted once survive rebuilds. It uses your first Apple Development identity, or `MULTIPLEX_CODESIGN_IDENTITY`. Set `MULTIPLEX_TRACE_FOCUS=1` to log every keyboard focus change.

## Repository layout

```
crates/            Cargo workspace members; crates/multiplex-desktop is the GPUI desktop app
apps/              Native iOS/iPadOS (apps/ios, Swift) and Android (apps/android, Kotlin) applications
tools/             Standalone spike workspaces with their own lockfiles
scripts/           Automation grouped by verb: verify/, test/, build/, sync/, bench/, run/, dev/
tests/             Shared cross-crate test assets: fixtures/, support/, ui/ audits, swift/ runners
design/, locales/  Design tokens and localization catalogs consumed by multiplex-ui-contract
docs/              Guides, ADRs (decisions/), and completion/engineering evidence
dist/              Ignored build output for mobile FFI artifacts (see scripts/build/)
```

The root `Cargo.toml` is a virtual workspace manifest whose default member is the
desktop crate, so `cargo run`, `cargo check`, and `cargo test` from the root still
target the app; add `--workspace` to cover every crate.

## License

MIT
