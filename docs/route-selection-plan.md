# Implementation plan: reaching a paired computer by its best route

Status: **proposal.** Written 2026-09-21.

## Goal

A phone reaches its paired computer as fast as the network allows, over the best route
available, and stays on the best route while a session runs, with no setting to change:

- at home, over the local network, even when Tailscale is on, a VPN is on, or the phone uses a
  Tailscale exit node;
- away from home, over Tailscale, then SSH, then the relay, in that order;
- when the network changes mid-session (Wi-Fi to cellular, home to café, back home), moving to the
  new best route without the terminal or the screen being lost.

Nothing here changes who can connect. Every route still ends in the same Noise XX handshake
against the computer's pinned key, and a route counts as working only after that handshake.

## What happens today

- The computer listens on every private address it has (RFC 1918, Tailscale's 100.64/10,
  `fc00::/7`), follows network changes, and announces itself with Bonjour on LAN interfaces only.
- Pairing saves all of those addresses on the phone.
- Connecting (iOS `openFirstRoute`, Android `openFirstReachable`) tries them **one at a time**,
  most recently working first, **up to 10 seconds each**, then asks Bonjour.
- The kind of route (private network, SSH, relay) is **chosen by the person**; the phone never
  moves between kinds on its own.
- A screen session rides the Controller connection, so it inherits whatever route won, for good.

Consequences: a dead first address costs 10 s; home use sticks to Tailscale if that last worked;
a Tailscale exit node without "Allow local network access" silently sends LAN traffic away and
costs a timeout before the 100.x address is tried; nothing ever upgrades a slow route.

## Design

### 1. One route planner, in Rust, shared by both apps

A pure state machine in a new module of `multiplex-controller-bindings` (already UniFFI-exported
to Swift and Kotlin), with no sockets, clocks, or threads of its own:

- **Input:** the candidates (kind, address, port, what is known about each), the phone's current
  network (interface type and a salted fingerprint of the local subnet and gateway, never an SSID,
  so no location permission), and events: *attempt started*, *transport connected*,
  *handshake verified* (with round-trip time), *failed*, *network changed*, *Bonjour saw the host*.
- **Output:** actions: *start attempt N now*, *start attempt N at t+d*, *cancel attempt N*,
  *use attempt N*, *probe candidate N later*, *migrate to attempt N*.

Both apps only execute actions with their native transports (`NWConnection`, Kotlin sockets) and
report events back. The policy lives once, is unit-tested once, and ships as shared JSON test
vectors that the Swift and Kotlin tests replay against their own executors, the pattern the
controller-security golden vectors already use.

### 2. Racing instead of queueing (RFC 8305 "happy eyeballs", adapted)

Candidates are tiered and started staggered, in parallel, and the first to **finish the Noise
handshake** wins; the others are cancelled at once.

| Tier | Candidates | Starts at |
| --- | --- | --- |
| 0 | The route that won last time **on this same network fingerprint** | 0 ms |
| 1 | LAN addresses Bonjour resolved just now; then LAN addresses in the phone's current subnet | 0 ms, then +100 ms apart |
| 2 | Tailscale / VPN addresses (100.64/10, `fd7a:115c:a1e0::/48`, MagicDNS) | +250 ms |
| 3 | Other saved private addresses | +500 ms |
| 4 | SSH route, if configured | +1.5 s |
| 5 | Relay, if configured | +2.5 s |

- **Why the handshake, not the TCP connect.** A `192.168.1.10` on a café network is somebody
  else's device and may accept TCP; only the Noise handshake with the pinned key proves it is this
  computer. Counting TCP success would pick wrong hosts.
- **Per-attempt limits:** LAN connect 1.5 s, VPN 3 s, SSH and relay their own; the whole race gives
  up after 12 s with a precise reason (see 5).
- **Per-network memory:** the winner is remembered per network fingerprint, so the second connect
  from the same place usually completes on its first attempt (tier 0), with no race at all.
- **Cross-kind fallback** follows the person's choice: the kind they picked is always tier 0; SSH
  and the relay join the race only when configured and when "Use other routes when this one is
  unreachable" is on (on by default for newly configured routes, off for existing ones).

### 3. Moving a live session to a better route

The host already replays output from any sequence watermark and holds one writer lease, so a
session can move without a protocol change: **make before break.**

