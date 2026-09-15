# Enrollment Custody Ownership

Status: shared Rust prerequisite implemented; not enabled in product enrollment or
native mobile custody. This does not close C07.

## Why a Receipt Is Needed

An enrollment create can commit to the credential store and still report failure.
Its prepared journal proves the intended reference, but an existing value at that
reference could instead be a collision. Comparing the epoch key alone is insufficient:
two enrollment attempts may legitimately receive the same workspace epoch key.

An independent random 32-byte `ReplicationCustodyOwner` associates one attempt with
its stored record. Generate it once, separately from the reference and epoch key,
and durably journal it before any create. Keep it in app-private metadata; redact it
from logs, exports, debug output and diagnostic bundles. It is association metadata,
not a replacement for platform custody encryption or local filesystem protections.

## Record Contract

Existing references and v1 secret records are unchanged. New writes remain opt-in.

| Offset | Length | v2 owned epoch record |
|---|---|---|
| 0 | 4 | Existing `TRSC` magic |
| 4 | 2 | Big-endian version 2 |
| 6 | 1 | Epoch-key role only |
| 7 | 8 | Big-endian nonzero key epoch |
| 15 | 32 | Epoch key bytes |
| 47 | 32 | Nonzero per-attempt ownership receipt |

The exact length is 79 bytes. Only the existing 47-byte v1 records and 79-byte v2
epoch records decode. Reject crossed version/size combinations, unsupported versions,
other roles, empty receipts, zero keys and wrong epochs. Legacy reads remain supported;
legacy records cannot pass ownership validation and are never upgraded in place.

`store_owned_epoch_key_at` uses create-only backend semantics. `load_owned_epoch_key`
reads and validates a snapshot of the key, role, epoch and receipt. Neither performs
recovery, mutation, adoption or deletion. Normal epoch reads support a valid v2 record
so a future successfully committed member can keep using its key.

## Required Integration Order

1. Extend Rust's native callback adapter and both platform stores to accept exactly
   the supported record lengths. Preserve buffer wiping, bounds, create-only semantics
   and errors. Test real Keystore/Keychain round trips before enabling v2 writes.
2. Add optional ownership and request-binding fields to the private enrollment journal.
   Preserve canonical legacy decoding. Bind the receipt to the exact pending request
   and authenticated enrollment context, not merely a caller-provided account name.
3. Under the existing product lock, persist the complete prepared intent before a
   single owned create. A failed create must never trigger an automatic retry or a
   replacement identity. Keep the journal even when its confirmation write fails.
4. Explicit recovery must validate pending identity, context, journal, absence of an
   unrelated transaction and receipt before any rollback decision. Validate all
   preconditions before mutation. A key read is not an atomic compare-and-delete;
   preserve exclusive product ownership and the backend's no-overwrite contract.
5. After an owned-key match, durably transition to a confirmed rollback state before
   exact deletion. Preserve that state on deletion failure. After a published profile,
   validate committed repository custody and never roll it back merely because the
   cleanup journal remains. Cover every publication boundary with fault injection.
6. Missing receipts on old prepared intents, mismatched receipts, inaccessible stores
   or malformed evidence must remain fail-closed. Do not fabricate ownership for old
   ambiguous records. They still need a separate reviewed preservation/recovery route.
7. Rebuild native artifacts and rerun Android/iOS process-restart and UI acceptance.
   Never call this complete from Rust-only tests or advertise automatic sync.

## Trust Boundary

The receipt distinguishes accidental collisions and uncertain writes when private
journals and native custody are trusted. It does not defeat an attacker able to rewrite
both stores. It does not prove global exclusive ownership, provide authority to revoke
devices, or permit deleting arbitrary user-selected credential references. Recovery
must validate the surrounding product transaction even after receipt validation.
