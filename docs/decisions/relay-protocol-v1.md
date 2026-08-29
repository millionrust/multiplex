# Relay Protocol v1

- Status: accepted for local-loopback core only
- Date: 2026-08-29
- Scope: `termirust-relay-protocol` and `termirust-relay-server`
- Product/public authority: none; D06 gates product clients and D05 gates public operation

## Decision

Relay v1 forwards already authenticated Controller-v1 ciphertext as whole, bounded WebSocket binary messages. The relay authenticates separate route-scoped Host and Controller admission credentials, pairs exactly one endpoint of each role, checks only canonical outer fields, and keeps ciphertext in bounded memory only while both peers are online. It cannot decode, forge, acknowledge, persist, replay, or recover an inner Controller mutation.

The server supports cleartext WebSocket only on an IP loopback bind and injected rustls TLS on loopback for WSS conformance. `RelayServerConfig` rejects every non-loopback bind before opening the state store or socket. Certificate creation, public binding, reverse proxying, client routes, packaging, accounts, and deployment remain outside this goal.

This ADR finalizes one security amendment to the Goal 22.1 research transcript: the 32-byte role-specific admission verifier is included directly in the signed challenge bytes. This makes verifier substitution detectable from the proof itself. The canonical vectors in `tests/fixtures/relay-v1/vectors.json` replace the research harness as the exact v1 contract.

## Dependencies

Runtime dependencies are exact workspace pins:

- `ed25519-dalek 2.2.0` for relay-scoped admission signatures
- `tokio-tungstenite 0.28.0` / `tungstenite 0.28.0` for RFC 6455 framing and upgrade validation
- `rustls 0.23.37` / `tokio-rustls 0.26.4` with the ring provider for injected TLS
- `tokio 1.50.0` / `tokio-util 0.7.18` for bounded tasks, channels, timeouts, and cancellation
- `fs2 0.4.3` for an OS-released exclusive metadata-store lock
- `serde 1.0.228` / `serde_json 1.0.149` for versioned public admission metadata
- `zeroize 1.8.2` for admission private seeds

`rcgen 0.14.10` is test-only and creates ephemeral loopback certificates. No test key or certificate is committed. The protocol crate has no socket, filesystem, Controller, Host/session/domain, terminal, PTY, artifact, approval, or UI dependency.

## Outer Envelope

`RelayEnvelopeV1` is canonical big-endian binary data:

| Field | Bytes | Rule |
|---|---:|---|
| Magic | 4 | `TRR1` |
| Version | 2 | exactly `1` |
| Direction | 1 | `1` Host to Controller, `2` Controller to Host |
| Reserved | 1 | exactly zero |
| Route ID | 32 | opaque random value; never derived from product IDs |
| Connection sequence | 8 | starts at zero and increases by exactly one per direction after pairing |
| Ciphertext length | 4 | 1 through 1,048,576 |
| Ciphertext | bounded | unchanged sealed Controller frame |

The envelope header is 52 bytes. The ciphertext maximum is 1,048,576 bytes and the WebSocket binary-message maximum is 1,048,640 bytes. The 12-byte difference between the maximum envelope and WebSocket limits is reserved transport headroom, not an application extension point. Unknown version, nonzero reserved byte, invalid direction, empty/mismatched length, replay, gap, route mismatch, and role/direction mismatch fail closed.

## Admission

The HTTP upgrade must use path `/relay/v1`, exact Origin `termirust://relay-local`, and negotiated subprotocol `termirust-relay-v1`. Browser-style cross-origin or missing-origin upgrades are rejected before admission. WebSocket masking and frame/message bounds remain enabled.

Binary admission messages are:

| Message | Bytes | Contents |
|---|---:|---|
| Client hello | 40 | magic/version/role/reserved/route ID |
| Server challenge | 128 | hello binding plus verifier, epoch, serial, expiry, and 32-byte CSPRNG nonce |
| Client proof | 112 | magic/version/role/reserved/route ID/serial/Ed25519 signature |
| Server result | 16 | magic/version/closed diagnostic/connection ID or canonical zero |

The signature transcript is `termirust-relay-admission-v1\0 || canonical_challenge`. It binds protocol version, route, role, exact credential verifier, current revocation epoch, single-use serial, absolute expiry, and fresh server nonce. A challenge is source-bound, expires after 30 seconds, and is consumed before proof verification so every replay fails. Host and Controller use different relay-scoped credentials provisioned over an already authenticated direct route; these are not Controller-v1 long-term keys.

Exactly one Host and one Controller may occupy a route. Pairing resets connection-local sequences. Duplicate role, role swap, unknown/revoked route, stale epoch, invalid signature, expired/replayed proof, pair cap, handshake cap, and source failure cap return closed stable diagnostics.

## Compiled Limits

Configuration can lower but never exceed:

