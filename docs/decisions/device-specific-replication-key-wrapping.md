# Device-specific replication key wrapping

## Decision

TermiRust wraps each workspace epoch key independently to one recipient device using RFC 9180 HPKE
Auth mode with DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, and ChaCha20-Poly1305. The recipient opens a
package with its device private key and an independently trusted workspace-authority public key.
Base mode is not used because it provides no sender authentication.

[RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html) defines this KEM/KDF/AEAD composition and
Auth mode. Auth mode proves possession of the authority X25519 private key to the recipient; it is
not a transferable signature or non-repudiation mechanism. TermiRust uses the maintained pure-Rust
`hpke` crate with only `alloc` and `x25519` features. Version 0.13.0 is pinned because the current
workspace SSH stack pins a prerelease `rand_core` 0.10 contract incompatible with `hpke` 0.14's
stable 0.10.1 contract. The selected version passes the upstream RFC vectors, but its own crate
documentation says it has not received a dedicated audit. A dependency/security review remains a
release gate; the protocol bytes are versioned so the implementation can be replaced without
silently changing the contract.

Controller Noise keys and relay admission keys are deliberately not reused. Existing mobile
`encrypted_vault_key` values remain placeholders and are not accepted by this contract.

## Keys and entropy

Authority and device key roles use distinct Rust types. Each private wrapper owns 32 bytes, redacts
`Debug`, and zeroizes on drop. Public wrappers contain exactly one structurally valid 32-byte
X25519 encoding and redact raw bytes from `Debug`. X25519's mandatory all-zero shared-secret check
is enforced by HPKE during encapsulation/decapsulation; malformed or low-order inputs fail closed.

Production generation and wrapping first obtain exactly 32 bytes through fallible `getrandom`, then
feed those bytes into the pinned HPKE X25519 derivation. This preserves a typed randomness failure
instead of using HPKE's panic-on-OS-RNG-failure convenience path. The internal one-block RNG is
bounded to the exact pinned X25519 entropy request and wipes bytes as HPKE consumes them. Tests use
the same injected entropy boundary with synthetic fixed material.

## Wire and authenticated context

The schema-v1 key package is exactly 97 bytes and no larger than the 256-byte parser cap:

1. four-byte `TRKW` magic
2. two-byte package version
3. one-byte suite identifier
4. eight-byte nonzero epoch
5. 32-byte HPKE encapsulated key
6. two-byte ciphertext length
7. 48-byte ciphertext (32-byte epoch key plus 16-byte Poly1305 tag)

Separate canonical HPKE `info` and AAD domains each length-prefix and bind package version, suite,
epoch, workspace ID, recipient replica ID, trusted authority public key, and exact recipient public
key. The wire package therefore carries no workspace/device identifier while relabeling any caller
context, authority, recipient, epoch, encapsulated key, or ciphertext causes authentication to
fail before key release.

The caller must supply the exact expected epoch. This leaf does not infer freshness from an
untrusted package. Successfully opened key bytes move into E12.2's zeroizing `ReplicationEpochKey`;
all intermediate plaintext/key buffers are wiped, and failures expose stable content-free errors.

## Authority and lifecycle boundary

HPKE authenticates a package against a public key the caller already trusts. E12.4 now supplies the
storage-neutral authority state machine that enrolls exact device public keys, advances revisions
and epochs, and excludes revoked devices from fresh distributions. It still does not establish
native secure-store ownership or durable freshness. A revoked device can always retain keys it
learned before revocation, so revocation rotates to a fresh epoch and distributes it only to
remaining devices.

The following remain separate work:

- native secure-store ownership and durable authority enrollment
- atomic repository commit of authority revision and key-epoch transitions
- highest-seen epoch/revision rollback protection and historical-key retention
- repository persistence, shared-folder/network transport, retries, and recovery
- migration of placeholder mobile key metadata and user-facing device/conflict UI

D04 still controls final retention, backup, export, and deletion defaults. D05 continues to forbid
an operated account or cloud-sync service without explicit authority.
