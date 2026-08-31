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

The implementation is `clatter = 2.2.0`, pinned exactly in the crate manifest and `Cargo.lock`, with default features disabled and only `alloc`, `use-25519`, `use-chacha20poly1305`, and `use-blake2` enabled. Application SAS derivation additionally pins `hkdf 0.12.4`, `sha2 0.10.9`, `subtle 2.6.1`, and `zeroize 1.8.2`.

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

The Noise prologue is `ASCII("termirust-controller-v1\0") || u16be(84) || offer_bytes`. This binds version, suite, expiry, nonce, Host identity, and requested capability template to the Noise transcript without making the QR/offer secret.

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

1. `salt = SHA-256(ASCII("termirust-controller-sas-v1\0") || pairing_nonce[32])`.
2. `info = ASCII("sas\0") || u16be(major) || u16be(minor) || host_static_public_key[32] || device_static_public_key[32]`.
3. Run HKDF-SHA256 with `IKM = h`, that salt and info, and output exactly five bytes.
4. Read the 40 bits MSB-first as eight five-bit indices into `0123456789ABCDEFGHJKMNPQRSTVWXYZ`.
5. Display uppercase `XXXX-XXXX`. There is no checksum, modulo, discarded range, or localized alphabet.

The comparison space is exactly 40 bits. SAS is comparison-only and is never entered as authentication. Accessible speech identifies each visible symbol as `letter` or `digit`. `Debug` output is redacted. The normative independent anchor is committed in `tests/vectors/controller-v1.json` and yields `YKHM-ZHBT`.

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

Unknown bits fail. Every opened or sealed frame must match both a granted capability and the exact current revocation epoch. A stale or future epoch is denied. Later Host code must validate the same policy again at the command boundary; this crate is not sole authorization merely because decryption succeeded.

## Transport framing

The Noise split produces two directional ChaChaPoly cipher states. Controller-v1 uses those selected-implementation cipher states directly so terminal ciphertext frames can exceed Noise's 65,535-byte handshake/message convenience limit while retaining the standard Noise nonce and rekey primitive. No application-authored DH, AEAD, hash, or key schedule exists.

The authenticated 32-byte frame header is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `TCF1` |
| 4 | 2 | major `1` |
| 6 | 2 | minor `0` |
| 8 | 1 | kind: control `1`, terminal `2` |
| 9 | 1 | one closed capability value |
| 10 | 2 | reserved zero |
| 12 | 8 | revocation epoch |
| 20 | 8 | sequence / Noise nonce |
| 28 | 4 | ciphertext byte length including 16-byte tag |

The complete header is ChaCha20-Poly1305 associated data. Sequence starts at zero, must equal the receiver's next sequence, and may not exceed `2^64 - 3`. A lower value is duplicate; a higher value is out of order. Directional cipher state rekeys immediately before each sequence divisible by `2^20` except zero. Rekey failure closes the channel. Frames are never retried under the same nonce.

Control plaintext is at most 65,536 bytes. A complete terminal frame, including the 32-byte header and 16-byte tag, is at most 1,048,576 bytes. Length is validated with checked arithmetic before allocation or crypto. Ciphertext, plaintext payload, and transport `Debug` are redacted.

## Compatibility

Controller-v1 has exact-version compatibility only. Unknown major or minor versions return `incompatible_version` before interpreting mutable fields. Unknown capabilities, kinds, suites, flags, or nonzero reserved bytes fail closed. A future compatible minor requires an ADR amendment and new immutable vectors; silent downgrade and best-effort parsing are forbidden.

## Golden vectors and change control

`crates/termirust-controller-security/tests/vectors/controller-v1.json` stores fixture-only private/public static and ephemeral keys, exact offer/prologue, all three messages, final `h`, SAS, both split transport keys, and first/last legal frames. A conformance run consumes those bytes; it never regenerates missing fields. The verification script checks the fixture plus ADR and lockfile checksums. Any deliberate protocol or dependency change must update this ADR first, regenerate every vector in review, and demonstrate that prior vectors fail under the declared compatibility policy.

## Residual risks and release gates

- Neither `clatter` nor this composition has been independently audited.
- The 40-bit SAS assumes an attentive out-of-band human comparison and later Host-side attempt limiting. It does not protect a user who approves a mismatch.
- Endpoint compromise, malicious platform secure storage, screen capture, accessibility-service compromise, memory disclosure outside zeroized values, and traffic analysis are out of scope for this cryptographic channel.
- Atomic nonce use, device persistence, revocation races, lost final ACK, secure-store invalidation, and route-specific denial of service are required in later goals.
- Direct remote exposure remains prohibited until D06, route-specific threat tests, native secure-store conformance, and independent professional cryptographic review are complete.

Acceptance of this ADR is an engineering contract, not an audit or security certification.
