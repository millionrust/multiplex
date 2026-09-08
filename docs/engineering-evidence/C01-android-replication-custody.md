# C01 Android Replication Custody

Date: 2026-09-07

Status: C01 complete under the brief's exit gate, with the unrelated canonical
baseline failure documented below. This is not a claim that the whole product's
verification suite is green.

Execution authority: `/Users/jacob/Downloads/termirust-codex-next-2026-09-07.md`.
Started from clean `6dd12ba` on branch `test`. No commits or pushes are authorized
by this brief. C02 and subsequent goals have not been started.

## Delivered Surface

- `ReplicationKeystoreStore` implements the generated `ReplicationSecureStore`.
- Production Android includes real Kotlin UniFFI code and four Rust ABI libraries.
- A debug-only, non-exported service supplies a second process for race testing.
- Instrumentation invokes `ReplicationCustody` through the packaged native library.
- A selected-device or owned read-only emulator runner verifies APK checksums and
  performs identity prepare/reopen/cleanup across an explicit app-process stop.

No startup identity creation, permissions prompt, enrollment screen, Controller
protocol change, terminal change, or iOS target change was introduced. This goal
does not make mobile enrollment or synchronization usable yet.

## Artifact Construction

Commands:

```sh
bash scripts/build-mobile-replication-bindings.sh --android
bash scripts/sync-mobile-replication-bindings.sh --android --write
bash scripts/sync-mobile-replication-bindings.sh --android --check
```

All four ABI slices build serially with Rust 1.97.1, UniFFI 0.32.0, NDK
27.0.12077973, API 26, and 16 KiB ELF LOAD alignment. The builder reuses the
workspace's pinned UniFFI generator without modifying the Controller pipeline.
It checks ELF architecture, required exported custody symbols, alignment, and
SHA-256 inventories before promoting the complete staged set.
Two clean four-ABI builds produced identical inventories and generated Kotlin;
`--check` against the previously packaged set passed after the second build.

The generated Kotlin and libraries live together in the added
`app/src/main/replication/{kotlin,jniLibs}` source set. This intentionally differs
from copying libraries into the existing mixed Controller JNI directories: one
owned directory can be staged/replaced without touching other native libraries.
There are still four standard ABI directories and standard APK `lib/<abi>` entries.
Gradle preserves the already-stripped Rust bytes, allowing the runner to compare
every APK library's SHA-256 directly to its verified input.

`python3 scripts/test-mobile-replication-artifacts.py`: four tests passed, covering
incomplete/tampered artifacts, rollback after a simulated publication failure,
preservation of an unrelated Controller sentinel, and bounded child-process cleanup.
This does not claim power-loss durability for the developer artifact-promotion step.

## Storage Contract

- Dedicated namespace below `noBackupFilesDir/replication-secrets`, and a distinct
  `termirust-replication-<namespace>-v1` Keystore alias.
- 1-128 printable non-space ASCII account bytes, internally hashed filenames.
- Exactly 47 plaintext bytes; v1 ciphertext is exactly 76 bytes. Reads allocate a
  fixed 77-byte buffer to detect excess input. Namespace, version, and account are AAD.
- Device credentials are required. Locked/no-credential states fail as `Locked`;
  no authentication UI is launched by storage or app startup.
- API 30+ uses credential-only timed key authentication; API 26-29 uses the older
  authentication-validity API. API 28+ additionally requires an unlocked device in
  Keystore. Every operation checks device credential/unlock state before waiting
  and again after acquiring the namespace lock. The authentication window is 300s.
- A bounded striped JVM lock plus a stable OS file lock coordinates instances and
  processes. The combined lock-wait budget is two seconds; interruptions preserve
  interruption status and return a structured failure without publication.
- Directory, lock, and account paths reject symlinks/non-regular files. Native
  no-follow opens and lock inode checks guard file substitution. These are not a
  claim of protection against arbitrary code already executing under the app UID.
- Create rejects committed base/backup state, removes only stale uncommitted
  pending state, fsyncs a new private file, renames it, then fsyncs the directory.
- A backup is authenticated before recovery; corrupt evidence is retained.
- Loading never creates a wrapping key. Creating with a lost key and committed
  ciphertext also fails rather than provisioning a replacement.
- Deletion removes only the selected account's base/backup/pending state. It does
  not delete the namespace lock, other identities, Controller data, or wrapping key.
- Owned mutable plaintext buffers are cleared; FFI/immutable-copy zeroization is
  not claimed. Errors exclude account IDs, references, key bytes, and OS messages.

