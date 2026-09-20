# ADR: Controller-v1 pairing and channel security

- Status: Accepted for a Rust-only reference implementation
- Decision date: 2026-08-27
- Decision owner: TermiRust engineering
- Sign-off: Codex implementation owner, Goal 20.1
- Release status: Not approved for remote exposure; D06 and an independent cryptographic review remain mandatory

## Context

TermiRust needs one transport-independent trust contract before desktop, SSH, LAN, Swift, or Kotlin code can control a Host. The current mobile snapshot is not a security baseline: an absent public key and a device identifier do not authenticate a Controller. This ADR covers only an in-memory Rust reference and deliberately creates no listener, persisted Host identity, device database, FFI, account, service, or mobile behavior.

The protocol follows revision 34 of the [Noise Protocol Framework](https://noiseprotocol.org/noise.html), including the fundamental [XX pattern](https://noiseprotocol.org/noise.html#interactive-handshake-patterns-fundamental) and post-handshake channel binding. The specification defines the protocol construction, not the security of a Rust implementation or this application protocol.

## Decision

Controller-v1 uses exactly `Noise_XX_25519_ChaChaPoly_BLAKE2s`. The device/Controller is always the Noise initiator and the Host is always the responder. There is no role negotiation and no protocol or cipher downgrade. Controller-v1 accepts only version `1.0`; every other major or minor is incompatible before mutation.

The implementation is `clatter = 2.2.0`, pinned exactly in the crate manifest and `Cargo.lock`, with default features disabled and only `alloc`, `use-25519`, `use-chacha20poly1305`, and `use-blake2` enabled. Application SAS derivation additionally pins `hkdf 0.12.4`, `sha2 0.10.9`, `subtle 2.6.1`, and `zeroize 1.9.0` (1.8.2 until 2026-09-18; see the amendment below).

## Dependency review

`clatter 2.2.0` was selected over `snow 0.10.0` because the latter does not zeroize internal symmetric and transport state on drop. `clatter` stores private keys, DH outputs, symmetric state, and cipher keys in zeroize-on-drop containers. Its X25519, ChaCha20-Poly1305, and BLAKE2s implementations are pure Rust through RustCrypto dependencies; the selected feature set contains no native dependency and does not enable its PQ, AES-GCM, SHA, or system-RNG features. The crate declares Rust 1.81 and MIT licensing, which are compatible with this workspace. Source inspection found no `unsafe` block in `clatter 2.2.0`; transitive RustCrypto crates remain governed by the lockfile and repository dependency policy.

`clatter` is maintained and its 2.2.0 API supports externally supplied static and ephemeral keys, handshake hashes, remote static-key inspection, and split cipher states. It is also a low-level implementation, is not independently audited, and has documented panic surfaces for oversize Noise messages and invalid application-created patterns. TermiRust uses only the library-provided `noise_xx()` pattern, supplies exact-size key types, and checks every external length before calling those surfaces. The crate's internal fallback RNG is unreachable because both static and ephemeral keys are required from the caller; no production entropy source is selected in this goal.

`cargo deny check` is the advisory, license, and source gate for the locked graph. Passing it does not constitute a cryptographic audit. Any dependency version or selected feature change requires a new ADR checksum and complete vector regeneration/review.

### 2026-08-30 workspace lock review

Goal 15.1 added the isolated `termirust-tui` workspace crate and its pinned Ratatui/Crossterm
presentation graph. `cargo tree -p termirust-controller-security` confirms that none of those
packages enter the controller-security dependency closure; the pinned cryptographic package
versions, selected features, protocol implementation, and every offer, handshake, SAS, key, and
transport-frame vector remain byte-for-byte unchanged. The prior fixture correctly failed only its
workspace `Cargo.lock` checksum after the addition. The complete vectors were reviewed again, and
`cargo deny check` plus the dedicated vector verifier pass for the new locked graph. This note and
the fixture checksums record that dependency review without claiming a new cryptographic audit.

Goal 15.2 connected that same TUI package to the already-locked `termirust-client`, Host protocol,
session-host test fixture, Tokio, `rand 0.8.6`, and `vt100 0.16.2` packages. The lockfile change only
adds those existing package names to `termirust-tui`'s dependency list; it adds no package, changes
no selected version or feature, and does not alter the controller-security dependency closure or
any protocol vector field. The lock checksum was reviewed again under the same controls.

Goal 15.3 connected `termirust-tui` to the already-locked `termirust-cli` package so reviewed TUI
management intents use the same typed local lifecycle facade. The lockfile change adds only that
existing workspace package name to the TUI dependency list. It adds no external package, changes no
selected version or feature, and leaves the controller-security dependency closure and all protocol
vector bytes unchanged. The lock checksum and complete golden-vector suite were reviewed again.

Goal 15.5 connected `termirust-tui` directly to the already-locked `uuid` package for canonical
Controller device-ID parsing and to the already-locked `serde_json` package in tests for a hostile
newer-store fixture. The lockfile change adds only those existing package names to the TUI
dependency list. It adds no package, changes no selected version or feature, and leaves the
controller-security dependency closure and all protocol vector bytes unchanged. The lock checksum
and complete golden-vector suite were reviewed again.

E12.6 connected `termirust-store` to the existing `termirust-replication-security` workspace crate
and added the already-locked `zeroize 1.8.2` package to store contract tests. No package, version, or
feature in the controller-security dependency closure changed, and no Controller offer, handshake,
SAS, key, or frame vector changed. The global lock checksum and this ADR checksum were reviewed and
repinned because the golden fixture deliberately detects any workspace dependency-graph change.

N05-N11 added native mobile-terminal adapters, self-hosted relay packaging, and the isolated
`termirust-mcp` workspace crate. The resulting lockfile entries use packages already reviewed in the
workspace and add no package, version, or selected feature to the controller-security dependency
closure. `cargo tree -p termirust-controller-security` and the complete golden-vector suite confirm
that every Controller offer, handshake, SAS, key, and frame vector remains byte-for-byte unchanged.
The lock checksum and this ADR checksum were reviewed and repinned for the expanded workspace graph.

N12 connected `termirust-mcp` to the already-locked `fs2 0.4.3` and `sha2 0.10.9` packages for
owner-local receipt locking and one-way command fingerprints. Neither package, version, nor selected
feature changes the controller-security dependency closure, and all Controller protocol vector
bytes remain unchanged. The workspace lock and ADR checksums were reviewed and repinned again.

N14 connected `termirust-store` directly to the already-locked `fs2 0.4.3` and `same-file 1.0.6`
packages so revisioned metadata stores use real Windows advisory locks and recovery-file identity.
Neither package, version, nor selected feature enters the controller-security dependency closure.
The complete Controller offer, handshake, SAS, key, and frame vectors remain byte-for-byte
unchanged; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 `termirust-controller-listener` added `mdns-sd 0.21.3` (default features off) and its
new dependency `socket-pktinfo 0.4.0` to announce the listener with Bonjour on LAN interfaces.
Neither enters the controller-security dependency closure, and every Controller vector is
unchanged; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 the desktop app added `alacritty_terminal 0.26.0` (default features off) for terminal
emulation, with its new dependencies `cursor-icon 1.2.0`, `miow 0.6.1`, `rustix-openpty 0.2.0`, and
`signal-hook 0.4.4`. None enters the controller-security dependency closure, and every Controller
vector is unchanged; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 the new `termirust-screen-codec` workspace crate added `xxhash-rust 0.8.18` (default
features off, `xxh3` only) for tile hashing, and uses the already-locked `miniz_oxide 0.8.9` and
`proptest 1.11.0`. Neither enters the controller-security dependency closure, and every Controller
vector is unchanged; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 the new `termirust-screen-capture` workspace crate added `screencapturekit 10.0.3`
(macOS only, default features off) with its new dependencies `apple-cf 0.10.0`, `apple-metal 0.9.0`,
and `doom-fish-utils 0.4.0`, all MIT OR Apache-2.0. The older `screencapturekit 0.2.8` that GPUI
pulls in is unchanged. None enters the controller-security dependency closure, and every Controller
vector is unchanged; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 the new `termirust-screen-protocol` workspace crate was added. It adds no external
package, uses only the already-locked `termirust-screen-codec` and `proptest 1.11.0`, stays outside
the controller-security dependency closure, and changes no Controller vector; the workspace lock
and ADR checksums were reviewed and repinned.

On 2026-09-15 the new `termirust-screen-session` workspace crate was added. It adds no external
package, depends only on the screen codec and protocol crates, stays outside the controller-security
dependency closure, and changes no Controller vector; the workspace lock and ADR checksums were
reviewed and repinned.

On 2026-09-16 `termirust-screen-bindings` took the already-locked `hex 0.4.3`, `serde_json 1.0.149`
and `sha2 0.10.9` as dev-dependencies, for the recorded session its tests and the Swift
conformance runner replay. They are test-only, add no package, and stay outside the
controller-security dependency closure; every Controller vector is unchanged, and the workspace
lock and ADR checksums were reviewed and repinned.

On 2026-09-16 the mobile Controller fixture was taught to serve a synthetic screen, so a phone can
be tested against a real host. That adds `termirust-screen-codec`, `-host`, `-protocol` and
`-session` to `termirust-controller-listener`'s **dev**-dependencies only. It adds no external
package, changes no selected version or feature, and nothing reaches a published artifact or the
controller-security dependency closure; every Controller vector is unchanged, and the workspace
lock and ADR checksums were reviewed and repinned.

On 2026-09-16 the new `termirust-screen-bindings` workspace crate was added so phones can watch
and drive a screen. It mirrors `termirust-controller-bindings`: the same pinned `uniffi 0.32.0`,
no other external package, and the screen workspace crates. It never touches Controller-v1 keys or
frames, so the controller-security dependency closure is unchanged and every Controller vector
still matches; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-16 the desktop app was connected to the screen crates it needs to share this
computer's displays: `termirust-screen-capture`, `-codec`, `-host`, `-input`, `-protocol`, and
`-session`. The lockfile change adds only those existing workspace package names to the app's
dependency list; it adds no external package and changes no selected version or feature. None
enters the controller-security dependency closure, and every Controller vector is unchanged; the
workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-16 the new `termirust-screen-host` workspace crate was added to serve screens over the
Controller channel. It adds no external package: it uses the already-locked `async-trait 0.1`,
`tokio`, and `tokio-util`, plus the screen and Controller workspace crates. It depends on
`termirust-controller-listener`, not on the controller-security crate's internals, so the
controller-security dependency closure is unchanged and every Controller vector still matches; the
workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-15 the new `termirust-screen-input` workspace crate was added. It adds no external
package: on macOS it uses the already-locked `core-graphics 0.24.0` (MIT OR Apache-2.0) with the
dependency-free `highsierra` feature for scroll events. It stays outside the controller-security
dependency closure and changes no Controller vector; the workspace lock and ADR checksums were
reviewed and repinned.

On 2026-09-16 the new `termirust-screen-video` workspace crate was added to encode the Remote
Screens motion region with hardware HEVC. It adds **no package at all**: it declares no
dependencies, and reaches VideoToolbox, Core Media, Core Video and Core Foundation through
hand-declared `extern "C"` items and framework link flags in its build script, exactly as the
spike in `tools/videotoolbox-spike` did. That is a deliberate choice over `objc2-video-toolbox`,
which would have brought an objc runtime and a large lockfile change for about two dozen symbols.
The crate is the one place in this path that contains `unsafe`; `termirust-screen-host`, which
consumes it behind a trait, keeps `#![forbid(unsafe_code)]`. It carries no key material, speaks no
protocol, and stays outside the controller-security dependency closure; every Controller vector is
unchanged, and the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-16 `termirust-screen-session` gained a dependency on `termirust-screen-video`, so a
viewer decodes the motion region itself and draws it into the same framebuffer the tiles go into.
The lockfile change adds only that existing workspace package name to the session crate's
dependency list; it adds no external package and changes no selected version or feature. The
session crate keeps `#![forbid(unsafe_code)]` — the unsafe stays inside the video crate — and the
video crate carries no key material and speaks no protocol, so the controller-security dependency
closure is unchanged and every Controller vector still matches; the workspace lock and ADR
checksums were reviewed and repinned.

On 2026-09-17 the new `termirust-screen-transport` workspace crate was added: the seam a QUIC
transport will fit into, holding which delivery each class of screen message needs. It adds **no
external package** — it depends only on `termirust-screen-protocol` — and keeps
`#![forbid(unsafe_code)]`. It carries no key material and performs no I/O of its own; the one
implementation today gathers bytes for the Controller channel to send, exactly as the host did
inline before. The controller-security dependency closure is unchanged and every Controller
vector still matches; the workspace lock and ADR checksums were reviewed and repinned.

On 2026-09-17 `termirust-screen-capture` gained a macOS dependency on `core-graphics 0.24.0`, so
a process can ask whether it holds Screen Recording before trying to capture. That package is
already in the workspace lock through `termirust-screen-input`, at the same version, and the
lockfile change adds only the edge. The call used is `CGPreflightScreenCaptureAccess`, which asks
and never prompts; it carries no key material and stays outside the controller-security
dependency closure, so every Controller vector is unchanged. The workspace lock and ADR checksums
were reviewed and repinned.

On 2026-09-17 `termirust-screen-input` gained a Linux dependency on `libc 0.2`, for the `ioctl`
and `write` calls that drive a `uinput` virtual device. That package is already in the workspace
lock at the same version through Tokio and others, so the lockfile change adds only the edge. The
calls used are `ioctl` and `write` on a file descriptor the crate opened itself; the module
carries `allow(unsafe_code)` while the rest of the crate keeps `deny`. It holds no key material
and stays outside the controller-security dependency closure, so every Controller vector is
unchanged. The workspace lock and ADR checksums were reviewed and repinned.

## Key and offer lifecycle

- A Host static X25519 private key will be generated by a platform CSPRNG and stored by a later goal in `SecretStore`. This goal accepts caller-provided key bytes only and persists nothing.
- A device static X25519 private key will be generated and held by the native platform secure store in a later goal.
- Every handshake requires fresh CSPRNG-generated Host and device ephemeral private keys. Test keys are conspicuously deterministic fixtures and must never be used in production.
- A pairing nonce is 32 random bytes, single-use, and valid for at most 300 seconds. The in-memory machine cannot be reused. Atomic nonce consumption and uncertain-final-ACK recovery belong to Goal 20.2.
- Host key loss invalidates all paired devices. Backup/restore never copies private pairing identity under this contract.

## Canonical offer and prologue

All integers are unsigned big-endian. Reserved fields must be zero. No trailing bytes are accepted.

`PairingOfferCore` is exactly 84 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `TCO1` |
| 4 | 2 | major, exactly `1` |
| 6 | 2 | minor, exactly `0` |
| 8 | 1 | suite, exactly `1` for the selected Noise protocol |
| 9 | 1 | reserved zero |
| 10 | 8 | expiry as Unix seconds |
| 18 | 32 | pairing nonce |
| 50 | 32 | Host static public key |
| 82 | 2 | requested capability bits |

The Noise prologue is `ASCII("multiplex-controller-v2\0") || u16be(84) || offer_bytes`. This binds version, suite, expiry, nonce, Host identity, and requested capability template to the Noise transcript without making the QR/offer secret.

## XX messages and transcript checks

Every Noise message carries one fixed 110-byte payload. The payload is plaintext in message 1 as required by XX and encrypted/authenticated in messages 2 and 3. It is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `TPS1` |
| 4 | 1 | step: device hello `1`, Host proof `2`, device proof `3` |
| 5 | 1 | role: device initiator `1`, Host responder `2` |
| 6 | 2 | reserved zero |
| 8 | 2 | major `1` |
| 10 | 2 | minor `0` |
| 12 | 32 | pairing nonce |
| 44 | 32 | Host static public key |
| 76 | 32 | device static public key |
| 108 | 2 | exact capability bits |

The sequence is `device -> Host: e + DeviceHello`, `Host -> device: e, ee, s, es + HostProof`, and `device -> Host: s, se + DeviceProof`. Each receiver checks exact step, fixed role, version, nonce, both ordered public keys, and capabilities. After message 2 the initiator compares Noise's authenticated responder static key to the offer. After message 3 the responder compares Noise's authenticated initiator static key to the device key declared in message 1. Any mismatch destroys live state.

After both roles process message 3, they take the final BLAKE2s Noise handshake hash `h` as channel binding. Only this final value may enter SAS-v1 or transport confirmation.

## SAS-v1

SAS-v1 is byte-for-byte fixed:

1. `salt = SHA-256(ASCII("multiplex-controller-sas-v2\0") || pairing_nonce[32])`.
2. `info = ASCII("sas\0") || u16be(major) || u16be(minor) || host_static_public_key[32] || device_static_public_key[32]`.
3. Run HKDF-SHA256 with `IKM = h`, that salt and info, and output exactly five bytes.
4. Read the 40 bits MSB-first as eight five-bit indices into `0123456789ABCDEFGHJKMNPQRSTVWXYZ`.
5. Display uppercase `XXXX-XXXX`. There is no checksum, modulo, discarded range, or localized alphabet.

The comparison space is exactly 40 bits. SAS is comparison-only and is never entered as authentication. Accessible speech identifies each visible symbol as `letter` or `digit`. `Debug` output is redacted. The normative independent anchor is committed in `tests/vectors/controller-v2.json` and yields `5FSW-YX9D`.

## Amendment 2026-09-15: six-digit code pairing

Phones may pair by typing a six-digit code the desktop shows in pairing mode, instead of scanning an offer and comparing a SAS. The XX handshake, payloads, SAS-v1, IK connections, and transport framing above are unchanged; code pairing adds a key exchange before XX and a binding in its prologue.

### Why a PAKE

A six-digit code carries about 20 bits. Using it directly as a Noise pre-shared key or prologue secret would let an active attacker who completes one handshake test all 10^6 codes offline against the recorded AEAD tags. The code is therefore used only as the password of a balanced PAKE, CPace ([draft-irtf-cfrg-cpace-14](https://datatracker.ietf.org/doc/draft-irtf-cfrg-cpace/)), which gives an active attacker one online guess per attempt and a passive observer nothing.

### Construction

- Group and hash: CPace over Ristretto255 with SHA-512, `DSI = "CPaceRistretto255"`, implemented in `termirust-controller-security::cpace` with `curve25519-dalek 4.1.3` (default features off, `zeroize` on) and the already pinned `sha2 0.10.9`. The implementation reproduces the draft's appendix B.3 generator, share, secret, ISK, and invalid-point vectors.
- Code: exactly six ASCII digits, drawn uniformly with rejection sampling from the OS CSPRNG. `PRS` is the six ASCII bytes.
- Offer: the 84-byte `PairingOfferCore` above, delivered by the Host in plaintext at the start of the connection. It is not secret.
- `CI = "multiplex-controller-code-v2" || offer_bytes`; `sid = device_nonce[32] || offer_nonce[32]`, where the device nonce is fresh per attempt.
- Parties: the device is CPace party A with `ADa = "multiplex-controller-device"`, the Host party B with `ADb = offer_bytes`. Shares that do not decode or multiply to the neutral element abort.
- `ISK` is the draft's initiator-responder ISK. `binding = SHA-512("multiplex-controller-code-binding-v2\0" || ISK)[..32]`.
- The XX prologue becomes `ASCII("multiplex-controller-v2\0") || u16be(84) || offer_bytes || ASCII("multiplex-controller-code-v2\0") || binding`. A peer that used another code derives another binding, so Host proof decryption fails at message 2 and neither static key is accepted.
- After message 3, `confirm_code_authenticated` finalizes only a code-bound machine. There is no SAS comparison; an unbound machine cannot use this path.

### Wire sequence

After the `TRCN` preface with purpose `3` (pair with code): device sends `CodePairingHello{device_nonce}`; Host sends the offer envelope; device sends its 32-byte share; Host sends its 32-byte share; then the three XX messages, the sealed registration, and the sealed acknowledgement exactly as for offer pairing.

### Attempt limits and lifecycle

- Pairing mode is opened only from the desktop app. The code lives in the listener process memory, is shown on the desktop, and is never persisted, logged, or sent on the network.
- Each code allows three attempts and one attempt at a time. An attempt is spent before the Host sends its share, so wrong codes, dropped connections, and successes all count. After the third failure the offer is rejected and the desktop must open pairing mode again.
- The code expires with its offer after at most 300 seconds. Opening pairing mode again rejects the previous code. Per-source failed-authentication limits apply as for other pairing.
- The success probability of guessing within one code is at most 3 / 10^6, and each new code requires the user to open pairing mode on the desktop.

### Dependency and checksum change

`curve25519-dalek 4.1.3` was already in the workspace lock through `x25519-dalek`; this amendment adds it as a direct dependency of `termirust-controller-security`. The Controller offer, XX, SAS, key, and frame vectors in `controller-v1.json` are unchanged; the ADR and lockfile checksums were repinned. Code pairing vectors are in `tests/vectors/controller-code-v1.json`.

## Pairing state and failure model

The only states are `Created -> Handshaking -> SasReady -> Confirmed | Rejected | Expired | Failed`. The handshake deadline is 30,000 milliseconds from construction, using a caller-supplied clock value. An offer already expired or more than 300 seconds in the future is rejected. Duplicate, reordered, malformed, oversized, role-confused, key-confused, nonce-confused, capability-confused, or unauthenticated input fails closed. There is no automatic retry or downgrade.

SAS confirmation consumes the in-memory Noise state and returns a transport plus authenticated public metadata. It does not persist or authorize a device record. SAS mismatch, rejection, timeout, cancellation, or any parsing/crypto error drops and zeroizes the live private, handshake, SAS, and cipher state. Errors carry stable codes/localization IDs and no peer bytes or secret values.

## Capabilities and authorization

Capabilities are a closed bit set:

| Bit | Capability |
|---:|---|
| 0 | `ObserveSessions` |
| 1 | `AttachOutput` |
| 2 | `SendInput` |
| 3 | `Resize` |
| 4 | `RespondToApproval` |
| 5 | `ObserveScreens` |
| 6 | `ControlPointer` |
| 7 | `ControlKeyboard` |

Bits 5 to 7 were added by the Remote Screens amendment below. `ObserveScreens` allows watching a
screen; the two control bits are separate from it and from each other, and separate from
`SendInput`, which stays a terminal capability. Unknown bits fail. Every opened or sealed frame must match both a granted capability and the exact current revocation epoch. A stale or future epoch is denied. Later Host code must validate the same policy again at the command boundary; this crate is not sole authorization merely because decryption succeeded.

## Transport framing

The Noise split produces two directional ChaChaPoly cipher states. Controller-v1 uses those selected-implementation cipher states directly so terminal ciphertext frames can exceed Noise's 65,535-byte handshake/message convenience limit while retaining the standard Noise nonce and rekey primitive. No application-authored DH, AEAD, hash, or key schedule exists.

The authenticated 32-byte frame header is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `TCF1` |
| 4 | 2 | major `1` |
| 6 | 2 | minor `0` |
| 8 | 1 | kind: control `1`, terminal `2`, screen `3` |
| 9 | 1 | one closed capability value |
| 10 | 2 | reserved zero |
| 12 | 8 | revocation epoch |
| 20 | 8 | sequence / Noise nonce |
| 28 | 4 | ciphertext byte length including 16-byte tag |

The complete header is ChaCha20-Poly1305 associated data. Sequence starts at zero, must equal the receiver's next sequence, and may not exceed `2^64 - 3`. A lower value is duplicate; a higher value is out of order. Directional cipher state rekeys immediately before each sequence divisible by `2^20` except zero. Rekey failure closes the channel. Frames are never retried under the same nonce.

Control plaintext is at most 65,536 bytes. A complete terminal frame, including the 32-byte header and 16-byte tag, is at most 1,048,576 bytes; a complete screen frame has the same limit, which is the tile codec's largest batch. Length is validated with checked arithmetic before allocation or crypto. Ciphertext, plaintext payload, and transport `Debug` are redacted.

## Compatibility

Controller-v1 has exact-version compatibility only. Unknown major or minor versions return `incompatible_version` before interpreting mutable fields. Unknown capabilities, kinds, suites, flags, or nonzero reserved bytes fail closed. A future compatible minor requires an ADR amendment and new immutable vectors; silent downgrade and best-effort parsing are forbidden.

## Amendment 1: Remote Screens capabilities and frame kind (2026-09-16)

Remote Screens lets a paired device watch a computer's screen and drive its pointer and keyboard.
That permission belongs inside this channel, not in application settings, so it is enforced on the
LAN, SSH, and relay routes alike and is revoked by the existing epoch.

The amendment adds exactly three capability bits — `ObserveScreens` (5), `ControlPointer` (6), and
`ControlKeyboard` (7), widening the closed mask from `0x001f` to `0x00ff` — and one frame kind,
screen `3`, whose complete frame is at most 1,048,576 bytes, the tile codec's largest batch.
Watching and driving stay separate permissions, and both stay separate from `SendInput`, so a
device may watch without being able to touch anything.

The version stays `1.0`. No field, offset, size, or previously defined value changes; the new
values appear only where a Host offers them. An implementation that predates this amendment meets
a new capability or kind as an unknown value and fails closed with `unknown_capability` or
`invalid_encoding`, which is this ADR's specified behaviour rather than an exception to it.
Discovery is the offer's capability template, which a Host that cannot serve screens simply never
sets, so nothing negotiates or downgrades.

Every offer, handshake, SAS, key, and frame vector published before this amendment is unchanged
byte for byte; the fixture's primary offer still requests bits 0 to 2 only. New immutable vectors
pin an offer and prologue that request the screen bits, the first screen frame on a confirmed
transport, and the rejection of the first unused capability bit and frame kind.

Acceptance of this amendment is the release gate "Capability ADR amendment accepted"; the
implementation ships behind it.

### Lockfile note: Windows and Linux screen capture (2026-09-18)

The M6 capture backends changed the workspace `Cargo.lock`, so the checksum below was repinned.
Nothing in `termirust-controller-security` changed, and no vector byte changed; only the pinned
lockfile hash did.

Windows added no package: `windows 0.61.3` was already in the lock file, and the capture crate now
names it. Linux added `pipewire 0.8.0` and with it `libspa`, `pipewire-sys`, `libspa-sys`,
`cookie-factory`, and `nix` (all MIT), plus the build-time `system-deps` (MIT/Apache) and
`bindgen` (BSD-3-Clause). `ashpd 0.13.13` and `async-io 2` were already locked and gained a
`screencast` feature. All are permissive and none is reachable from this crate: every addition sits
behind `cfg(target_os)` in `termirust-screen-capture`, which does not depend on
`termirust-controller-security`. `pipewire-sys` links the system `libpipewire-0.3` rather than
vendoring it, so a Linux build needs `libpipewire-0.3-dev` present.

`windows-capture` was considered and rejected: it wraps Windows.Graphics.Capture, which delivers a
whole texture per frame and reports no damage, and Desktop Duplication's dirty and move rectangles
are what the tile codec is built around.

`cargo deny check` is green on advisories, bans, licences, and sources after the change.

### Amendment: zeroize 1.8.2 to 1.9.0, and russh 0.57 to 0.63 (2026-09-18)

Remote Screens Stage B carries pictures over QUIC, and the transport chosen for it in section 5.3
of the plan is iroh. iroh 1.2 could not be resolved alongside this workspace as it stood, for two
separate reasons, and one of them is named in this ADR — so this amendment is what the change
control section below asks for.

**The zeroize pin.** `iroh-base` requires `zeroize ^1.9` against the exact `=1.8.2` pinned here and
in seven other manifests. It moved to `=1.9.0`, still pinned exactly.

The *contract* this ADR relies on is unchanged: a `Zeroizing` value is still overwritten on drop.
The **machinery enforcing it is not**. 1.9.0 replaces the atomic fence with an `optimization_barrier`
in a new architecture-dependent implementation that includes inline assembly, and also changes some
type layouts. "A minor release from the same authors" is not an argument that erasure still happens
on every target, and the golden vectors cannot speak to it either — they pin ciphertext, and
erasure leaves no trace in ciphertext. What can be said honestly is that the API contract is the
same and that this is the version iroh requires; whether the new barrier erases as reliably as the
old fence on every platform this ships to is a question for the independent review, and is listed
as such below.

**The russh pin, which took two attempts and is the part worth reading.** russh through 0.59 pins
`rand_core = "=0.10.0-rc-3"` exactly, against the released `^0.10` that `ed25519-dalek 3.0.0`
requires. Moving to 0.60 cleared that and looked sufficient: the workspace compiled and the whole
Docker-backed SSH and SFTP suite passed.

It was not sufficient, and the way it failed is the lesson. Adding iroh to the lockfile made the
desktop crate stop compiling, inside a dependency neither of them names: `rsa 0.10.0-rc.16`, which
russh 0.60.1 carries. iroh's `ed25519 3.0.0` needs the *released* `pkcs8 0.11.0`; that rsa release
candidate was written against `pkcs8 0.11.0-rc.11`, in which `Error::KeyMalformed` is a unit
variant rather than one with fields. Cargo unifies both to a single `0.11.x` and rsa loses. The
same collision sits behind russh 0.60's elliptic-curve stack, where `p256/p384/p521 0.14.0-rc.7`
pin `primefield 0.14.0-rc.7` against a `crypto-bigint` that the newer rsa cannot use.

In other words: **iroh needs the released RustCrypto crates and russh 0.60 is still on their release
candidates.** This is a *source* incompatibility, not an unsatisfiable version constraint — rsa
rc.16 asks for `pkcs8 0.11.0-rc.10`, which permits the release, so resolution succeeds and
compilation is what fails. Cargo allows a prerelease requirement to advance to a compatible
released version, which is why the break appears late and somewhere neither crate names.

russh 0.63 moved to the released stack — `primefield 0.14.0`, `pkcs8 0.11.0`, `rsa 0.10.0-rc.18` —
and is the upstream answer rather than the only conceivable one. Two alternatives exist and were
not taken: a `[patch]` against a forked rsa, which carries a maintenance cost on a cryptographic
dependency and is the last thing this workspace should own; and dropping russh's optional `rsa`
feature, which removes this particular compilation path at the cost of RSA key support and does
not on its own settle the elliptic-curve side, whose requirements are ranges rather than exact
pins. Upgrading is the choice that keeps every key type working and leaves the crypto to upstream.

Worth recording because it will recur: this workspace now depends on two independent projects
tracking the same pre-release ecosystem, and they will fall out of step again.

**What changed and what did not.** No code in `termirust-controller-security` changed, no handshake
or SAS derivation changed, and **no vector byte changed** — all four golden vectors pass untouched.
Only the pinned lockfile checksum and this document's own moved.

**What russh 0.63 changed at the call sites**, all in the desktop crate:

- *SSH-agent identities* are an `AgentIdentity` enum rather than a bare public key, so a
  certificate held by an agent is offered through `authenticate_certificate_with` and a plain key
  through `authenticate_publickey_with`. The split is an improvement: a certificate carries
  principals and validity that a public key does not, and the server needs them.
- *Key generation* takes an RNG through rand_core 0.10's traits, which `rand 0.8`'s `OsRng` does
  not implement, so three call sites use `rand` 0.10 under an alias. Both are the operating
  system's source; only the trait shape differs.
- *Host key verification* — `check_server_key` — now receives a `PublicKeyOrCertificate` rather
  than a `PublicKey`. **This is a signature change, not a behaviour change, because this app does
  not negotiate host certificates.** `Preferred::DEFAULT` sets `host_key_certificates` to an empty
  list (`russh-0.63.1/src/negotiation.rs:212`), certificate algorithms are opt-in, and every SSH
  and SFTP path here builds `client::Config::default()` and overrides only the keepalive. Nothing
  in `crates/` sets `host_key_certificates`. A server therefore cannot present a certificate to
  this client, and the `Certificate` arm is unreachable in the shipped configuration.

  Both call sites nevertheless take `public_key()` and pin that, so the arm is correct if the
  preference list is ever populated. For a certificate that accessor returns the **subject** key —
  the server's own key, which the CA signed — and not the CA's key, which is a separate
  `signature_key()` accessor (`ssh-key`'s `certificate.rs`). Two hosts signed by one CA therefore
  do not collide on one pinned entry, which is the mistake that would have made this a real
  downgrade.

  **It is deliberately not certificate validation**: no CA trust store, no principal match, no
  validity window, no critical options. Were the preference list ever populated, this code would
  accept an expired or wrong-principal certificate whose subject key satisfies trust on first use
  and whose holder proves possession — no weaker than the bare-key TOFU already offered, but not
  what the word "certificate" implies to a reader. Leaving the list empty is the safer policy and
  the code already does that. Host-certificate validation, if it is ever wanted, is its own feature
  with its own decision record.

  *(An earlier draft of this amendment stated that 0.63 advertises the certificate algorithms and
  that a server may answer with one. That was wrong, and it is recorded here rather than quietly
  edited because the error ran in the direction that overstates what the code does.)*
- *Channel-open callbacks* for reverse forwards and agent forwarding are handed a handle that must
  be accepted, and which rejects with `AdministrativelyProhibited` when dropped. This is a trap
  worth naming: the parameter can be ignored with an underscore, the code compiles, and every
  forwarded connection is then silently refused at runtime. Both sites accept explicitly. It also
  improves the refusal path — an unapproved agent-forward request or an unrecognised forwarded
  connection now costs that channel alone, where before it failed the handler and took the whole
  SSH session with it.

**iroh is now in the lockfile**, as an optional dependency of `termirust-screen-transport` behind
an `iroh` feature that is off by default. A default build of this workspace — which is every build
that ships today — resolves no QUIC stack, no relay client and no Tokio runtime from it, and
`cargo tree -p termirust-screen-transport` shows only the screen protocol beneath it. It is named
here because the lockfile checksum this ADR pins is what would otherwise let a dependency of that
size arrive without anyone saying so.

**What this does not settle.** Adopting iroh is still gated on the 0.4 device spike. This
amendment makes it *possible* to adopt, and the owner directed it on 2026-09-18 ahead of that
spike; if 0.4 says no, the feature stays off, these pins stay where they are, and nothing has to
be undone.

**Nor does it settle the release gate.** The independent cryptographic review named at the top of
this document is still outstanding, and this amendment does not touch it.

What can be claimed is narrower than "nothing changed", and the distinction matters. The handshake
*construction* and the SAS derivation are unchanged, and every golden vector is byte-for-byte
unchanged. That is not the same as saying the reviewable surface has not moved:

- Remote Screens adds capability bits and frame kinds, so a screen-enabled offer puts different
  capability bytes in the prologue. The construction is identical; the transcripts are not.
- The screen work adds authorization boundaries that are security responsibilities in their own
  right — per-frame device and capability checks, connection-scoped one-use screen tickets, and
  the watch-versus-control separation — even though the cipher construction beneath them is
  untouched.
- The zeroize erasure machinery changed, as recorded above, and no vector can detect that.
- The optional iroh transport, when it is ever enabled, brings identity binding, resumption and
  replay questions that this ADR has never covered. Its `connect` waits for
  `handshake_completed()` before returning a route, so no application data is sent early today;
  the replay reasoning in that function's documentation has to be demonstrated, not asserted,
  before the 0-RTT typestate is taken.

So this amendment supplies evidence for a review, not a substitute for one, and it does not shorten
the reviewer's list.

**Accepted by the decision owner on 2026-09-18.**

### Lockfile note: Windows local-pane job object (2026-09-19)

The desktop crate now names `windows-sys 0.61.2` on Windows, to put each local pane's shell and
console host into a job object that ends them when the app exits. That changed the workspace
`Cargo.lock` by one dependency edge, so the checksum below was repinned. Nothing in
`termirust-controller-security` changed, and no vector byte changed; only the pinned lockfile hash
did.

No package was added: `windows-sys 0.61.2` was already in the lock file. The edge sits behind
`cfg(windows)` in `termirust-desktop`, which this crate does not depend on.

`cargo deny check` is green on advisories, bans, licences, and sources after the change.

### Lockfile note: the crates renamed to multiplex (2026-09-20)

Every crate was renamed from `termirust-*` to `multiplex-*`, and the desktop package from
`termirust` to `multiplex`, so the workspace `Cargo.lock` names them differently and the
checksum below was repinned. No dependency was added, removed or moved: the change is the
names of this workspace's own packages and the paths between them. Nothing in this crate
changed beyond its own name, and no vector byte changed.

`cargo deny check` is green on advisories, bans, licences, and sources after the change.

## Amendment 2: Controller-v2, the Multiplex identity (2026-09-21)

The product is now called Multiplex, and every domain separator in this construction carried the
old name. A separator is an input to key derivation, not a label: `multiplex-controller-v2\0` is
in the Noise prologue, `multiplex-controller-sas-v2\0` in the SAS salt, and
`multiplex-controller-code-v2` and `-code-binding-v1\0` in CPace. Renaming them changes the keys,
the transcript and the SAS for the same inputs.

### What this amendment changes

- Every separator moves to the Multiplex name and the new version:
  `multiplex-controller-v2\0`, `multiplex-controller-sas-v2\0`,
  `multiplex-controller-connection-v2\0`, `multiplex-controller-code-v2`,
  `multiplex-controller-code-binding-v2\0`, and `multiplex-controller-device`.
- The protocol version becomes exactly `2.0`.
- The four-byte magics do not move. `TCO1`, `TPS1`, `TCP1`, `TCC1` and `TCF1` name the byte
  layout, not the protocol, and the layout is unchanged. Moving them was tried and reverted: a
  Controller-v1 peer reading a `TCO2` offer stops at the magic and reports a malformed encoding,
  never reaching the version field, which is the opposite of what this amendment is for. Holding
  the magic still is what lets both sides answer `incompatible_version`.
- Nothing else about the construction moves. The Noise pattern, its pinned implementation, the
  CPace group and encoding, the SAS derivation and its digits, the frame header layout, the
  capability bits, and every bound field are as Amendment 1 left them.

### Why the version moves with the name

A separator change alone would leave both peers agreeing on version `1.0` and failing at message
2 with an authentication failure, which is exactly what a tampering attacker produces. A user
would be told their pairing was attacked when it had merely aged. `2.0` makes the refusal
explicit: exact-version compatibility rejects a `1.0` offer with `incompatible_version` before
any key material is derived, and the peer can say plainly that both sides must be updated.

### What it costs

Every paired device must pair again. A stored Controller-v1 device record cannot authenticate a
Controller-v2 channel, because the static keys were bound into a transcript this version does not
produce. That cost is paid once, and it is paid now rather than at the next identity change.

### Evidence

Every vector in `controller-v2.json` and `controller-code-v2.json` was regenerated, in review,
from the same fixture keys as before, so the two files differ from their predecessors in exactly
the values the separators and version fields feed. The previous files are kept beside them as
`legacy/controller-v1.json` and `legacy/controller-code-v1.json`, and
`prior_vectors_are_refused` demonstrates the compatibility policy: the v1 offer bytes are
rejected with `incompatible_version`, and the v1 prologue is not a prologue this version
produces.

This amendment does not change the release gate. The independent cryptographic review named at
the top of this document remains outstanding, and renaming a separator is not a substitute for
it.

## Golden vectors and change control

`crates/multiplex-controller-security/tests/vectors/controller-v2.json` stores fixture-only private/public static and ephemeral keys, exact offer/prologue, all three messages, final `h`, SAS, both split transport keys, and first/last legal frames. A conformance run consumes those bytes; it never regenerates missing fields. The verification script checks the fixture plus ADR and lockfile checksums. Any deliberate protocol or dependency change must update this ADR first, regenerate every vector in review, and demonstrate that prior vectors fail under the declared compatibility policy.

## Residual risks and release gates

- Neither `clatter` nor this composition has been independently audited.
- The 40-bit SAS assumes an attentive out-of-band human comparison and later Host-side attempt limiting. It does not protect a user who approves a mismatch.
- Code pairing relies on CPace's security argument and on this implementation of it, neither of which has been independently reviewed here. A code read by an attacker (shoulder surfing, screen sharing) lets that attacker pair within the code's lifetime; recording-friendly mode does not hide the code.
- Endpoint compromise, malicious platform secure storage, screen capture, accessibility-service compromise, memory disclosure outside zeroized values, and traffic analysis are out of scope for this cryptographic channel.
- Atomic nonce use, device persistence, revocation races, lost final ACK, secure-store invalidation, and route-specific denial of service are required in later goals.
- Direct remote exposure remains prohibited until D06, route-specific threat tests, native secure-store conformance, and independent professional cryptographic review are complete.

Acceptance of this ADR is an engineering contract, not an audit or security certification.
