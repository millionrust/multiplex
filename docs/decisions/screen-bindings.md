# ADR: generated Remote Screens bindings

- Status: Accepted for the Stage A phone client
- Decision date: 2026-09-16
- Decision owner: TermiRust engineering
- Release status: Remote Screens is off by default; the phone surfaces are milestone M3

## Decision

Watching and driving a computer's screen crosses into Swift and Kotlin through its own UniFFI
boundary, `termirust-screen-bindings`, beside `termirust-controller-bindings` rather than inside
it. The Controller boundary is the audited cryptographic edge, and
[controller-bindings.md](controller-bindings.md) keeps it deliberately narrow; pixels, tile
caches, and input events have no business widening it.

The two boundaries meet on the phone, not in Rust: the Controller object owns the authenticated
connection and hands the screen object the payload of each screen frame, then sends back the bytes
it produces under the capability each frame must claim.

Everything pinned for the Controller bindings applies unchanged: UniFFI exactly `0.32.0`, Rust
exactly `1.98.1`, the same iOS and Android targets, API 26, 16 KiB ELF LOAD alignment, and JNA for
Kotlin. UniFFI is MPL-2.0; this wrapper stays MIT OR Apache-2.0.

- Swift module: `MultiplexRemoteScreens`; C module: `MultiplexRemoteScreensFFI`
- Kotlin package: `com.multiplex.screens`; library: `multiplex_screen_bindings`

## Boundary contract

`ScreenViewer` is one opaque stateful object per screen session, serialised by a Rust mutex and
safe to call from independent native threads. It performs no I/O, spawns no thread, and holds no
key material: a 32-byte ticket enters as bytes and is proved by the session protocol, never stored
or reused by this crate.

Every fallible export returns the closed `ScreenBindingError`; no Rust error string crosses FFI.
Input the caller cannot mean is refused here rather than at the computer: a ticket that is not 32
bytes, modifier bits outside the four defined ones, an empty or off-surface rectangle, a pane
session id that is not 16 bytes, and empty typed text.

Pixels leave only through `copy_pixels`, one rectangle at a time, so a client redraws what an
update reported as damaged instead of copying a whole 4K framebuffer per frame. The session keeps
the framebuffer; the boundary copies.

Keychain, Keystore, sockets, retries, clocks, and user interface stay on the native side, as they
do for the Controller boundary. The verifier rejects generated sources that mention them.

## Reproducibility and promotion

`scripts/build/mobile-screen-bindings.sh` mirrors its Controller counterpart: fresh isolated
target directories per target, both languages generated with `--no-format`, Android page alignment
checked, public ABI symbols and SHA-256 recorded for every release output, and a staged tree
promoted only when complete. `scripts/verify/mobile-screen-bindings.sh --rebuild-twice` requires
two clean builds to be byte-identical and the native copies to match.
`scripts/sync/mobile-screen-bindings.sh --check` rejects stale copies in the applications.

Any change to UniFFI, Rust, targets, JNA, or an exported type, method, or error requires an ADR
revision, a lockfile review, regenerated artifacts, the crate's tests, and a two-build comparison.
Generated files are never edited by hand.

## What this boundary does not decide

- Screen permission. `ObserveScreens`, `ControlPointer`, and `ControlKeyboard` are Controller-v1
  capability bits (amendment 1 of [controller-security-v1.md](controller-security-v1.md)),
  enforced on the computer per frame. A phone that lies about a frame's capability is refused
  there, not here.
- Transport. Stage A rides Controller screen frames; Stage B may add QUIC. The boundary takes and
  returns bytes either way.
