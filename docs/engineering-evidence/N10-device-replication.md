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

## Verification

- `cargo test replication::tests --bin termirust`: 3 passed.
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
