# SSH Key Generation And Public-Key Deployment

Status: accepted for the v1 desktop implementation

## Decision

TermiRust generates Ed25519 identities locally with operating-system entropy and writes an
OpenSSH private key plus its `.pub` peer. The user selects an absolute destination. Generation
uses exclusive staging files, filesystem links for no-overwrite publication, strict permissions,
directory synchronization, and inode-aware cleanup. Existing files, indirect parent directories,
and foreign-owned destination directories fail closed.

Generated identities join the existing Keychain, where the user can inspect the public key and
SHA-256 fingerprint. The ordinary state store contains the identity label and local path, never
private bytes or passphrases. Passphrase inputs are cleared when work starts and operation objects
redact secret paths and values from debug output.

Deployment is an explicit, reviewed operation against one saved Connection. The review names the
host, port, username, configured operation authentication, fixed destination, public key, and
fingerprint. It uses the existing SSH authentication, jump-chain, timeout, and TOFU/pinned
host-key stack without fallback or a trust bypass. Only the public key crosses this connection.

The remote destination is fixed to the authenticated user's canonical home:
`~/.ssh/authorized_keys`. TermiRust rejects unsafe owners, symlinks, and non-regular files; bounds
input size and line count; serializes its own writers with a recoverable lock; stages with mode
`0600`; fsyncs; and atomically replaces the file. Matching uses the decoded algorithm and key
blob, so comments and options do not affect idempotency. Removal deletes only exact matching key
lines and separately warns that another login route is not guaranteed.

An add is “verified” only after a separate fresh SFTP authentication using the generated private
key. A completed write followed by a rejected fresh login is reported as installed but not
verified. The acceptance suite additionally proves a fresh interactive terminal login. Audit
records are bounded to operation, result, fingerprint, saved host identity, endpoint, username,
and time.

## Limits

- Ed25519 is the only generated algorithm in v1.
- The implementation does not create CAs, certificates, FIDO/smartcard keys, or root keys.
- It does not change `sshd_config`, disable passwords, choose arbitrary remote paths, schedule
  rotation, delete keys automatically, or claim Windows secure-store integration.
- The lock coordinates TermiRust operations. A hostile process already running as the same remote
  user remains outside the SSH account trust boundary.
- A verification failure does not roll back the installed key because a second concurrent edit
  could make rollback destructive. The result explicitly says the key was installed and offers
  exact removal.

## Verification

Run `./scripts/verify-ssh-key-lifecycle.sh`. Live acceptance requires Docker and OpenSSH
`ssh-keygen`; the script fails rather than silently claiming those checks on an unavailable host.
