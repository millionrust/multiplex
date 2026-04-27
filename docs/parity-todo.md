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

## Platform And Packaging

- Windows packaging/distribution flow
- Linux packaging/distribution flow
- installer/updater strategy
- platform-native file dialogs, shortcuts, and clipboard edge cases

## Vault / Team / Sync

- real encrypted vault sync across devices
- account/device reconciliation and conflict handling
- shared-vault invitations
- remote collaboration workflows
- credential sharing / synced identities parity

## UI Parity Pass

- deeper settings surface

## Working Rules

- Preserve backward compatibility for saved state and restored workspaces.
- Add tests when persistence, restore, or protocol behavior changes.
- Use global semantic theme tokens instead of ad hoc UI colors.