| Limit | Maximum |
|---|---:|
| Registered routes | 1,000 |
| Simultaneously forwarding pairs | 100 |
| Unauthenticated handshakes | 4 |
| Failed admissions/source/10 minutes | 5 |
| Admission lifetime | 30 seconds |
| Idle heartbeat | 90 seconds |
| Queue per endpoint | 64 messages or 4,194,304 encoded bytes |
| Route throughput | 4,194,304 bytes/second sustained; 8,388,608-byte burst |
| Ciphertext payload | 1,048,576 bytes |
| Encoded WebSocket message | 1,048,640 bytes |

Item, encoded-byte, and token budgets are charged before enqueue. The first exceeded queue/rate/sequence/route constraint cancels both route tasks, drops queued ciphertext, and preserves the specific aggregate close code long enough for both peers to observe it. A missing peer receives only heartbeat traffic; application bytes are rejected rather than queued.

## Runtime State

Server state is `Stopped -> Loading -> ListeningLoopback -> Draining -> Stopped|Failed`. Route state is `Registered -> HostWaiting|ControllerWaiting -> Forwarding -> Closed|Revoked`.

Each connection has one bounded outgoing channel and a cancellation token. Two authenticated endpoint tasks forward whole opaque messages. Disconnect closes the exact route and drops both queues. Revocation first atomically persists the incremented epoch/revoked flag, then cancels both endpoints. Shutdown stops accept, cancels unauthenticated/authenticated tasks, waits at most two seconds, releases the OS lock, and persists no connection, sequence, queue, or ciphertext state. Restart requires fresh admission and sequences.

## Metadata Store

`relay-state-v1` JSON contains only format/version and validated route ID, Host/Controller public verifiers, revocation epoch, quota, and revoked flag. It is capped at 1 MiB. Unix directories/files use modes `0700`/`0600`. An OS advisory exclusive lock is automatically released on crash.

Mutation writes a same-directory stage file, flushes it, atomically renames it, then fsyncs the parent directory. Injected failures before the stage write, after the stage write, after the stage sync, after rename, and after parent sync prove recovery yields either the complete old state or the complete new state. A real restrictive-directory test proves write permission failures preserve the current state. Unknown fields, duplicates, invalid verifiers/quotas, permissive permissions, oversize, corrupt, or newer state fail closed without rewriting evidence. Plain credentials, Controller ciphertext, terminal/session metadata, addresses, and logs never enter the store.

## TLS And Residual Risk

`RelayTlsServerConfig` accepts an injected standard rustls server configuration and redacts its Debug representation. The test suite proves trusted WSS admission and that cleartext sent to a TLS listener never reaches HTTP/WebSocket handling. Plain `ws://` remains permitted only on loopback for local engineering. No downgrade or silent fallback exists.

TLS and Controller encryption do not hide endpoint IP, connection timing, ciphertext size, direction, volume, route occupancy, or infrastructure metadata from a relay/operator. The core exposes only aggregate counters and stable codes; it has no per-route logs, address output, analytics, account, provider, license, or content metrics. A malicious operator can still delay, correlate, or drop ciphertext.

## Diagnostics

The 35 numeric/string pairs in the vector corpus are closed and locale-neutral. Human/operator surfaces must render localization IDs and textual state; meaning cannot depend on color. Debug and evidence output redact route IDs, verifiers, credentials, nonces, ciphertext, and protected TLS configuration. `NO_COLOR` requires no special path because the core emits no ANSI styling.

## Performance Decision

The reproducible real TCP/WebSocket loopback results are in `docs/benchmarks/relay-core-2026-08-29.md`. Acceptance thresholds are p99 sequential admission below 2 seconds and p99 concurrent 1 KiB round trip below 500 ms at each 1/10/100-pair point, with zero queue drops, persistent ciphertext bytes, and per-route log bytes. The recorded run passes these conservative local-core thresholds. It is not WAN, public capacity, reverse-proxy, multi-process, or provider evidence.

## Authority Boundary

- D06 must approve exact Host/desktop/iOS/Android relay routes, capabilities, lost-device behavior, and visible UX before a product client imports these crates.
- D05 must approve operator, regions/processors, retention/privacy, procurement/budget, credentials, abuse, incident, on-call, legal responsibility, and explicit deployment before any public endpoint.
- Goal 22.2.1 creates no product route, package, account, service, purchase, public listener, production credential, or production data.
- Direct LAN/VPN/SSH routes remain independent and free. Relay can never become mandatory or a silent fallback.

## Verification

Canonical source and vectors are checked by `scripts/verify-relay-v1-vectors.sh --check`. Real-loopback measurements are produced by `scripts/bench-relay-core.sh --loopback-only --pairs 1,10,100 --runs 10`. Protocol property tests and the framework-free `relay_fuzz_decode` stdin target exercise every decoder; hostile socket tests, exact boundary tests, WSS tests, crash matrices, strict Clippy, workspace tests, and dependency policy are required before changing this ADR to a product decision.
