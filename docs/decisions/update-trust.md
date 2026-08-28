# ADR: Update metadata trust

- Status: Accepted for offline metadata verification only
- Decision date: 2026-08-28
- Decision owner: TermiRust engineering
- Selected implementation: `tough 0.24.0`
- Release authority: none; package fetch, staging, execution, signing, and publication remain prohibited

## Context

TermiRust needs an authenticated update-metadata boundary before any platform updater can
download or execute a package. TLS alone does not establish update authority or prevent a
mirror from serving old metadata. This decision covers only an offline, platform-neutral
verifier and synthetic fixtures. It creates no network client, installer, updater UI,
production signing key, repository publisher, or bypass action.

The normative model is [The Update Framework specification 1.0.35](https://github.com/theupdateframework/specification/blob/master/tuf-spec.md),
latest stable as observed on 2026-08-28 and dated 2026-07-15 in the specification. The
client update order and rollback rules in sections 5.1 and 5.2 are mandatory. This ADR is
an implementation decision, not a claim that the selected library is a reference
implementation or independently audited.

## Decision

Use `tough = 0.24.0` behind the private types of `termirust-update-trust`. The dependency
is exact-pinned as `=0.24.0` with default features disabled. The crates.io archive SHA-256
and Cargo checksum are both:

`35b378d98765c2ae9cdc3e9963ea7e670da8cdd9ee39611b8d722083c7f1ac11`

The corresponding annotated tag resolves to commit
`98d8eb8b2ce63515d9b4981c938ef6453c5b5771` (observed 2026-08-28). The released source is
licensed MIT OR Apache-2.0. Any pin, feature, checksum, trust-state schema, or metadata
contract change requires this ADR and the attack fixtures to be reviewed again.

The adapter returns only `VerifiedTarget` metadata. It deliberately exposes no selected
library type and no API that fetches, stages, opens, launches, or executes target bytes.

## Candidate Review

| Candidate | Evidence observed 2026-08-28 | Decision |
|---|---|---|
| `tough 0.24.0` | Released 2026-07-10; exact crate checksum and tag commit available; top-level roles, root rotation, safe expiry, rollback datastore, consistent snapshots, thresholds, and recursive delegations exist in the pinned source; filesystem transport works without the `http` feature | Selected behind a narrow adapter |
| `tuf 0.3.0-beta9` | Official TUF organization implementation, but its own README labels the API beta/unstable; the latest non-yanked version reported by crates.io remains beta and enables Hyper by default | Rejected for this pin |
| Custom TUF/signature implementation | Would make TermiRust responsible for canonicalization, threshold, rotation, delegation, rollback, and expiry cryptography | Prohibited |

The `tough 0.24.0` crate-level documentation still says delegated roles are unsupported,
but the same pinned source contains recursive `load_delegations`, delegated threshold and
path checks, and delegation fixes through 0.22.0. TermiRust does not resolve that conflict
by assumption: `valid-delegated` proves an exact target behind a terminating delegated
role, while hostile threshold and delegation-bound tests fail closed. This discrepancy is
a documentation and maintenance risk to recheck on every upgrade.

## Security History

Versions before 0.20.0 had published path, rollback, duplicate-key, expiry-time, and role
validation fixes. Versions before 0.22.0 had a delegated signature-threshold bypass and
other delegation fixes. Version 0.23.0 introduced a signed-expiry canonicalization
regression; 0.24.0 fixes it in pull request 952. The selected version is therefore 0.24.0,
not a compatible range. `cargo deny check` remains mandatory because this dated review
does not predict future advisories.

## Trust Contract

The caller supplies an immutable embedded/bootstrap root and a local metadata directory.
The verifier applies these limits before cryptographic verification:

| Input | Limit |
|---|---:|
| Bootstrap root | 256 KiB |
| Each metadata role | 1 MiB |
| Delegated role files | 8 |
| Targets across roles | 10,000 |
| Metadata directory entries | 44 |
| Persisted TermiRust trust state | 16 KiB |

Metadata is checked in TUF dependency order by `RepositoryLoader` with
`ExpirationEnforcement::Safe` and explicit `Limits`. The adapter additionally parses each
bounded role with an injected clock before crypto, checks cancellation between bounded
operations, and requires root, timestamp, snapshot, and targets roles. Target selection is
an exact normalized path and then requires exact channel, platform, architecture, store
range, protocol range, rollout, length, and SHA-256 metadata. Unknown TermiRust custom
fields and unknown custom schemas fail closed.

The selected package has no public injected-clock API and writes role metadata to its
datastore while loading. TermiRust compensates by:

1. enforcing expiry against the injected clock before calling the library;
2. using safe library expiry as an independent current-clock check;
3. giving each attempt a fresh temporary library datastore; and
4. committing only TermiRust's compact monotonic state after the complete target and
   compatibility result verifies.

This means partial library writes are discarded. The accepted root, timestamp, snapshot,
and targets versions plus the latest observed clock are written through a private `0600`
temporary file, synchronized, atomically persisted, and followed by a parent-directory
sync. A lower role version is rollback, an equal timestamp version is replay, and a lower
clock is clock rollback. Every failure returns no target and leaves prior state unchanged.

The bootstrap root bytes themselves are not learned from the repository or replaced in
TermiRust state. A future production build must embed and version its reviewed bootstrap
root as an application asset. Passing an attacker-controlled bootstrap root violates this
contract and cannot be repaired by TUF metadata verification.

## Errors and Recovery

`TrustErrorCode` is closed and content-free: cancellation, resource limit, missing or
invalid metadata, invalid signature, tamper, expiry, replay, rollback, clock rollback,
missing/wrong/incompatible target, corrupt/newer state, and state I/O. There is no
ignore/continue representation. Corrupt, oversized, or newer persisted state remains
inspectable as bytes and is never silently reset. Recovery requires an explicit later
product workflow; this crate stays read-only on those failures.

## Dependency and Build Surface

Default features are disabled, and the adapter contains no HTTP or other network client.
The selected `tough` graph still includes `rustls`, `aws-lc-rs`, and native `aws-lc-sys`
for signature verification. Source inspection found no `unsafe` block in the `tough`
crate itself; the native cryptographic transitive surface remains governed by the lockfile,
SBOM, platform builds, and dependency policy. A future attempt to remove that native
surface requires a fresh candidate comparison rather than an unreviewed feature change.

The synthetic RSA private key under `tests/fixtures/update-tuf/source` comes from the
selected library's public test corpus. It is conspicuously test-only, signs no payload,
and is reachable only from an ignored integration-test generator. It must never be copied
into a release repository or production signing process.

## Executable Evidence

- `valid-v1` and `valid-v2` prove deterministic verification and monotonic advancement.
- `valid-delegated` proves exact terminating-delegation target selection.
- `root-rotation` proves a valid cross-signed rotation and rejects a mutated rotation.
- The attack matrix derives tampered, expired/frozen, replayed, rollback, missing-role,
  wrong-platform, incompatible, excessive-delegation, oversized-role, and 10,001-target
  cases from checked-in bases.
- The duplicate-signature fixture verifies threshold bypasses fail with the selected pin.
- File-state tests prove private permissions, atomic replacement, write-failure retention,
  and read-only corrupt/newer-state recovery.
- `update_metadata` runs arbitrary bounded metadata through the real preflight and TUF
  loader under libFuzzer/AddressSanitizer; the acceptance run is 10,000 cases.
- `MANIFEST.sha256` freezes every reviewed fixture byte. RSA-PSS fixture regeneration is
  intentionally not byte-deterministic, so regeneration always requires manifest review.

## Residual Risks and Release Gates

- The selected library and this adapter have not received an independent TermiRust audit.
- The library documentation/delegation mismatch requires explicit re-evaluation on update.
- One role may consume up to 1 MiB before JSON parsing; cancellation is bounded between
  role operations rather than interrupting a single parser call.
- This goal verifies target metadata only. Target-byte streaming, length/hash verification,
  authenticated transport, disk quotas, staging, installer privilege, downgrade approval,
  package signatures, and rollback belong to later gated goals.
- Production root/key custody, threshold ceremony, offline keys, timestamp automation,
  repository publication, compromise response, and emergency rollback require named
  release authority and cannot be inferred from these fixtures.

No platform updater is authorized by this ADR alone.

## Primary Sources

All sources were accessed 2026-08-28.

- [TUF specification 1.0.35](https://github.com/theupdateframework/specification/blob/master/tuf-spec.md)
- [`tough` 0.24.0 release](https://github.com/awslabs/tough/releases/tag/tough-v0.24.0), [pinned source](https://github.com/awslabs/tough/tree/98d8eb8b2ce63515d9b4981c938ef6453c5b5771), and [RepositoryLoader API](https://docs.rs/tough/0.24.0/tough/struct.RepositoryLoader.html)
- [`tough` security policy and advisories](https://github.com/awslabs/tough/security), [delegated threshold advisory](https://github.com/awslabs/tough/security/advisories/GHSA-8m7c-8m39-rv4x), and [0.24 expiry fix](https://github.com/awslabs/tough/pull/952)
- [`rust-tuf` project and beta warning](https://github.com/theupdateframework/rust-tuf) and [`tuf 0.3.0-beta9`](https://crates.io/crates/tuf/0.3.0-beta9)
