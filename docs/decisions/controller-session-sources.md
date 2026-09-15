# Controller Session Sources Decision

Status: Accepted for LAN/VPN, Controller-over-SSH, and self-hosted relay routes; remote exposure remains gated by D06 and the independent cryptographic review

Reviewed: 2026-09-15

## Context

A paired Controller could list and attach three kinds of terminal, but not on every route:

| Source | LAN/VPN listener | Controller-over-SSH | Self-hosted relay |
| ------ | ---------------- | ------------------- | ----------------- |
| Durable sessions (Session Host records) | yes | yes | yes |
| Live desktop panes | yes | no | no |
| tmux sessions the app did not create | yes (new) | no | no |

The LAN listener receives its sources in a launch descriptor from the desktop app. The SSH
and relay routes run `serve_repository_stdio_bridge` in a separate process
(`termirust controller-bridge --stdio` and `termirust relay-host run`) and received no
sources beyond the repositories. A phone therefore saw different terminals depending on how
it reached the computer, which made the SSH route look broken.

tmux discovery is new. It lists every session on the user's default tmux server and attaches
through an in-process Session Host running `tmux attach-session -f ignore-size`, so input
reaches the user's real shell.

## Decision

Every route serves the same sources, through one type, `RepositoryBridgeSources`:

- **Live desktop panes** reach a bridge process through a pointer file. The desktop app writes
  `<runtime parent>/desktop-pane-bridge.json` when its pane bridge starts and removes it when
  the bridge stops. The file holds only a schema tag, a version, and the endpoint id. Its
  socket location is always derived from the pointer's own location, never read from it, so
  the file cannot redirect a reader. A reader ignores the file unless it is a regular,
  non-symlink file owned by the current user with no group or other permission bits.
- **tmux sessions** are served only when the user turned on "Show tmux sessions". The LAN
  listener receives the setting in its launch descriptor; the SSH and relay bridges read the
  saved setting at the start of each connection, so turning sharing off applies to the next
  connection on every route.

No wire change is made. tmux rows use the existing `origin: terminal`, `runtime: "tmux"`, and
`lifecycle: "live"` fields.

## Trust Boundary

The Controller trust boundary is unchanged: Noise XX authentication, per-device capability
bits, revocation epochs, occupant-generation fencing, and the single writer lease gate every
command on every route, exactly as for durable sessions. A device without `SendInput` cannot
type into a tmux session or a live pane on any route.

What changes is the set of terminals behind that boundary. Consequences accepted:

- A device paired for input can type into any tmux session on the default server, including
  ones the user started by hand, once sharing is on. Sharing is off by default and is a
  separate, explicit choice from pairing.
- The SSH and relay routes now reach live panes and tmux sessions. The relay still carries
  only Controller-v1 ciphertext; it never sees titles, session ids, or terminal bytes.
- The pane bridge socket and its pointer are protected by user-only file permissions and
  peer-credential checks, the same as Session Host sockets. Any process running as the same
  user could already reach Session Host sockets; the pointer adds no new class of access.
- tmux attach hosts keep their output journals in a random user-only directory under the
  runtime parent and delete them when the last connection detaches.

## Rejected Alternatives

- **Passing the pane endpoint on the command line of the SSH forced command.** The command is
  fixed in `authorized_keys` and cannot know a per-launch endpoint id.
- **A fixed pane endpoint id.** A stale socket from a crashed app would be indistinguishable
  from a live one, and the id would stop varying per launch.
- **`tmux send-keys` for input with a read-only client.** tmux 3.7c refuses `send-keys` while a
  read-only client is attached (`client is read-only`).
- **Keeping tmux sessions LAN-only.** It repeats the route inconsistency this decision removes.

## Verification

- `crates/termirust-controller-listener/tests/tmux_sessions.rs`: authenticated list, attach,
  input, detach, shared hosts, stale generations, ended sessions, and
  `repository_bridge_route_offers_live_panes_and_tmux_sessions`, which serves a published
  desktop pane and a tmux session through `serve_repository_stdio_bridge`.
- `desktop_pane_bridge` unit tests: pointer publication, discovery, removal on stop, and
  rejection of shared, symlinked, and malformed pointers.
- `controller::tests::bridge_sources_follow_the_published_bridge_and_the_sharing_setting` in
  the desktop crate.
