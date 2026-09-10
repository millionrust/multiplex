# Native Replication Custody

This contains the Swift/Kotlin secure-storage boundary for the existing
`termirust-replication-security` vault and a separate mobile enrollment-preparation
facade over `ReplicationProductService`. Both are separate from Controller pairing.
It is not a mobile sync implementation or enrollment UI.

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
by this conformance runner and the iOS app target, using the separately packaged
`mobile/ios/Replication` framework. It uses a replication-only service, disables
synchronization, and selects `WhenUnlockedThisDeviceOnly`. iPhone lifecycle and
backup/restore behavior require separate device qualification.

Android custody and four ABI artifacts are covered by
[C01](../../docs/engineering-evidence/C01-android-replication-custody.md). iOS app
packaging and simulator Keychain proof are tracked in
[C02](../../docs/engineering-evidence/C02-ios-replication-custody.md).
Remaining: enrollment acceptance and provider transport adapters,
enrollment/conflict/recovery UI, and physical-device lifecycle tests.

## Mobile Enrollment Preparation

`MobileReplicationProduct` is separate from the low-level `ReplicationCustody`.
Its constructor takes an existing dedicated app-private, no-backup directory and
the platform `ReplicationSecureStore`; it creates no identity. Use a native
canonical filesystem path, not a URL string or a provider URI. Invoke synchronous
methods on a worker thread. Do not share its owned `enrollment` child with direct
desktop service callers, and never remove its stable `enrollment.lock` file.

- `prepare_enrollment(local_exchange_directory)` delegates to the recoverable
  service flow and returns inert canonical public request bytes. The existing
  service requires an actual existing local filesystem exchange directory.
- `pending_enrollment()` returns the same request across object recreation, or
  None only when the enrollment root is absent. Corrupt/existing incomplete data
  is an error. A deletion/activation transaction reports RecoveryRequired.
- `cancel_pending_enrollment(expected_request)` compares the reviewed request
  under the facade lock and delegates exact deletion to the service. A different
  request returns StaleRequest; an already absent root returns false for a valid
  request. Storage denial stays Locked, rather than appearing successfully deleted.

All calls acquire a nonblocking OS lock; Busy does not mutate data. Secret errors
remain structured, and error text excludes identifiers, paths, or native messages.
Native callers must privately retain the reviewed request to retry cancellation;
do not silently retry uncertain mutations. Reload first after an ambiguous prepare
error. Reload is public metadata inspection, not proof that custody is currently
unlocked or that enrollment or synchronization has completed.

Android document URIs and Apple file-provider URLs are read by the C06 native
adapters into bounded untrusted bytes, never passed as Rust paths. Do not simulate
a provider by silently substituting a local directory.
Preparation/export/cancellation do not contact another device.
See [C03 evidence](../../docs/engineering-evidence/C03-mobile-enrollment-facade.md).

## Reviewed Acceptance and Transfer

C07 adds these source APIs; platform packaging/runtime evidence is tracked in
[C07](../../docs/engineering-evidence/C07-android-enrollment-import.md).

- `review_enrollment(expected_request, bundle_bytes)` returns canonical claims and
  a verification code, without trusting the authority or creating keys. The native
  user flow must compare that code independently with the desktop.
- `accept_enrollment(expected_request, bundle_bytes, expected_workspace, verified_code)`
  rechecks the request and claims and delegates authenticated activation to the service.
- `recover_pending_enrollment()` explicitly resolves an existing activation journal
  without preparing keys or accepting a bundle. It preserves evidence on failure
  and does not continue unrelated authority/deletion transactions. Reload pending
  status afterward; never automatically retry acceptance.
- `review_record_transfer(document_bytes, collection, record_id)` returns authenticated
  inert plaintext, a 32-byte review token and whether local data would change. Native
  callers must validate the host envelope/schema before presenting an actionable host.
- `apply_record_transfer(document_bytes, collection, record_id, expected_token)`
  repeats authentication and commits only the selected record locally. No provider
  publication, command execution, connection attempt or automatic conflict resolution.

Keep the exact reviewed bytes and token together; never reread the provider and assume
its content is unchanged. StaleRequest requires a fresh review. Do not auto-retry an
uncertain apply; reopen and review current state. Native memory now contains plaintext
during review: do not log, persist in UI state restoration, copy or export it implicitly.
This generic record boundary is not yet a complete Android host-import workflow.

Host screens should use the newer `review_host_transfer` / `apply_host_transfer`
source APIs. They add desktop envelope/stable-ID validation through termirust-protocol
and return only inert connection details plus the review token. No credentials,
startup actions, routing or environment settings are exposed as executable configuration.
The Android four-ABI artifact set now includes these methods and the shared recovery
validation fix. A desktop-service fixture now proves successful host review/import
through Android JNI, Keystore, provider reads and real product ViewModels. System-picker
acceptance, journaled failed-activation rollback and reconstructed post-profile cleanup
across process restart now also pass on Android. The four Android ABI libraries also
include write-ahead custody intents: absent keys allow intent cleanup; present but
unconfirmed keys are preserved as RecoveryRequired. Ownership-safe resolution of this
ambiguity, exact crash timing and cold-start reliability remain open. iOS artifact regeneration
is still pending. See C07 for the exact runtime scope.

The discovery APIs `preview_host_transfers(document_bytes)`
and `imported_hosts()` return `MobileReplicatedHost` rows (record/stable IDs, label,
host, port, username). Discovery authenticates and validates schemas but is not
permission to apply: select a record and obtain a fresh `review_host_transfer` token.
Authenticated tombstones are omitted from the list, never applied as local deletions
by discovery. Invalid/unsupported hosts fail the list instead of being silently hidden.
These list APIs are now packaged for all four Android ABIs and used by the native
host-selection/review screen. JNI rejection and desktop-service-to-Android acceptance
tests pass, including duplicate import, process reopening and lost wrapping-key refusal.
iOS packaging and UI for these APIs are still pending.

Android now also packages the explicit recovery API and exposes a confirmation
action. Shared source tests cover committed finalization and incomplete-activation
rollback; Android JNI tests cover no-op recovery, malformed-journal preservation and
rollback after a real journaled publication failure across a process restart, including
failed key deletion and successful re-enrollment. Reconstructed committed cleanup is
also verified across restart without any key creation/deletion or ciphertext changes.
This is not complete native
crash-boundary qualification or complete mobile sync. See C07.