The platform behavior is based on Android's
[Keystore builder contract](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec.Builder)
and [native filesystem APIs](https://developer.android.com/reference/android/system/Os).

## Device Proof

```sh
bash scripts/test-android-replication-custody.sh --avd Pixel_9
```

Three complete runs passed **13 tests each, zero skipped** (10 custody tests plus
prepare, reopen, cleanup). Final runtime: API 37, arm64-v8a, 16384-byte pages on the
Pixel_9 Google APIs/Play AVD. The other three ABIs were built and verified in the
APK, not executed on separate devices.
The runner starts it read-only with snapshots disabled, sets a fixture PIN only in
that disposable emulator, and stops only its owned emulator process.

Verified:

- Real Rust-created identities reopen through new store/custody instances.
- Reopening after a runner-driven process stop verifies a different PID and the
  same two public keys, using privately persisted references rather than key exports.
- Two threads and a second Android process race: exactly one create succeeds and
  one returns Collision; the winning typed secret envelope is unchanged.
- Exact/idempotent identity deletion preserves another identity and a real
  Controller-store sentinel.
- Controller and replication Rust libraries both load and execute in the same
  process; Controller protocol inspection works before and after replication use.
- Backup-only recovery, stale pending state, undeletable recovery state, and
  record/namespace symlink rejection.
- Empty/truncated/oversized/bad-version/bad-tag ciphertext and account substitution
  fail without repairing or replacing evidence.
- Lost wrapping key blocks both reads and new-account creation; no replacement key.
- Invalid accounts and wrong reference roles fail before custody access.
- Main-thread storage calls fail immediately rather than blocking on custody.
- Lock timeout and interruption do not publish an account.
- Access-denied exception mapping is simulated and labeled as such.

No personal device was locked, cleared, or uninstalled. Fixtures clean up their own
namespaces and aliases; the restart fixture has an explicit cleanup stage. This is
not physical-device lifecycle or API-26 runtime qualification.

## Regression Gates

| Check | Result |
|---|---|
| Rust custody contract tests | 5 passed |
| Rust custody Clippy, all targets, warnings denied | Passed |
| Shared replication product tests | 11 passed |
| Swift in-memory generated-binding conformance | Passed |
| Android unit tests + both debug APKs + lint | Passed; 76 unit tests: 72 passed, 4 existing live-fixture skips |
| APK library checksums, four ABIs | Passed |
| Explicit missing-device selection | Failed as required, before installation |
| Canonical local product-model verifier | Failed on the pre-existing Controller fixture Cargo.lock checksum |
| Final C01 instrumentation run | 13 passed, zero skipped |
| Existing Android Controller/Host golden | 1 passed: pairing, control, reconnect, revocation |
| Android unified-route verifier, run separately | Passed |
| iOS unified-route verifier, run separately | Production device build and simulator lifecycle suites passed |
| Terminal/Session fixture synchronization and three mobile/remote route validators | Passed separately |
| Artifact publication/process cleanup tests | 4 passed |

The four JVM skips are the existing direct-SSH test and three SSH Controller live
tests without their external fixtures. None are replication instrumentation tests.
During implementation, the runner's PIN-success recognition and SDK propagation
were corrected. The APK check caught Gradle re-stripping verified inputs; packaging
now preserves the release bytes. Those incomplete runs are not counted as passes.

## Canonical Baseline Failure

`./scripts/verify-product-model.sh --local` exited 101 in the workspace Rust-test
step. The failing test is
`termirust-controller-security::fixture_locks_transport_keys_last_sequence_and_document_checksums`,
at `crates/termirust-controller-security/tests/golden_vectors.rs:175`.

It hashes the whole workspace lockfile and compares it with the frozen Controller
fixture. The isolated exact test reproduced the failure:

- Current/HEAD lockfile: `0886142a000877bce24e47fcc9bb8c762dbd9b8f0c132cd31f66c12bf412729b`.
- Fixture expectation: `4d1f9e0c8981b182ac4e10a461bfdaff47f119f68f93a5882b9aee16a32e9bff`.
- `git show 3d55d96^:Cargo.lock | shasum -a 256` matches the fixture expectation.
- `git show HEAD:Cargo.lock | shasum -a 256` matches the current lockfile.
- C01 changes neither Cargo.lock nor that fixture. The mismatch predates C01 and
  comes from adding the replication binding crate in commit `3d55d96`.

No checksum assertion was weakened or fixture silently refreshed. The aggregate
verifier stopped there; subsequent aggregate steps, Rust docs, and policy were not
reached in that invocation. The platform and fixture checks listed above were run
separately, not reported as an aggregate pass.

The brief permits C01 completion with an unrelated baseline failure precisely
recorded. This failure still needs a separately scoped fixture/provenance update
before the canonical baseline can be called green.

## Cleanup And Limits

Owned C01 Android emulators were read-only and stopped. Fixture entries and wrapping
aliases were removed; restart cleanup completed. The existing Controller golden
runner also stopped its Host and emulator and restored its test resource. No app
data was cleared or uninstalled, and no unrelated files were removed. Disk checks
remained above 15 GiB (approximately 18-19 GiB free).

Physical lock/unlock testing, API-26 runtime behavior, other hardware/ABI runtimes,
and mobile enrollment/sync UI remain unproven by C01. The new debug process service
is not in the release manifest. C01 modifies no iOS source or target configuration.

## Next Queued Goal

C02: package the existing Apple replication binding/Keychain adapter into the iOS
app and prove it through the shipped framework on a simulator. Do not activate C02
as part of this C01 run.
