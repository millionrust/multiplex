# Replication secret custody

## Decision

TermiRust stores replication authority private keys, device private keys, and symmetric epoch keys
behind opaque 256-bit random references. The shared replication-security crate owns the typed,
versioned encoding and validation boundary. A backend sees only an opaque reference and a
zeroizing fixed-size secret envelope; application metadata and future replica repositories hold
references, never raw key material.

The optional `os-keyring` adapter uses `keyring` 3.6.3 binary `set_secret` and `get_secret` APIs.
On macOS and iOS this selects Apple Keychain, on Windows it selects Credential Manager, and on
Linux it selects the persistent native/Secret Service combination already used by TermiRust. The
adapter is not enabled by default in the pure security crate. Unsupported targets return
`Unavailable`; they must never inherit `keyring`'s testing mock as a production secure store.

[Apple Keychain Services](https://developer.apple.com/documentation/security/keychain-services)
is intended for small secrets, including cryptographic keys. Apple also documents per-item
accessibility conditions, including device-lock and user-presence choices. This leaf does not
choose biometric, unlock, synchronizable, or backup policy because those are platform product and
D04 decisions.

## Typed envelope and references

The schema-v1 secret envelope is exactly 47 bytes:

1. four-byte `TRSC` magic
2. two-byte schema version
3. one-byte role (`authority`, `device`, or `epoch`)
4. eight-byte key epoch (zero for authority/device; nonzero for epoch keys)
5. exactly 32 bytes of validated private or symmetric key material

References separately bind the same role and optional epoch to 32 random bytes. Their platform
account form is bounded and contains no workspace, device, host, user, path, or key bytes. Debug
output redacts the identifier, and stable errors contain no reference or platform error details.
The identifier is not itself a cryptographic secret, but it links public metadata to a secret item
and therefore remains excluded from TermiRust logs and diagnostics.

Typed vault methods reject cross-role and cross-epoch access before backend retrieval, then verify
the stored envelope again. A current epoch key is never substituted for a missing historical key.
Temporary private-key copies, encoded envelopes, and loaded backend buffers zeroize on drop. As
with all process-memory zeroization, this reduces residual copies but is not a claim against a
compromised process, swap, crash dumps, platform internals, or forensic recovery.

## Historical key index

Opening an E12.2 record requires the exact key epoch carried in its authenticated envelope. The
historical index is an ordered, contiguous map from epoch to opaque reference. It has a hard safety
cap of 64 entries, but no default retention setting: every caller must explicitly provide a limit
between one and 64. Restored entries may begin at any epoch after prior retirement, but gaps,
duplicates, wrong-role references, and over-limit state fail closed.

Appending requires the exact monotonic successor. If the explicit bound is exceeded, the index
returns the oldest references as a deterministic retirement plan. It does not delete them. A
future atomic repository must first commit the new public authority/repository state and retained
index, then process retirement with a crash-recoverable journal. This ordering avoids deleting the
only usable key before public metadata is durable.

[NIST SP 800-57 Part 1 Rev. 5](https://csrc.nist.gov/pubs/sp/800/57/pt1/r5/final) treats key
inventory, states, backup, archive, recovery, and destruction as explicit key-management policy.
The index supplies bounded inventory mechanics; D04 still decides retention duration/count,
backup/export inclusion, recovery, and deletion language. Removing a credential-store item is not
described as secure erasure on SSDs, snapshots, backups, or synchronized platform keychains.

## Android boundary

[Android Keystore](https://developer.android.com/privacy-and-security/keystore) is designed so its
keys can remain non-exportable and operations occur through the system provider, optionally in
secure hardware. Returning such a key as a Rust byte array would discard that property. The
portable byte-store adapter therefore does not claim Android support.

A later native Android adapter should generate a non-exportable Keystore wrapping key and use an
approved authenticated-encryption mode to wrap the 47-byte TermiRust envelope into app-private
storage, with alias/version binding, lock/authentication/error mapping, rotation, backup policy,
and device acceptance tests. It must expose the same missing/locked/invalid/unavailable semantics
to shared code without exporting the Keystore key. That implementation and its release claim are
outside this leaf.

## Failure and concurrency boundary

Backends distinguish missing, access denied or locked, invalid, collision, and unavailable. The
portable `keyring` API does not reliably distinguish a locked store from permission denial, so the
contract intentionally combines those recovery states rather than guessing. Platform details are
not embedded in errors.

The OS adapter checks for an existing item before creating one. That check and write are not a
cross-process transaction in the portable `keyring` API. References use independent 256-bit
randomness, and a future repository must serialize writers under its own ownership lock. The
adapter makes no standalone compare-and-swap or multi-item atomicity claim.

## Deferred boundary

This decision does not integrate E12.4 transitions, persist reference metadata, choose retention,
delete retired material, define backup/export/recovery, implement Android Keystore, add UI, or
start transport/background work. E12.6 owns atomic replica persistence and crash ordering. D04
continues to own final retention, backup, export, and deletion guarantees; D05 continues to
prohibit an operated account or cloud service without explicit approval.
