# Local-first replication merge core

## Decision

TermiRust uses a bounded multi-value register per opaque record key as the first local-first
replication primitive. Every candidate carries a version vector and either opaque sealed bytes or a
tombstone. Merge retains only causally maximal candidates. Concurrent maxima remain an explicit
conflict; no timestamp, device name, input order, or arbitrary winner silently discards one.

This follows the established use of vector clocks to distinguish causal ancestry from parallel
version histories in [Amazon's Dynamo paper](https://www.amazon.science/publications/dynamo-amazons-highly-available-key-value-store).
It also preserves concurrent same-property values for later review, consistent with the conflict
model documented by [Automerge](https://automerge.org/docs/reference/documents/conflicts/). The
transport-independent, user-owned boundary follows the offline and ownership goals described in the
[Local-First Software paper](https://www.inkandswitch.com/local-first/static/local-first.pdf).

## Algebra and deletion

For each key, the state is the maximal antichain of versioned candidates. Merge unions both sets,
deduplicates exact versions, rejects divergent values with the same exact clock, and removes every
candidate dominated by another clock. Stable structural sorting canonicalizes the result. The
committed fixture proves commutativity, associativity, and idempotence for valid bounded documents.

A causally later tombstone resolves to deletion. A tombstone concurrent with a live candidate stays
in the conflict set, so neither deletion nor resurrection happens silently. Conflict resolution is a
new candidate whose vector joins every reviewed candidate and increments an active author's counter;
that candidate then dominates the reviewed set.

## Replica authority

The merge policy lists at most 16 known replicas. Active replicas may author a new version. A revoked
replica may appear only as historical causality at or below its accepted counter cutoff, including a
zero cutoff for a device revoked before any accepted mutation. Unknown replicas and counters beyond
revocation fail closed.

This policy is semantic, not cryptographic. E12.2's authenticated-sealing contract now binds the
record, workspace, schema, clock, claimed author, key epoch, and operation against modification by
an actor without the workspace epoch key. It does not individually sign authors. Device-key
distribution, epoch authority, and any per-device signature contract must still be complete before
an untrusted transport is accepted.

## Bounds and privacy

JSON input is capped at 8 MiB before decode. A document has at most 4,096 unique keys, eight
candidates per key, 16 counters per vector, and 1 MiB per sealed payload, with an 8 MiB aggregate
payload cap. IDs are bounded opaque ASCII tokens. The core does not open payloads, read user state,
or call a filesystem, keyring, clock, random source, process, or network.

Errors are stable content-free categories. Debug formatting redacts IDs and payloads. Merge audit
events include only sequence, outcome, and candidate count; they contain no record key, replica,
vector, payload, path, endpoint, or user content.

## Deferred work

- device-key distribution, epoch rotation, and optional per-device signatures
- signed authority/key-epoch transitions and rollback protection
- atomic replica repository, compaction, tombstone retention, and crash recovery
- integration with the existing encrypted portable-bundle/shared-folder workflow
- peer or self-hosted transports, conflict review UI, and cross-device acceptance

D04 still controls final retention, backup, quota, and deletion defaults. D05 continues to prohibit
an operated account, cloud-sync service, or public deployment without explicit authority.
