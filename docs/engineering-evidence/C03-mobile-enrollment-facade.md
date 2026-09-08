# C03 Mobile Enrollment Facade

Date: 2026-09-08

Status: complete. C04-C06 are queued, not active.

Scope: a separate typed UniFFI object over `ReplicationProductService`, exposing
prepare, pending request reload, and exact cancellation. No enrollment UI, new
crypto, provider URI conversion, or automatic identity creation.

The existing service requires a local filesystem exchange directory. C03 accepts
only actual directories; Android document URIs and Apple provider URLs are rejected.
This is a local-filesystem API contract, not provider transport completion. C06 must
add a real adapter before providers can be used. Native callers supply a dedicated
private no-backup container and invoke synchronous methods on worker threads.

Exit: shared service and facade tests, generated Swift/Kotlin contract execution,
refreshed packaged artifacts, and existing native custody regression passes.
Record errors, cleanup, counts, and limitations before marking complete.

## Contract

`MobileReplicationProduct` is an additive object in the existing replication
library, separate from `ReplicationCustody`. It reuses the same secure-store
callback through `NativeReplicationSecretBackend`; no mobile enrollment crypto
or persistence format is reimplemented. `termirust-store` is now a dependency,
without GPUI or the optional OS-keyring feature.

The native caller supplies a canonical path for its dedicated private no-backup
container. A stable outside-root lock serializes facade calls across instances
and processes, including initial creation. Busy fails without waiting or publishing.
This is not a lock contract for unrelated direct desktop callers or a defense
against arbitrary malicious code already running under the app UID.

- Prepare delegates to the existing service's staging and secret-custody flow.
- Reload returns bounded canonical public request bytes, not private keys or
  custody references. None means the root does not exist; corruption is not None.
- Cancellation requires the reviewed canonical request. Comparing under the lock
  prevents an old screen from deleting a replacement request. StaleRequest leaves
  the current request intact. Exact deletion preserves unrelated identities.
- Pending deletion/activation is explicit RecoveryRequired. A failed secret delete
  can be retried with the reviewed request while the durable pending request remains.
- Errors are content-free and structured. No implicit network operation, automatic
  identity creation, or silent retry occurs.

## Verification

- Rust binding tests: 10 passed (existing 5 custody plus 5 facade tests).
- Shared product service: 11 passed.
- Binding Clippy, all targets with warnings denied: passed.
- Existing Swift in-memory custody conformance: passed (not native Keychain proof).
- Artifact metadata/publication tests: 3 iOS and 4 shared passed.
- iOS production device build and simulator run: 21 passed, zero skipped
  (6 custody/enrollment tests plus 15 Controller/lifecycle regressions).
- Android packaged JNI/Keystore instrumentation: 14 passed, zero skipped
  (11 custody/enrollment tests and 3 existing custody process-restart stages).
- Android JVM unit/lint gate: passed; 76 tests, 72 passed, 4 existing external-SSH
  fixture skips, zero failures/errors. These are the direct-SSH live test and three
  SSH Controller live tests, not the native enrollment proof.

Tests cover byte-identical request reopening, duplicate preparation, stale
cancellation rejection, idempotent cancellation, preservation of another identity,
Busy across independent file descriptors, URI rejection, corrupt pending evidence,
and a simulated Locked deletion followed by recovery with the same request.
Two concurrent prepare calls allocate exactly one identity and preserve the winning
request; the other call reports Busy or AlreadyConfigured.

The lockfile change adds only the binding's required dependency edges (`fs2`,
`libc`, `termirust-store`, test-only `tempfile`); versions were not upgraded.
The pre-existing canonical Controller lockfile checksum failure documented by C01
is not repaired here. No aggregate whole-product green claim is made.

## Reproduction And Native Evidence

```sh
cargo test --locked -p termirust-replication-bindings
cargo test --locked -p termirust-store --test replication_product
cargo clippy --locked -p termirust-replication-bindings --all-targets -- -D warnings
bash scripts/test-swift-replication-bindings.sh
python3 scripts/ios-replication-artifacts.py build
python3 scripts/ios-replication-artifacts.py sync --write
python3 scripts/ios-replication-artifacts.py sync
python3 scripts/test-ios-replication-custody.py --simulator 7F76A1D5-5CC3-44DD-8883-DA554B851C99
bash scripts/build-mobile-replication-bindings.sh --android
bash scripts/sync-mobile-replication-bindings.sh --android --write
bash scripts/sync-mobile-replication-bindings.sh --android --check
bash scripts/test-android-replication-custody.sh --avd Pixel_9
ANDROID_HOME="$HOME/Library/Android/sdk" ./mobile/android/gradlew -p mobile/android testDebugUnitTest lintDebug --no-daemon
git diff --check
```

Both native tests invoke the generated product object through the packaged Rust
library, prepare with the real platform store, reconstruct native store and product
instances, compare request bytes, and cancel with the reviewed request. They verify
idempotence, preservation of a separate custody identity, and rejection of a provider
URI. The fuller corruption/Busy/stale-request/failure cases run in Rust; they are not
misreported as physical-device tests.

iOS: Xcode 26.6 (17F113), iOS 26.5 arm64 simulator. Android: API 37 arm64-v8a,
16 KiB pages, owned read-only Pixel_9 emulator. All Apple slices and all four Android
ABIs were rebuilt. Required exports and packaged checksums passed; Android APK JNI
hashes matched the packaged sources. Other ABI slices were not executed.

Test namespaces and filesystem directories are fixture-owned. The iOS simulator
was shut down because the runner booted it; the Android emulator was stopped by its
runner. No personal device, unrelated credentials, or app data was cleared. Disk
checks remained at approximately 20-21 GiB free. No commits or pushes were made.

## Limits

Reload inspects public metadata, not current private-key availability. Native tests
will prove object recreation, not physical process-restart enrollment recovery.
Power-loss during partial filesystem removal and physical lock/backup behavior are
not established by these tests. If pending evidence is missing or corrupt, exact
cancellation fails closed rather than guessing which identity to delete.

C04 is queued for Android enrollment UI. C05 is the corresponding iOS UI. C06 must
introduce real provider semantics before mobile provider-backed enrollment import
or sync is possible. No UI or provider work is included in C03.
