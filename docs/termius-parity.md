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

- Implemented: SSH hosts, quick connect, imported SSH config hosts, imported local identities, known-host pinning, tabs, split panes, reconnect, logs, and restorable workspaces.
- Implemented: cross-platform secure credential storage through the system credential store via `keyring`.
- Partially aligned: key management exists, but there is no Termius-style identity model yet.
- Missing for serious parity: groups, vaults, snippets, SFTP, port forwarding, jump-host chains, local terminal, command autocomplete, shared/team workflows, and packaging polish.

## Recommended Build Order

1. Identity model and host/group organization.
2. Port forwarding and jump-host chains.
3. SFTP and local terminal.
4. Snippets and reusable command workflows.
5. Vault/sync/team collaboration architecture.
6. Command autocomplete and deeper UX polish.

## UI Direction

- Keep all interaction colors semantic and globally tokenized.
- Preserve a native desktop feel instead of copying Electron-style chrome.
- Match Termius feature depth where it matters, but exceed it on speed, keyboard flow, and platform correctness.
