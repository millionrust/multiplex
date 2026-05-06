# Termius Parity Notes

This project is being pushed toward a native Rust replacement for Termius on Windows, macOS, and Linux. The parity target below is based on current public Termius pages, not memory.

## Verified Termius Surface

- Core desktop platforms: Termius publicly lists macOS, Windows, and Linux as supported desktop platforms. Source: <https://www.termius.com/>
- Core navigation model: the product surface prominently includes Vault, SFTP, Workspace, Hosts, Keychain, Port forwarding, Snippets, Known Hosts, and Logs. Source: <https://www.termius.com/>
- Productivity workflow: the homepage explicitly calls out IDE-style autocomplete, one-click connect, and workspace restore. Source: <https://www.termius.com/>
- Data model: official docs define Hosts, Groups, Vaults, Keys, Identities, Snippets, Port Forwarding rules, Known Hosts, and History Items as first-class concepts. Source: <https://termius.com/documentation/glossary>
- Team model: official docs describe Vaults as end-to-end encrypted shared storage for Hosts, Groups, Keys, Snippets, Port Forwarding rules, and Known Hosts. Source: <https://termius.com/documentation/what-is-termius>
- Shared operations: official docs describe shared snippets as a central command repository inside vaults. Source: <https://termius.com/documentation/collaborate>
- Jump hosts: official docs support multiple jump hosts in a chain for one-click access to a target machine. Source: <https://termius.com/documentation/jump-hosts>
- Credential sync and sharing: official docs cover syncing keys/passwords through an encrypted vault and secure credential sharing across teams. Sources: <https://termius.com/documentation/secure-credentials-sync>, <https://termius.com/documentation/secure-credentials-sharing>
- Local terminal: official docs expose a local terminal on desktop, with platform-specific packaging limitations. Source: <https://termius.com/documentation/local-terminal>

## Current State In This Repo

- Implemented: SSH hosts, quick connect, imported SSH config hosts, reusable local identities, snippets with pinned workspace quick actions, starred hosts, host groups/tags with broader library search, batch host selection with bulk star/group actions, saved-group management cards with group-level select/bulk-target/load actions, host-group defaults with inheritance for username/tags/identity/jump/startup/forwarding settings, per-host startup directory/command preferences plus per-host session preferences for connect view and scrollback depth, multiple saved port-forwarding rules per host including local TCP tunnels, remote reverse tunnels, and dynamic SOCKS5 proxies, multi-hop jump-host chains, known-host pinning, tabs, split panes, reconnect, logs, restorable workspaces, a workspace-level SFTP remote-files browser with upload/download/delete, a native local terminal with configurable default shell settings, first-class local vault containers for hosts/snippets/identities with local shared-vault member/role metadata, prompt-aware command capture, keyboard-selectable inline command autocomplete sourced from target-aware ranked snippets/history/built-ins plus path-aware suggestions from remote-file/startup context, argument-aware command templates for common Git/Docker/Kubernetes/systemd flows, and live shell-context suggestions derived from recent terminal output with current-directory-aware ranking, a searchable command palette for direct command execution with built-in and live-context command metadata, clearer workspace chrome with aggregate split-pane runtime health, more actionable library/workspace empty states, a dismissible first-run Hosts onboarding panel with quick-start actions and import guidance, cross-platform global shortcuts for section navigation, settings, host search, and Files/Terminal workspace toggles, and a persisted settings surface for theme/font preferences, startup restore behavior, session-history retention, plus portable data export/import including known-host trust records and passphrase-encrypted local backups.
- Implemented: cross-platform secure credential storage through the system credential store via `keyring`.
- Partially aligned: vaults now exist locally in the data model and library UI, and shared vaults have local member/role metadata. Passphrase-encrypted local backups now exist as sync groundwork, but encrypted account sync/sharing transport, invitations, and remote collaboration flows are still missing.
- Partially aligned: reusable identities now exist, but deeper identity metadata and sharing flows are still missing.
- Missing for serious parity: vault sync, remote shared/team workflows, deeper layout/polish work across the full desktop surface, and packaging polish.

## Recommended Build Order

1. Identity model and host/group organization.
2. Port forwarding and jump-host chains.
3. Vault sync/team collaboration architecture.
4. Host administration workflows and UX polish.
5. Packaging and platform-native distribution.

## UI Direction

- Keep all interaction colors semantic and globally tokenized.
- Preserve a native desktop feel instead of copying Electron-style chrome.
- Match Termius feature depth where it matters, but exceed it on speed, keyboard flow, and platform correctness.