1. While on a tier ≥ 2 route, the phone probes better candidates: immediately on a network change
   or when Bonjour sees the host, and otherwise every 15 s, backing off to 2 min.
2. A probe is a full handshake on the new route. If it verifies and its round-trip time is
   clearly better (≤ 70 % of the current one, or any direct route replacing the relay), the phone
   attaches on the new connection from its current output watermark, moves the writer lease
   (release on the old, acquire on the new), and then closes the old connection.
3. A screen session on the new connection asks for a refresh; the tile cache's existing `Missing`
   recovery resends only what the viewer lacks.
4. If the current route dies (keepalive fails), the race from 2 starts at once and the session
   resumes at the watermark; the person sees "Reconnecting" for as long as the race takes.

Guard rails: at most one migration per 30 s, never while a paste or a writer command is in flight,
and never to a route that failed within the last minute (no flapping).

### 4. The computer keeps the phone's address list current

Today the phone learns addresses at pairing and through Bonjour on the LAN. Add one authenticated
Controller message, **`HostAddresses`**, which the listener sends at connect and whenever its bound
addresses change (it already follows network changes). The phone replaces its saved private-network
candidates with it. A DHCP change or a new Tailscale address then never strands a phone that is
away from home. This is a wire addition, so it gets a short decision record next to
`controller-session-sources.md`; phones that do not know the message ignore it.

### 5. Saying why, when it does not connect

The planner knows what failed and how, so the error is specific:

- LAN failed with a network-unreachable error while the Tailscale address worked and the phone is
  on Wi-Fi in the host's subnet: **"Your Tailscale exit node is sending local traffic away. Turn on
  'Allow local network access' in Tailscale to connect directly."**
- Everything failed on Wi-Fi with no Bonjour answer: "The computer is not on this network, and no
  Tailscale, SSH, or relay route is set up."
- The handshake reached a device with a different key: "Something else answered at this address."

Each race also records, in the bounded local diagnostics store (allowlisted fields only: tier,
kind, milliseconds, outcome code; no addresses), what won and how long it took, so the timings in 2
can be tuned from evidence.

## Work plan

| Phase | Delivers | Where | Estimate |
| --- | --- | --- | --- |
| 1 | Route planner state machine, tiers, per-network memory, handshake-verified success; shared JSON test vectors | `multiplex-controller-bindings` | 2 days |
| 2 | iOS and Android executors replacing `openFirstRoute` / `openFirstReachable`; replay the vectors in XCTest and JUnit with the existing injectable transport factories | `apps/ios`, `apps/android` | 2 days |
| 3 | Cross-kind fallback (SSH, relay) behind the per-route setting | apps | 1–2 days |
| 4 | Make-before-break migration, probing, screen refresh on migrate | apps + planner | 3–4 days |
| 5 | `HostAddresses` message, decision record, listener sender, phone receiver | listener, apps | 2 days |
| 6 | Specific failure messages, diagnostics fields, docs | apps, `docs/remote-terminals.md` | 1 day |

Phases 1–2 alone remove the 10 s stalls and make home use prefer the local network; they ship
first. 4 is the largest and the one that makes screens feel seamless.

## How it is proven

- **Planner:** unit tests for every tier ordering, stagger, cancellation, memory, migration
  threshold, anti-flap rule, and failure message; property tests that exactly one attempt is ever
  used and every other is cancelled.
- **Executors:** the shared vectors replayed on both platforms with fake transports that connect,
  hang, refuse, or present the wrong key on cue.
- **Listener:** `HostAddresses` sent on connect and on a simulated address change.
- **Real devices, by a person, before release:**

  | Phone network | Expected route |
  | --- | --- |
  | Home Wi-Fi, no VPN | LAN, first attempt after the first visit |
  | Home Wi-Fi, Tailscale on | LAN |
  | Home Wi-Fi, Tailscale exit node, LAN access off | Tailscale, with the exit-node hint |
  | Home Wi-Fi, Tailscale exit node, LAN access on | LAN |
  | Home Wi-Fi, corporate full-tunnel VPN | Tailscale or relay |
  | Cellular with Tailscale | Tailscale |
  | Cellular without Tailscale | SSH or relay |
  | Screen share, walk from Wi-Fi to cellular and back | Moves to Tailscale, back to LAN, screen never lost |
  | Computer's DHCP address changes while away | Reconnects after `HostAddresses` |
