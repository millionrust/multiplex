# Native Replication Custody

This is the Swift/Kotlin secure-storage boundary for the existing
`termirust-replication-security` vault, separate from Controller pairing storage.
It is not a mobile sync implementation or an enrollment workflow.

`NativeReplicationSecretBackend` implements the shared `ReplicationSecretBackend`
contract so a subsequent mobile product adapter can use `ReplicationProductService`
without reimplementing authority, enrollment, or record cryptography.

## Native Store Requirements

- Use a replication-only namespace, never the Controller pairing namespace.
- `create` must atomically reject collisions. Do not implement it as load followed
  by an unguarded overwrite. Report success only after durable storage completes.
- Keep secrets device-local and excluded from backup/cloud key synchronization.
- Return Missing only for absent secrets. Locked, denied, corrupt, and unavailable
  stores must remain errors; never generate replacement keys while loading.
- Delete exactly the requested account, including recoverable storage copies.
- Be thread-safe. Clear mutable secret buffers after use and never log secrets or
  opaque account/reference identifiers. FFI and Swift immutable-data copies cannot
  be promised to be zeroized; minimize their lifetime. Rust loaded buffers zeroize.
- Bound native reads before allocating. The current typed secret envelope is 47
  bytes; the Rust adapter rejects other sizes after the callback returns.

`ReplicationCustody` exposes only a device identity's opaque reference and public
key. It provides no general private-key export. Callers must privately persist the
returned reference and explicitly confirm destructive UI actions. The eventual
enrollment adapter should use the product service's recoverable enrollment flow,
not replace that flow with standalone identity creation.

## Verification And Limits

`cargo test --locked -p termirust-replication-bindings` tests the callback/backend
contract with an in-memory store, including recreation, exact deletion, collisions,
storage errors, malformed references, wrong key roles, and corrupt secret envelopes.
These tests do not establish Keychain/Keystore or desktop/mobile sync coverage.

On macOS, `bash scripts/test-swift-replication-bindings.sh` generates Swift and Kotlin
bindings and compiles/runs a Swift callback round trip against the native Rust
library. It uses in-memory storage, not Keychain, and does not compile or run Kotlin.

Add `--keychain` to run the native Apple adapter against the real macOS Keychain.
That test uses a unique test service and deletes its entries afterward. It exercises
atomic collision rejection across store instances, reopening identities, exact and
idempotent deletion, invalid accounts, and corrupt envelope lengths. Access-failure
mapping is tested with status constants, not by locking the user's device.

The Apple source lives at
`mobile/ios/TermiRustMobile/Security/ReplicationKeychainStore.swift`. It is compiled
by this conformance runner but is not yet in the iOS app target; that requires the
replication framework packaging. It uses a replication-only service, disables
synchronization, and selects `WhenUnlockedThisDeviceOnly`. iPhone lifecycle and
backup/restore behavior require separate device qualification.

Remaining: Android secure-store implementation, artifact packaging, mobile product
service/transport adapters, enrollment/conflict/recovery UI, and real-device tests.
