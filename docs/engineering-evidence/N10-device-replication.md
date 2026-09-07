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
- `python3 scripts/clippy-changed.py`: changed Rust lines passed.
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
- `bash scripts/test-swift-replication-bindings.sh` passed a compiled Swift callback
  round trip against the Rust library.

The new boundary is not yet packaged into either app. Swift conformance uses an
in-memory store; neither native Keychain/Keystore implementations nor Android
runtime coverage are established by it. Mobile product service, transport, and UI
integration remain open. The Controller binding/API is unchanged.
