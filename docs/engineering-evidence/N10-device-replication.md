# N10 Device Replication

Date: 2026-09-07

Status: In progress. Desktop workflows are implemented; mobile integration and
the full two-device product acceptance gate remain unfinished.

## Implemented Desktop Workflows

- Per-record encrypted replication for user profiles, custom vaults, identities,
  snippets, and known-host pins through a user-selected shared folder.
- Device enrollment packages with verification codes, pending-request recovery,
  explicit cancellation, and device status.
- Reviewed conflict resolution, recovery, key rotation, signed authority updates,
  device revocation, and confirmed local replica deletion.
- OS credential storage for replication secrets.

## Review Freshness

A conflict review can remain open while local records change. Previously, applying
it could replace those edits with the older reviewed records. Desktop reviews now
bind the sync-eligible local records and host-key pins to a SHA-256 fingerprint.
Both apply and conflict resolution reject changed local contents before committing
the reviewed sync. A fresh review remains available through the existing sync flow.
The fingerprint is internal and contains no plaintext record values.

Regression coverage exercises edits made after review, host-key additions made
after review, rejection without publication, and successful fresh review/application.
Existing tests exercise two enrolled replicas converging and exact remote deletion.

Review preparation now captures sync-eligible records once and uses that same
snapshot for reconciliation and fingerprinting. A deterministic regression models
an SSH host pin being added after capture but before review preparation finishes;
applying that review is rejected without publication, the pin survives, and a fresh
review succeeds. This covers the preparation window, not a general cross-store
transaction or all concurrent mutations during application.

## Verification

- `cargo test replication::tests --bin termirust`: 4 passed.
- `cargo test -p termirust-store --test replication_product`: 11 passed, including
  enrollment cancellation, restart recovery, rotation, revocation, and deletion.
- `python3 scripts/dev/clippy-changed.py`: changed Rust lines passed.
- `cargo fmt --check` and `git diff --check`: passed.

## Remaining Acceptance Work

- Native Swift and Kotlin enrollment, conflict, recovery, rotation, revocation,
  and deletion screens using the shared authority contracts.
- Mobile credential-store and shared-folder or user-hosted transport integration.
- Real desktop/mobile encrypted convergence across offline edits and conflicts.
- User-facing lifecycle and interrupted-operation testing with native secure stores.

The desktop convergence fixture uses independent local replica directories and an
in-memory secret backend. It does not establish mobile UI or OS key-store coverage.

## Mobile Storage Prerequisite

The Android Controller secret store now lets `AtomicFile.openRead` recover committed
backups, deletes the base and recovery files together, bounds encrypted reads before
allocation, and refuses to create a replacement encryption key while reading an
existing secret. This fixes existing pairing storage and is prerequisite work for
mobile replication; replication is not yet wired to this store.

On 2026-09-07, `ControllerSecureBlobStoreInstrumentedTest` passed all four tests on
the Pixel 9 emulator using the real Android Keystore and file APIs:

- recover a committed backup when the base file is absent;
- delete base/backup/pending files without removing an unrelated secret;
- reject a missing encryption key without creating another one;
- reject oversized encrypted input.

Android unit tests, debug APK, and instrumentation APK builds passed. The tests use
unique secret identifiers and encryption-key aliases and remove them afterward.
The real Android Controller/Host golden test also passed after this change,
covering pairing, terminal control, reconnect, and revocation. The fixture emulator
and Gradle daemon were stopped after verification.

