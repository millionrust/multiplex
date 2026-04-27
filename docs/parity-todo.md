# Termius Parity TODO

This is the working parity checklist for the native Rust desktop client. Completed items are omitted from the active backlog; this file tracks what still needs to be closed for serious Termius replacement parity.

## Active Priority

- UI parity pass:
  - final library/layout polish for snippets and vaults
  - visual consistency pass across all library and workspace surfaces
  - settings panel restructured into a sectioned, scrollable layout with grouped cards (Appearance, Terminal, Startup, Sessions, Local Shell, Keyboard Shortcuts, Portable Data, Encrypted Backup); Terminal section now exposes copy-on-select and a custom monospace font family override; next pass should bring the same treatment to Snippets and Vaults
- Hosts:
  - host cards show a Last-connected badge derived from session logs and a per-host description row
  - host description, port-forward labels, vault, and jump host are searchable from the library filter
- Workspaces:
  - per-workspace Broadcast Input mirrors keystrokes/paste across panes, with a Broadcasting badge on each pane header
  - per-pane Clear (scrollback + remote screen) and Duplicate actions, alongside keyboard shortcuts (Cmd+D duplicate, Cmd+Shift+B broadcast, Cmd+Shift+L clear, Cmd+Alt+Left/Right cycle tabs)
- Sessions:
  - SSH panes auto-reconnect after non-user-initiated drops, with configurable attempt count and delay (Off / 1 / 3 / 5 / 10 attempts, 2/5/10/30 second delays); local shells are deliberately excluded
  - SSH keep-alive (Off / 15s / 30s / 60s / 120s) prevents NAT and load-balancer timeouts on idle sessions
- Snippets:
  - {{HOST}}, {{USER}}, {{PORT}}, {{TITLE}}, {{ADDRESS}} placeholders expand against the active pane's connection request before each snippet is sent
- Hosts (visual signaling):
  - per-host color tag with eight presets, surfaced on the host card avatar and the connected pane status dot
  - per-host environment variables exported into the remote shell before the startup directory and command
- Pane and workspace ergonomics:
  - inline rename for both workspace tabs and individual panes (click title to edit, Enter to commit, Esc to cancel)
  - Detach button moves a split pane into its own new workspace tab while keeping the session alive
  - Cmd+T opens a fresh local terminal in a new workspace tab
- Terminal safety:
  - configurable confirmation banner before sending a multi-line clipboard into a remote shell

## Platform And Packaging

- macOS: cargo-bundle metadata is in place (`[package.metadata.bundle]` in Cargo.toml). `cargo bundle --release` produces `TermiRust.app`. Signed/notarized distribution requires an Apple Developer Program membership; flow documented in docs/building.md.
- Windows: cargo-wix path documented. Signed MSI requires a CA-issued code-signing certificate; flow documented in docs/building.md.
- Linux: `cargo bundle --release --format deb` / `--format rpm` produces packages. AppImage path documented. Snap/Flatpak manifests still TODO.
- Auto-update: not yet wired. Path documented (self_update crate + signed manifests on HTTPS origin) but not implemented.
- Icons: `assets/icons/app.png` (1024x1024) still needs to be added before bundling will succeed.

## Vault / Team / Sync

- shared-folder sync (Dropbox / iCloud Drive / Google Drive / Syncthing) is wired: Settings -> Shared-folder sync card pushes/pulls the encrypted bundle through any cloud-synced folder, no server required. Covers the solo-user-on-multiple-machines case end-to-end.
- still missing: account-based sync for users without a cloud drive, automatic device reconciliation, last-write conflict resolution, shared-vault invitation flow, remote collaboration workflows.

## UI Parity Pass

- deeper settings surface

## Working Rules

- Preserve backward compatibility for saved state and restored workspaces.
- Add tests when persistence, restore, or protocol behavior changes.
- Use global semantic theme tokens instead of ad hoc UI colors.
