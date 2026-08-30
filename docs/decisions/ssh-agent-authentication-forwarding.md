# SSH-Agent Authentication and Forwarding

TermiRust can authenticate SSH, remote-exec, jump-host, and SFTP connections with identities held
by a local OpenSSH-compatible agent. Agent authentication does not copy private keys into the app
and does not imply agent forwarding.

## Local agent boundary

On Unix platforms, an empty agent-socket field resolves from `SSH_AUTH_SOCK`; an explicit field
uses that absolute path. Before connecting, TermiRust requires a direct Unix socket owned by the
current effective user. Symlinks, non-sockets, relative paths, oversized paths, unavailable
sockets, empty agents, and agents exposing more than 64 identities are rejected. Listing and
signing operations have five-second timeouts. Authentication tries the bounded identity list in
agent order and never falls back to a password or private-key file.

Agent errors describe the failure class without logging or returning socket paths, key blobs,
fingerprints, signatures, or agent protocol traffic. Windows does not currently provide this
Unix-socket adapter and reports the capability as unavailable.

## One-shot forwarding

Forwarding is a separate action on the final SSH protocol screen: **Connect + Forward Agent
Once**. The warning is the confirmation for that single connection attempt. Normal Connect keeps
forwarding disabled.

The approved socket is available only to the interactive target SSH transport. It is not exposed
to jump hops, remote exec, or SFTP. Once the runtime starts, TermiRust clears forwarding from the
request stored in pane state. Reconnect, duplicate, workspace restore, fleet restore, canvas
restore, and automatic reconnect therefore return to disabled and require another explicit
action.

## Persistence and import

Profiles and restorable workspaces may retain only the optional agent socket reference. They never
persist forwarding approval, identities, signatures, or private-key material. SSH config
`IdentityAgent SSH_AUTH_SOCK` selects the environment agent, an explicit path is imported after
home expansion, `IdentityAgent none` stays disabled, and an explicit `IdentityFile` continues to
take precedence. Mobile vault export rejects agent profiles until its schema and runtime can
represent this authentication method without downgrade.
