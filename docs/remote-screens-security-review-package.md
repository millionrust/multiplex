# Independent review package: Remote Screens ticket bootstrap and endpoint pinning

> **Gate 4:** Independent review note for the ticket bootstrap and endpoint pinning — the same
> D06-style gate the Controller carries. Screen data is more sensitive than terminal text, not less.

This is the brief for whoever takes that review. It exists so the reviewer spends their time on the
protocol rather than on finding where things live, and so the scope is agreed before rather than
argued after.

**It is not a self-assessment, and nothing in it should be read as one.** Where this document says a
property holds, it says how it was checked, so the reviewer can disagree with the check.

## Why screen data raises the stakes

The Controller's existing gate covers a channel carrying terminal text. This one carries a
continuous picture of a person's display, plus the ability to move their pointer and type on their
machine. Three consequences the reviewer should hold onto:

- A screen leaks what text never does: other applications, notifications, anything visible that the
  user forgot was visible.
- `ControlPointer` and `ControlKeyboard` are not "read" capabilities. A flaw that grants them is
  remote code execution by hand.
- Sessions are long and continuous, so anything that degrades over time — nonce reuse, a rekey that
  does not happen, a revocation that is not noticed — has time to matter.

## Scope

**In scope:**

| Area | Where |
|---|---|
| Screen ticket issue, binding and single use | `crates/termirust-controller-listener/src/screen_tickets.rs` |
| Per-frame device and capability checks | `crates/termirust-controller-listener/src/runtime.rs` |
| Capability enforcement at the message level | `crates/termirust-controller-listener/src/screen_session.rs` |
| Session authorisation and grants | `crates/termirust-screen-session/src/host.rs` (`TicketVerifier`, `Grants`) |
| Capability bits and the frame kind | `docs/decisions/controller-security-v1.md`, Amendment 1 |
| Endpoint pinning, when iroh is enabled | `crates/termirust-screen-transport/src/quic.rs` |
| Watch versus control separation | across the above |

**Out of scope, because it belongs to the Controller's own review:** the Noise XX handshake
construction, CPace, SAS derivation, and the transport frame format. Unchanged here — see below for
how that was checked, and please challenge the check rather than taking it.

**Not yet built, so review the intent only:** iroh 0-RTT early data. `IrohTransport::connect` waits
for `handshake_completed()` before returning a usable route, so no application data is sent during
a handshake today. The replay argument for doing so is written in that function's documentation and
is explicitly *asserted, not demonstrated* — it should not be accepted on this pass.

## What changed, and how that claim was checked

The branch adds Remote Screens above the Controller channel. The claim is that it does not alter the
Controller's cryptographic construction. Checks performed:

- `git diff dev..HEAD -- crates/termirust-controller-security/src/` is empty.
- All four golden vectors — offer, handshake, SAS, transport frames — pass byte-for-byte unchanged.
- `scripts/verify/controller-security-vectors.sh --check` passes, including the ADR and lockfile
  checksums.

**What that does *not* establish**, and a reviewer should not let it stand for:

- The capability namespace and the authenticated frame kinds are new, and screen capability bits
  enter the prologue. Identical *construction* is not an identical *transcript*.
- The screen authorisation boundaries above are new security responsibilities regardless of the
  cipher beneath them.
- `zeroize` moved to 1.9.0, which replaced the atomic fence with an architecture-dependent
  `optimization_barrier` including inline assembly. The drop contract is unchanged; the machinery
  is not, and no ciphertext vector can detect erasure failing.
- `russh` moved to 0.63, changing host-key verification and channel-open handling on the SSH route.

## Questions the review should answer

Ranked by what would hurt most if wrong.

1. **Can a device without `ObserveScreens` obtain screen data by any route** — LAN, SSH, relay —
   including through resume, a stale epoch, or a race between revocation and an in-flight session?
2. **Is a screen ticket genuinely single-use and bound to its connection?** What happens if one is
   replayed on a second connection, after a host restart, or after the device is unpaired?
3. **Is watch separable from control in practice, not just in the type system?** Can a viewer
   holding only `ObserveScreens` cause input to be injected by any path, including the pane
   attachment and tmux routes?
4. **Revocation.** When a device is removed, how long can an established screen session keep
   receiving pixels, and is that bounded by anything other than the viewer's good behaviour?
5. **Endpoint pinning.** If iroh is enabled, what binds an authenticated Controller identity to an
   iroh endpoint key, and what stops a different endpoint claiming a session?
6. **Nonce and rekey behaviour over a long session**, including reconnect, resume, and concurrent
   sessions to the same host.
7. **Does anything leak pixels outside the encrypted path** — diagnostics, logs, crash reports,
   thumbnails, the resume store?

## How to run it

```bash
git clone <repo> && git checkout feat/remote-screens
cargo test --workspace --all-targets --no-fail-fast
./scripts/verify/controller-security-vectors.sh --check
cargo deny check
```

Docker-backed SSH and SFTP tests skip themselves without a daemon; set `DOCKER_HOST` and
`MULTIPLEX_DOCKER_FIXTURE_HOST` to run them.

Background reading, shortest path first: [the plan](remote-screens-implementation-plan.md) sections
5 and 8; [the ADR](decisions/controller-security-v1.md) including all three amendments;
[RS3](engineering-evidence/RS3-remote-screens-stage-a.md) for how Stage A rides the Controller
channel.

## What a finished review looks like

A note recording scope, threat model, method, findings with severities, and what was *not* examined.
The last part matters as much as the rest: this gate exists to bound the risk, and an unbounded
"looks fine" bounds nothing.

**This document was written by the implementer.** Treat every claim in it as a hypothesis with a
pointer attached.
