# Termius Parity TODO

This is the working parity checklist for the native Rust desktop client. Completed items are omitted from the active backlog; this file tracks what still needs to be closed for serious Termius replacement parity.

## Active Priority

- UI parity pass:
  - final library/layout polish for hosts, snippets, and vaults
  - visual consistency pass across all library and workspace surfaces
  - settings panel restructured into a sectioned, scrollable layout with grouped cards (Appearance, Terminal, Startup, Sessions, Local Shell, Keyboard Shortcuts, Portable Data, Encrypted Backup); Terminal section now exposes copy-on-select and a custom monospace font family override; next pass should bring the same treatment to Snippets and Vaults

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
