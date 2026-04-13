# Termius Parity TODO

This is the working desktop parity backlog for the Rust client. It tracks the remaining product surface after the current host/workspace/SFTP/local-terminal/vault/autocomplete foundation.

## Now

- Keep the host editor and workspace chrome clear when multiple routing features are active.
- Deepen forwarding further with richer per-rule metadata and runtime inspection/management.
- Preserve backward compatibility for existing saved state and restored workspaces.

## Next

- Add richer shell intelligence:
  - prompt-aware command capture
  - argument and path suggestions
  - command metadata and categories
  - command metadata from shell context
- Deepen host organization:
  - stronger group views and inheritance
  - richer per-host session preferences
- Improve terminal/workspace polish:
  - better empty states
  - more informative workspace status
  - stronger cross-platform shortcut and focus behavior

## Major Remaining Gaps

- Real encrypted vault sync across devices
- Account/device reconciliation and conflict handling
- Shared-vault invitations and remote collaboration workflows
- Credential sharing / synced identities parity
- Packaging and platform-native distribution polish
- UI polish pass for desktop parity and consistency

## Acceptance Bar

- Feature parity should not regress existing saved-state compatibility.
- New behavior needs tests when it changes persistence, restore, or protocol behavior.
- UI changes should use global semantic theme tokens instead of ad hoc colors.
