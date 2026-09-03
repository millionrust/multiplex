# N06 Native Mobile SSH Controller Evidence

Status: Complete for the explicit user-configured SSH Controller route.

## Product behavior

- iOS and Android expose `Controller over SSH` as a separate Device route. It is not labeled or
  implemented as Direct SSH.
- Configuration is per paired Host and requires an endpoint, port, username, pinned OpenSSH host
  key or SHA256 fingerprint, authentication method, and credential.
- Route metadata contains only a typed credential reference. Secrets are stored in iOS Keychain
  or Android Keystore-backed storage and are deleted when the route or paired Host is removed.
- Selecting the route remains an explicit confirmed action. Failure or removal never silently
  falls back to LAN/VPN or another route.
- The SSH channel may execute only `termirust controller-bridge --stdio`. The existing Controller
  protocol still owns Host authentication, capabilities, cancellation, sequence/replay checks,
  and the single-writer lease.

## Native transport evidence

`./scripts/test-mobile-controller-ssh-transports.sh` starts a disposable pinned OpenSSH fixture
and executes the real Swift NIOSSH and Kotlin SSHJ adapters. Both clients pass:

1. OpenSSH private-key authentication and bidirectional bridge bytes.
2. Password authentication and bidirectional bridge bytes.
3. Rejection of a mismatched pinned Host key.

The fixture provides only a test `termirust controller-bridge --stdio` echo shim. It proves native
SSH authentication, pinning, fixed-command execution, and duplex ownership; it does not pretend
to replace the Rust Controller authority tests.

## Controller authority evidence

`./scripts/test-controller-ssh.sh` passes the Rust SSH Controller suites for JSON behavior, strict
SSH argv construction, reconnect/cancellation ownership, pairing, and authority revalidation at
the remote bridge. The focused iOS route/coordinator suites and Android unit suite also pass.

Additional passing gates:

- `cargo test -p termirust-mobile-ffi` (10 tests)
- Android `:app:testDebugUnitTest :app:assembleDebug`
- iOS generic simulator application build
- iOS `AppleControllerRouteTests` and `AppleControllerRouteViewModelTests`

## Known build warnings

SwiftNIO currently emits package-level module-map and unavailable `Sendable` conformance warnings.
They do not fail the Swift 6 build and are not introduced by the route protocol.