The recovery/deletion behavior follows the Android
[AtomicFile API](https://developer.android.com/reference/android/util/AtomicFile).

## Native Replication Custody Boundary

`termirust-replication-bindings` now exposes a separate UniFFI secure-store contract
for replication. Its adapter implements the existing `ReplicationSecretBackend`,
requires durable create-only writes with collision errors, retains distinct storage
failures, bounds accepted secret envelopes, and keeps Rust loaded buffers zeroizing.
Device identity operations return opaque references and public keys, not private
keys. They validate key roles before accessing or deleting native storage.

- Five Rust callback contract tests passed (recreation, exact deletion, collision,
  error propagation, malformed/wrong-role references, corrupt data).
- Package Clippy with `-D warnings` passed.
- Swift and Kotlin bindings generated successfully.
- `bash scripts/test/swift-replication-bindings.sh` passed a compiled Swift callback
  round trip against the Rust library.

The boundary is now packaged on Android through C01, with four verified ABI
libraries and real Rust/Keystore instrumentation: 13 tests passed, including
second-process races and app-process restart. See
[C01 custody evidence](C01-android-replication-custody.md) for exact checks, cleanup,
and the pre-existing canonical Controller lockfile-checksum failure. Swift's
in-memory conformance alone does not establish native custody. The Apple adapter
is now packaged in the production iOS target through C02, with a production device
build and five real-framework simulator Keychain tests passed. See
[C02 custody evidence](C02-ios-replication-custody.md) for regression status and limits.
A separate mobile preparation facade now delegates prepare/reload/request-bound
cancellation to `ReplicationProductService`. C03 records 21 iOS and 14 Android
native regression tests, including real platform storage for the new flow. See
[C03 facade evidence](C03-mobile-enrollment-facade.md). This accepts real local
filesystem directories only, not provider URIs. Android request UI is now integrated
through C04: Devices > menu > Enrollment supports preparation, pending reload,
copy/export, and reviewed cancellation with recoverable deletion. Its 26 emulator
test invocations include real JNI/Keystore, Compose interactions at three viewport
sizes, and a process-restart request round trip. See
[C04 Android UI evidence](C04-android-enrollment-ui.md) for exact counts and the
unqualified provider-picker/physical accessibility paths. iOS request UI is integrated
through C05 with preparation, pending reload, native copy/share/JSON export, reviewed
cancellation and Keychain deletion recovery. Final iPhone and iPad simulator runs each
passed ten tests with zero skips; the production device build and 21 custody/Controller
regressions also passed. See [C05 iOS UI evidence](C05-ios-enrollment-ui.md) for screenshot
coverage and the still-open native-picker keyboard/physical accessibility qualification.
The initial C06 provider boundary now supports bounded native document reads and
explicitly rejects automatic publication without distributed revision guarantees.
Seven Android DocumentsProvider tests and seven iOS coordinated-local-file/error tests passed,
including real temporary URI grants/revocation across UIDs, partial-transfer rejection,
and local filesystem permission denial/recovery with corrected iOS error mapping;
A separate iOS local Files picker workflow also passed: external security-scoped
read, exact bytes, cancellation and restart/reselection. Ten existing iPhone
enrollment regressions passed afterward, with zero skips. Third-party cloud-provider
hydration, persisted grants and incoming Rust service integration remain open.
See [C06 transport evidence](C06-mobile-provider-transport.md). Enrollment acceptance
and bidirectional provider transport are not complete. The Controller binding/API
is unchanged; request export does not establish encrypted record synchronization.

## Android Reviewed Acceptance

[C07](C07-android-enrollment-import.md) now records a successful desktop-service-to-
Android enrollment and encrypted-host import fixture: packaged Rust, real Android
Keystore, bounded document-provider reads, explicit Compose confirmation, duplicate
import, process reopening and missing wrapping-key refusal. The dedicated runner
passed 18 executions including write-ahead custody intent safety, real system-picker selection/cancellation, recovery
after a journaled activation failure, and finalization of reconstructed committed cleanup
state across process stops. It uses a disposable
desktop authority and synthetic host data, not desktop UI automation or a third-party
provider. Ambiguous prepared-custody resolution, exact-instruction crash timing and cold-start reliability
remain open. Imported records remain inert; this is
not bidirectional sync or automatic SSH credential/connection provisioning.

## Apple Native Custody Adapter

`ReplicationKeychainStore.swift` implements the new callback contract using a
replication-only Keychain service. `SecItemAdd` rejects collisions without replacing
an existing secret. Reads validate the typed envelope length; deletion targets only
the supplied service/account and distinguishes already absent from access failure.

`bash scripts/test/swift-replication-bindings.sh --keychain` passed on macOS using
Swift 6 language mode:

- existing in-memory Swift/Rust conformance;
- real Keychain duplicate creation preserves the original value;
- recreated store/engine loads the same device public key;
- exact and idempotent deletion leaves the other identity and fixture record intact;
- deletion leaves an identically named account in a different service untouched;
- invalid account/secret lengths and corrupt stored values are rejected;
- access-error status mapping retains Locked rather than Missing.

Fixtures use a random test service, with cleanup confined to that service. The run
does not lock the user's device; access-error coverage is mapping-only. This does
not establish iPhone background, lock, or backup/restore behavior. The adapter is
also included in the iOS app target; C02 adds simulator proof without claiming
physical-device lock, background, or backup/restore qualification.

The implementation follows Apple's
[duplicate-item semantics](https://developer.apple.com/documentation/security/errsecduplicateitem)
and [device-local unlocked accessibility](https://developer.apple.com/documentation/security/ksecattraccessiblewhenunlockedthisdeviceonly).
This accessibility prevents migration to another device; it is not evidence that
all same-device backup/restore routes have been excluded or tested.
