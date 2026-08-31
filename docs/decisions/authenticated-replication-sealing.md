# Authenticated replication record sealing

## Decision

TermiRust seals each replicated put and tombstone with AES-256-GCM-SIV. A nonzero workspace key
epoch supplies 32 secret bytes; HKDF-SHA-256 derives a separate AEAD key for each workspace/record
pair. The complete binary envelope fits inside the replication domain's existing 1 MiB opaque
sealed-payload limit.

[RFC 8452](https://www.rfc-editor.org/rfc/rfc8452.html) defines AES-GCM-SIV as nonce
misuse-resistant authenticated encryption, requires a 96-bit nonce, and recommends random nonces
where uniqueness cannot be statefully guaranteed. That matches local-first writes from several
devices better than a shared persisted nonce counter. Random nonces are still mandatory; misuse
resistance is defense in depth, not permission to reuse them.

[RFC 5869](https://www.rfc-editor.org/rfc/rfc5869.html) defines HKDF's extract-then-expand
construction and application-specific `info`. TermiRust uses a fixed versioned salt and canonical
workspace/record/epoch `info`, keeping record keys independent and preventing this epoch key from
being reused directly as an AEAD key in another protocol. The implementation uses the existing
RustCrypto `aes-gcm-siv`, `hkdf`, and `sha2` dependency families already present in the locked
workspace.

## Envelope and authenticated context

The schema-v1 binary envelope is big-endian and contains:

1. four-byte `TRRS` magic
2. two-byte envelope version
3. one-byte cipher-suite identifier
4. eight-byte key epoch
5. twelve-byte nonce
6. four-byte ciphertext length
7. ciphertext followed by its 16-byte authentication tag

Canonical additional authenticated data length-prefixes and binds the TermiRust domain, envelope
version, cipher suite, key epoch, operation kind, workspace ID, collection and record IDs, author,
and every deterministically sorted version-vector replica/counter pair. The nonce is an intrinsic
AEAD input, ciphertext/tag are verified by the cipher, and the parser rejects altered magic,
version, suite, length, or bounds before opening.

Put bodies are nonempty and bounded so the full envelope is at most 1 MiB. Deletes encrypt an empty
body and therefore carry a 16-byte authentication tag instead of being unauthenticated bare enum
variants. E12.1's causal merge and tombstone ordering do not change.

## Secret and failure handling

Workspace epoch keys and successfully opened payloads redact `Debug` and implement
`ZeroizeOnDrop`. Per-record derived keys and temporary canonical metadata buffers are zeroizing.
Any decrypted buffer rejected by semantic checks is explicitly wiped before return. AEAD failures
collapse to one content-free authentication error; malformed wire data, unsupported versions,
wrong epochs, invalid contexts, unavailable randomness, and bounds have stable typed errors.

Opening requires the exact envelope epoch. A later authority layer may retain prior epochs for
historical reads, but sealing with an old epoch and deciding which epochs remain acceptable are not
implicit in this crate.

## Authority limitation

AEAD proves that a record came from an actor holding the workspace epoch key and that its claimed
author metadata was not changed afterward. It does not cryptographically prove which individual
device authored the record because authorized devices share epoch material. E12.3 adds
device-specific authenticated wrapping for epoch distribution; enrollment, epoch
rotation/revocation, rollback protection, and any individually attributable signature contract are
separate work and must be complete before accepting an untrusted transport.

Controller Noise static keys are not reused for replication. Existing mobile `encrypted_vault_key`
strings remain placeholder metadata and are not treated as cryptographic wrapping.

## Deferred work

- device enrollment and secure-store ownership for the E12.3 wrapping contract
- epoch rotation, revocation, rollback prevention, and historical-key retention policy
- optional per-device signatures if individual attribution is required
- atomic repository, shared-folder transport, crash recovery, and compaction
- conflict/recovery UI and cross-device acceptance

D04 still controls final key/content retention, backup, quota, and deletion defaults. D05 continues
to prohibit an operated account, cloud-sync service, or public deployment without explicit
authority.
