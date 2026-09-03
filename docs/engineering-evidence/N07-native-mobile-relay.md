# N07 Native Mobile Relay Evidence

Status: Complete for implementation and automated native iOS-simulator and Android-emulator
coverage.

## Product Behavior

- Swift and Kotlin use native WebSocket transports for an explicitly selected self-hosted relay.
- Both adapters require `wss://.../relay/v1`, normal platform TLS validation, an exact SPKI pin,
  the TermiRust Origin/subprotocol, a role-specific admission credential, and current epoch.
- Admission and envelope encoding/decoding use the shared Rust relay protocol through the mobile
  FFI, preventing independently implemented wire formats.
- Route credentials are held in iOS Keychain or Android Keystore-backed storage. Route settings
  persist only credential references and nonsecret metadata.
- Mobile setup accepts the strict `controller-route.json` operator package from the clipboard.
  Wrong schema, role, path, pin, route length, secret length, or unknown fields fail closed.
- `termirust relay-host` imports only `host-route.json`, stores its credential in the OS keyring,
  runs the existing Controller-v1 Host backend over the relay, reconnects with a bounded delay,
  and removes only route state.
- `termirust-relay` provisions independent Host/Controller secrets, stores only verifiers,
  supports revoke/remove, bounds handshakes/frames/queues/rates, and never persists payloads.

## Automated Evidence

Passing during the N07 implementation:

- `cargo test -p termirust-relay-server -p termirust-relay-client --all-targets`
- `cargo clippy -p termirust-relay-server --all-targets -- -D warnings`
- `cargo test relay_host_service --bin termirust -- --nocapture`
- `cargo test -p termirust-mobile-ffi`
- Android `:app:testDebugUnitTest :app:assembleDebug`
- iOS `AppleControllerRouteTests`
- Rust relay Host/Controller end-to-end, hostile TLS/limit, revocation, crash-recovery, replay,
  sequence, queue, and reconnect suites

`./scripts/test-mobile-controller-relay-transport.sh` executes one non-skipped native iOS XCTest
against a disposable TLS relay. Two fresh Swift transports send distinct payloads through the
canonical Rust relay to a reconnecting Rust Host and receive exact echoes. The harness rejects
zero-test and XCTest-skip results and removes its CA, packages, cloned simulator, and processes.

`./scripts/test-mobile-android-relay-transport.sh --avd Pixel_9` executes the corresponding
non-skipped Android instrumentation test against the same Rust relay and echo Host. Two fresh
OkHttp transports use the production JNI admission/envelope implementation, exact SPKI pin, and
bounded duplex stream. The disposable CA is trusted only by the instrumentation HTTP client;
`adb reverse`, generated route material, APK fixture assets, and owned processes are removed or
restored by the harness.

Local result on 2026-09-03: `OK (1 test)` in 0.298 seconds followed by the harness PASS marker;
the Rust echo Host exited successfully after observing both Android connections.

## Security Boundary

Relay diagnostics contain closed codes, roles, counts, and lifecycle states only. Tests assert
that relay metadata and diagnostics contain no route credentials or forwarded plaintext. The
relay sees only Controller-v1 ciphertext inside ordered outer envelopes and cannot grant terminal
authority; the Host remains authoritative for pairing, capabilities, replay, and writer leases.

## Limitations

- No public relay service is supplied or required.
- Android relay coverage is emulator evidence, not a physical-device or external-network result.
- TLS certificate issuance, DNS, reverse-proxy deployment, OS service installation, mobile code
  signing, and external-network behavior remain operator/environment responsibilities.
- SwiftNIO package warnings observed in Xcode belong to the SSH dependency graph and are not relay
  protocol failures.
