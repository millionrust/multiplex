# Replication authority lifecycle

## Decision

TermiRust uses a bounded, storage-neutral workspace authority state machine to bootstrap replication,
enroll device wrapping keys, rotate workspace epochs, and revoke devices. The authority state holds
only validated public metadata: workspace ID, trusted authority public key, monotonic revision,
current nonzero key epoch, and deterministic device records. The fresh epoch key remains in a
separate zeroizing transition value.

Every command after bootstrap carries the exact caller-observed authority revision. A successful
transition advances the revision and key epoch by exactly one, creates a fresh random E12.2 key,
and uses E12.3 to wrap it independently to every active device in replica-ID order. The transition
is returned only after every wrap succeeds. A future repository must compare-and-swap the recorded
base revision before publishing the state and distribution; this pure leaf does not claim durable
atomicity by itself.

## Membership transitions

Bootstrap creates revision 1 and epoch 1 with exactly one active device. Enrollment rejects reused
replica IDs and all previously enrolled public keys, then rotates for the old active membership plus
the new member. The new member receives no package for an earlier epoch. Manual rotation preserves
membership while replacing the epoch key.

Revocation records the caller-supplied last accepted causal counter, the new authority revision, and
the new epoch. The revoked device is excluded from that epoch's distribution; all remaining active
devices receive it. E12.1 policy projection preserves history at or below the cutoff and rejects
new authorship and counters above it. The ordinary revocation API refuses to remove the final active
device so accidental workspace abandonment requires a separate future destruction/recovery design.

The state retains at most `MAX_REPLICATION_REPLICAS` records, including revoked records, because
E12.1 version vectors have the same lifetime replica bound. Devices and packages are held in ordered
collections so visible/package order never depends on hash iteration.

## Security meaning

[RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html) supplies the authenticated single-recipient
HPKE operation used by E12.3. It authenticates possession of the configured authority key and does
not provide replay or rollback prevention, erase recipient secrets, or establish whether a device
is currently authorized. The authority revision, expected epoch, bound context, and future durable
compare-and-swap provide those application-level checks.

[RFC 9420](https://www.rfc-editor.org/rfc/rfc9420.html) is guidance for the lifecycle principle that
membership changes create new epochs and that security depends on excluding removed members from
new secrets and deleting obsolete key material. TermiRust is not implementing MLS, its ratchet tree,
or its forward-secrecy/post-compromise-security guarantees in this leaf.

A removed device may retain every key and plaintext it learned before revocation. Rotation provides
future-access denial only: it prevents that device from opening new epoch packages. It is not
cryptographic erasure and does not make old records secret from a previously authorized device.

## Failure and secret handling

State, records, transitions, distributions, package associations, and errors redact workspace,
replica, public-key, wrapped-package, and epoch-key material from debug/error output. Authority,
device, and epoch private materials retain zeroization on drop. Invalid/stale revisions, wrong
authority keys, duplicate identities/keys, device limits, unknown/already-revoked/final devices,
integer overflow, zero or unavailable randomness, and any HPKE failure return content-free errors
without mutating the borrowed state or publishing a partial distribution.

## Deferred boundary

This decision does not define Keychain/Keystore/keyring storage, authority-private-key recovery,
historical epoch-key retention, repository encoding or migration, shared-folder/network transport,
background retries, invitation/pairing UX, conflict UI, or workspace destruction. D04 still gates
retention, backup, export, and deletion defaults. D05 continues to prohibit an operated account or
cloud-sync service without explicit authority.
