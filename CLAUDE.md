# Multiplex

Native desktop SSH client built with `gpui`, `gpui-component`, `russh`, and `alacritty_terminal`.

## Current product shape

- Host library UI inspired by Terminus-style launchers, with groups, tags, vaults, batch selection, and bulk actions.
- Workflow navigation surfaces Activity, Projects, Connections, Sessions, Files / Artifacts,
  Devices, and Settings as primary destinations; specialized presets, vaults, keys,
  snippets, known hosts, and logs remain available as advanced tools.
- Sessions presents the authoritative active or archived typed Session library across
  all Projects and both app-attached and durable ownership routes, with Project, preset,
  and ownership context on each row.
- Files / Artifacts separates live local/SFTP browsing from the bounded Session artifact
  repository. Its artifact index spans authoritative Sessions and shows Project, Session,
  preset, state, type, size, and truthful import provenance before reusing the existing
  preview, export, quarantine, restore, and purge actions.
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
- Bounded local diagnostics store only allowlisted operational metadata; raw terminal content, stderr, panic text, and backtraces are excluded. Users can preview an exact privacy-scanned bundle before saving it locally.
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
- Remote access (Devices, or Settings → Remote Devices) is On/Off: the Controller listener
  binds every private address (RFC 1918, Tailscale's 100.64/10, fc00::/7) on one port, follows
  network changes, and announces `_multiplex._tcp` with Bonjour on LAN interfaces only.
  **Pair phone** shows a six-digit code (CPace bound into the Noise XX pairing, three attempts,
  five minutes); the QR offer and SAS comparison remain under "Other ways to pair".
- Paired mobile controllers can list, watch, and type into tmux sessions the app did not
  create when "Show tmux sessions" is on, over LAN, SSH, and relay routes. Devices also offers a previewed,
  reversible shell startup change that starts new Terminal, Zed, iTerm2, Ghostty, WezTerm,
  and VS Code terminal tabs inside tmux, and on macOS a LaunchAgent
  (`multiplex controller-service`) that keeps the LAN listener up after the app quits.
  See `docs/remote-terminals.md`.
- Saved host groups can open directly as SSH Fleet canvases. The fleet panel
  summarizes connection and tmux state and provides guarded reconnect,
  broadcast-input, and disconnect controls without removing canvas nodes.
- Local Codex, Claude Code, and Gemini structured jobs normalize provider events;
  interactive remote agents reuse saved SSH and tmux settings.
- Write-capable local coding agents default to app-managed Git worktrees with
  conservative status inspection and clean-only removal.
- Settings view for appearance theme, terminal font, default local shell, workspace restore, history limits, and import/export. A section sidebar shows one section at a time; search spans every section. Choices use the compact segmented control (`segmented_control` in `ui/app/mod.rs`).
- TOFU known-host pinning; Known Hosts view supports deleting pinned host keys.
- Keychain view shows imported key type, public key availability, and an "Add Key File" picker.
- Build/distribution metadata for cargo-bundle (macOS .app, Linux deb/rpm) lives in `crates/multiplex-desktop/Cargo.toml`; per-platform release flow is in `docs/building.md`.

## Explicitly out of scope right now

- drag-reordering split panes
- remote team / multiplayer features (shared-folder vault sync is the only sync that exists)

## Repository layout

- Root `Cargo.toml` is a virtual workspace manifest; `default-members` points at
  `crates/multiplex-desktop` (package name `multiplex`), so plain `cargo run` /
  `cargo check` / `cargo test` from the root target the desktop app. Use `--workspace`
  for every crate. Shared version and `rust-version` live in `[workspace.package]`.
- `crates/` holds every workspace member, including the desktop app, CLI, TUI, MCP,
  relay, session host, mobile FFI, and contract crates.
- `apps/ios` (Swift) and `apps/android` (Kotlin) are the native mobile applications;
  their FFI libraries are built by `scripts/build/` and copied in by `scripts/sync/`.
- `tests/` is shared cross-crate test material only: `fixtures/`, the `support/`
  module included via `#[path]`, `ui/` audit inventories, and `swift/` runners.
  Rust integration tests live inside each crate.
- `scripts/` is grouped by verb: `verify/`, `test/`, `build/`, `sync/`, `bench/`,
  `run/`, `dev/`. Every script resolves the repo root two levels up.
- `design/` and `locales/` are consumed by `multiplex-ui-contract`. `design/` also
  holds the Slate design references: `slate-design-system.html` (every token,
  component spec, and the Rust handoff) and `multiplex-design-system.html` (the
  interactive Multiplex prototype). `docs/` holds
  guides, ADRs under `decisions/`, and evidence under `completion-evidence/` and
  `engineering-evidence/`; `tools/` holds excluded spike workspaces; `dist/` is
  ignored build output.

## Important architecture

- [crates/multiplex-desktop/src/main.rs](crates/multiplex-desktop/src/main.rs)
  - Bootstraps GPUI, redirects logs to a file, restores the saved window bounds/display, registers the embedded asset source, and opens the main window.
- [crates/multiplex-desktop/src/platform_mac.rs](crates/multiplex-desktop/src/platform_mac.rs)
  - macOS window-control interop: disables the OS title-bar drag so the chrome tabs stay usable, and starts native window drags from the chrome's empty area.
- [crates/multiplex-desktop/src/assets.rs](crates/multiplex-desktop/src/assets.rs)
  - Embedded SVG asset source for the app chrome and custom Phosphor-style icons.
- `crates/multiplex-desktop/src/ui/app/` — main application state and UI, split across modules:
  - [mod.rs](crates/multiplex-desktop/src/ui/app/mod.rs) — `MultiplexApp` state, event loop, recursive split tree, window-bounds persistence.
  - [chrome.rs](crates/multiplex-desktop/src/ui/app/chrome.rs) — top chrome: tab strip, traffic lights, tab context menu.
  - [workspace.rs](crates/multiplex-desktop/src/ui/app/workspace.rs) — terminal pane rendering, split layout, SFTP files view.
  - [editor.rs](crates/multiplex-desktop/src/ui/app/editor.rs) / [hosts.rs](crates/multiplex-desktop/src/ui/app/hosts.rs) / [library.rs](crates/multiplex-desktop/src/ui/app/library.rs) — host editor and library.
  - [connect.rs](crates/multiplex-desktop/src/ui/app/connect.rs) / [sftp.rs](crates/multiplex-desktop/src/ui/app/sftp.rs) / [palette.rs](crates/multiplex-desktop/src/ui/app/palette.rs) / [overlay.rs](crates/multiplex-desktop/src/ui/app/overlay.rs) / [types.rs](crates/multiplex-desktop/src/ui/app/types.rs).
  - [canvas.rs](crates/multiplex-desktop/src/ui/app/canvas.rs) — canvas geometry, interaction, terminal and
    agent nodes, links, worktree controls, and orchestration UI.
- `crates/multiplex-desktop/src/agents/` — safe process launch, normalized protocols, provider adapters,
  context redaction, worktree ownership, and dependency scheduling.
- [crates/multiplex-desktop/src/ui/theme.rs](crates/multiplex-desktop/src/ui/theme.rs)
  - App color system and layout constants.
- `crates/multiplex-tmux/` — the one GPUI-free tmux integration: binary discovery, bounded
  `list-sessions` parsing, attach arguments, a listing self-check, and
  `shell_integration` (the previewed, conflict-checked startup-file change and its removal).
  The Controller listener's `tmux_sessions.rs` uses it to publish tmux sessions and attach
  through shared in-process Session Hosts running `tmux attach-session -f ignore-size`.
- `crates/multiplex-slate/` — Slate, the styled component library the desktop app will move to.
  - Built on `gpui-base` 0.6 (behavior: focus, keyboard, overlays, accessibility) over the
    `gpui-pre` 0.3 snapshots, which coexist in the workspace with the app's `gpui` 0.2.2.
    The desktop app cannot use Slate until it migrates from `gpui` 0.2 / `gpui-component`
    to `gpui-pre` + `gpui-base`.
  - [theme.rs](crates/multiplex-slate/src/theme.rs) resolves `DesignTokens` into GPUI colors,
    sizes, type, and shadows per `ThemeChoice`, installs them as a global, and projects them
    onto `gpui_base::Theme` so base inputs share the palette. Components read only from it.
  - [icon.rs](crates/multiplex-slate/src/icon.rs) generates the 16-unit stroke icon set and the
    eight status glyph shapes as SVG; serve them with `SlateAssets` (or `with_fallback`).
  - Primitives: `button.rs`, `controls.rs` (Segmented, Toggle, FilterTabs), `input.rs`,
    `kbd.rs`, `status.rs`, `tooltip.rs`. Shell: `shell.rs`. Data views: `data.rs`.
    Overlays: `overlay.rs` (menus, palette, dialogs, toasts, banners). Terminal chrome:
    `terminal.rs`. `split.rs` holds the `SplitNode` tree (four-pane cap, 0.15–0.85 ratios)
    and `SplitPanes`, which draws dividers and edge drop zones.
  - Callbacks follow GPUI's `Fn(&Event, &mut Window, &mut App)` shape so `cx.listener` fits.
  - [examples/gallery.rs](crates/multiplex-slate/examples/gallery.rs) renders every
    component and state in a working shell; `SLATE_THEME`, `SLATE_TAB`, `SLATE_OVERLAY`,
    and `SLATE_SCROLL` start it in a given state for screenshots.
- [crates/multiplex-desktop/src/terminal.rs](crates/multiplex-desktop/src/terminal.rs)
  - Terminal emulation over `alacritty_terminal`: a theme-resolved snapshot of the visible cells and cursor, scrollback, selection text, mode inspection, replies owed to the program (cursor position and device attribute reports), and a controller snapshot byte stream.
  - It re-wraps lines on resize and keeps the cursor on entering the alternate screen, as xterm does. The shared conformance fixtures follow vt100 there; `terminal.rs` tests pin those three cases to the xterm behavior.
  - It answers what a program asks about the terminal, because tmux waits for some of these
    before it finishes attaching and a terminal interface library (OpenTUI, which Codex and
    Claude Code draw with) asks the same questions directly: device attributes, `DECRQM` for
    synchronized updates, bracketed paste and focus (declining 2027, 2031 and 1016 honestly),
    the foreground and background colours, the terminal's name and version (`CSI > q`), and
    whether it is light or dark (`CSI ? 996 n`). The last two are recognised in
    `answer_name_and_version` because `alacritty_terminal` ignores them. `the_terminal_answers_*`
    tests cover every one of these.
- [crates/multiplex-desktop/src/ui/app/terminal_grid.rs](crates/multiplex-desktop/src/ui/app/terminal_grid.rs)
  - Per-pane grid entity that paints the snapshot the way Zed's terminal does: same-style cells batched into runs shaped with a forced cell-width advance, merged background rectangles, and a separately painted cursor. Session output wakes the app's event loop (`SshEventSender`) and is drawn immediately, then gathered in `motion.terminal_output_batch` windows.
- [crates/multiplex-desktop/src/ssh.rs](crates/multiplex-desktop/src/ssh.rs)
  - SSH runtime thread and Tokio event loop: shell open, PTY allocation, raw input/output, and remote resize.
- [crates/multiplex-desktop/src/local.rs](crates/multiplex-desktop/src/local.rs)
  - Local PTY shell sessions (started in the user's home directory).
  - Panes are given `TERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION` and `COLORTERM=truecolor`.
    The last one matters: `xterm-256color` cannot say that this terminal draws 24-bit colour,
    and a tmux the user starts themselves reads `COLORTERM` to turn its own RGB support on. It
    is also in `multiplex-tmux`'s `FORWARDED_ENVIRONMENT` so app-started clients carry it.
- [crates/multiplex-desktop/src/sftp.rs](crates/multiplex-desktop/src/sftp.rs)
  - SFTP runtime backing the remote-files view.
- [crates/multiplex-desktop/src/credentials.rs](crates/multiplex-desktop/src/credentials.rs)
  - System credential-store (keyring) access for saved passwords.
- [crates/multiplex-desktop/src/models.rs](crates/multiplex-desktop/src/models.rs)
  - Saved host models, draft parsing, connect-request generation, and persisted window bounds.
- [crates/multiplex-desktop/src/storage.rs](crates/multiplex-desktop/src/storage.rs)
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
TERMIRUST_TRACE_FOCUS=1 cargo run   # log every keyboard focus change to stderr
cargo test --workspace --all-targets --locked --no-fail-fast   # everything, as CI runs it
cargo nextest run --workspace --lib --bins --tests --examples --locked  # the same minus the
  # benches, as the Windows job runs it, in about two thirds of the time. nextest gives every
  # test its own process, so what may not run at once is declared in .config/nextest.toml.
TERMIRUST_TUI_PROBE="bun run app.ts" cargo test -p multiplex --bin multiplex -- \
  a_terminal_interface_program_renders --ignored --nocapture   # drive a real TUI program
                                                               # through the emulator
TERMIRUST_CLIPPY_BASE=<sha> python3 scripts/dev/clippy-changed.py  # the changed-line Clippy
  # policy as CI runs it. Its base defaults to HEAD, so running it with a clean working tree
  # reads no changed lines and always passes; CI passes the sha the push started from.
TERMIRUST_PERF_BUDGETS=1 cargo test --workspace --bench '*' --locked  # enforce the throughput
  # budgets. The benches always run and always print their p50/p95; the thresholds in
  # tests/support/perf_budget.rs are only asserted when `CI` is unset, because a hosted runner
  # overshoots them by more than ten times without anything in the code changing. Set this to
  # enforce them on a machine that sets `CI` but is not shared, or `=0` on a busy laptop.
cargo run -p multiplex-slate --example gallery  # Slate component gallery
cargo run -p multiplex-ui-contract --bin generate-tokens  # after editing design/tokens.toml; also writes the mobile SlateTokens.swift and SlateTokens.kt
```

The Docker-backed SSH/SFTP tests carry their fixture files in the image, so they also run
against a daemon on another machine through `DOCKER_HOST`. Such a daemon publishes the
fixture's port on its own machine, so set `TERMIRUST_DOCKER_FIXTURE_HOST` to an address that
machine is reachable at when its own name does not resolve to one (a host on several private
networks). Tests skip themselves when no daemon answers.

On macOS, GPUI may need access to the system shader cache during first compile/run.

On macOS, `.cargo/config.toml` runs binaries through `scripts/dev/run-signed.sh`, which re-signs the
desktop app with a stable identifier and your Apple Development identity (or
`TERMIRUST_CODESIGN_IDENTITY`) so Keychain and Local Network permissions survive rebuilds.

Structured diagnostics are stored under `<data dir>/multiplex/diagnostics` with
bounded rotation and retention. See [docs/diagnostics.md](docs/diagnostics.md).

## UI behavior details

- Clicking a host card loads it into the editor panel.
- Connecting opens a new workspace tab; the tab strip auto-scrolls to keep the active tab visible.
- Dragging a workspace tab reorders tabs; dropping onto a tab inserts before it; dropping in the empty strip after the last tab moves the tab to the end.
- Dropping a tab onto a terminal pane splits that pane.
- Double-clicking the empty chrome area opens a new local terminal.
- Active workspace search is local to the active pane; search and unread badges are per workspace tab.
- Typing in a terminal pane offers suggestions above it, drawn from snippets, command history,
  built-in templates, the browsed path, and recent output. Up and Down choose one, Enter accepts
  it in place of what was typed, and Escape puts them away, which is what the Settings shortcut
  list says. Nothing is offered for an empty line or on the alternate screen, so a full-screen
  program and a pane nobody is typing into behave as they always have. The line is followed by
  the bytes sent to the program, and anything that could rewrite it from elsewhere — Tab
  completion, Ctrl-C, Ctrl-U — gives it up rather than guess.

## Known implementation limits

- Mouse reporting is practical, not exhaustive protocol coverage.
- Search is plain substring search over terminal text, not regex.
- The `Keychain` imports keys from `~/.ssh` and allows picking files from disk, but does not generate keys.
- SSH config hosts are imported at startup (shown with an `SSH Config` badge) and runtime-synced, not written back into the app state file.
- Quick connect uses the first available SSH key; for password-only auth, use the host editor form.
- Durable hosted sessions still poll their Host for output every `motion.hosted_live_poll` (40 ms); SSH and local panes are drawn as output arrives.
- A file dropped on a wrapped tab that is scrolled back reaches the program whole only on tmux
  3.6 and newer. A drop arrives bracketed, which tmux reads as one key, and what happens next
  splits three ways: 3.4 and 3.5a consume it in copy mode without offering it to a binding, so
  the catch-all never runs and the tab keeps the file; 3.2a and 3.3a do run the catch-all and
  leave copy mode, but do not consume the paste's closing `ESC[201~`, so its bytes reach the
  program after the path and show up as `^[[201~` or a bare `~` depending on how the shell
  echoes them; 3.6 and newer deliver the path alone. Measured against 3.2a,
  3.3a, 3.4, 3.5a, 3.6 and 3.7c;
  `a_file_dropped_on_a_scrolled_back_tab_reaches_the_program` holds all three bands.
- Typing by hand into a wrapped tab that is scrolled back returns it to the prompt but loses
  that first character: tmux answers the catch-all copy-mode binding before any binding for the
  key itself and tells it nothing about which key ran it, so the binding cannot send the key on.
  Binding every printable key to send itself does not help — tmux still runs the catch-all, and
  a key sent from the binding that is leaving copy mode is swallowed. A dropped file is not
  affected: it arrives bracketed, as one key, and the path that follows reaches the program
  whole. `a_file_dropped_on_a_scrolled_back_tab_reaches_the_program` covers it.
- A tmux session the app did not configure cannot discover synchronized updates up to tmux
  3.7c: that build has the `Sync` output capability but no `[?2026$p` probe, so answering the
  query changes nothing and a full-screen program inside such a session tears while it
  redraws. Colour is fine there (`COLORTERM` reaches tmux), and the app's own wrapped tabs pass
  `-T RGB,sync` explicitly. Terminal.app (488) sets `COLORTERM=truecolor` itself, so colour
  works in its tabs too, but it has no synchronized updates at all and is deliberately absent
  from `SYNCHRONIZED_UPDATE_PROGRAMS`.
- Windows notes, all of which have bitten this workspace: keep text files LF (`.gitattributes`
  enforces it, and the signed update fixtures fail verification if Git rewrites them); a path
  is absolute there only with a drive behind it, so fixtures cannot use `/bin/sh` or
  `/usr/...`; committing a rename by opening its directory as a file is refused, so treat that
  as reduced durability rather than failure; a held lock reports a lock violation rather
  than `WouldBlock`, which `fs2::lock_contended_error()` names per platform; and a process that
  exits with local panes open leaves their console hosts behind, each spinning a core on Windows
  Server 2022, which starved every later test of a nextest run until `local/console_job.rs` put
  them in a job object that ends them with the process.
