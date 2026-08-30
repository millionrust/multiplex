# SSH access policy v1

Date: 2026-08-30
Status: accepted for E07 implementation

## Decision

TermiRust distinguishes five SSH authentication methods: password, private key, OpenSSH user
certificate, local agent, and security key. Agent forwarding is a separate delegation policy,
defaults to disabled, and requires confirmation for every connection. Selecting local-agent
authentication never enables forwarding.

The pure contract in `termirust-domain::ssh_access` contains no password, private key,
certificate body, filesystem path, provider handle, socket path, hostname, username, or
diagnostic text. Callers provide only material-presence flags and typed runtime availability.
Validation must pass before any socket is opened or credential is read.

OpenSSH certificate authentication requires a certificate plus an explicit signer source.
The signer may be a private key, local agent, or security-key provider. The corresponding
runtime capability and material must be available. A certificate is not treated as a private
key and a signer configured for any non-certificate method is rejected.

Runtime support uses three states: available, provider unavailable, and unsupported. Product
surfaces must preserve this distinction. They may not relabel a missing agent or hardware
provider as bad credentials and may not silently fall back to another method.

## Existing ownership

- `HostProfile`, `DraftProfile`, `ConnectRequest`, `RestorableConnection`, and jump-host
  records currently preserve password/private-key configuration.
- `src/ssh.rs` and `src/sftp.rs` contain duplicated password/private-key authentication
  branches. E07.2 must extract one authentication adapter used by both before adding
  certificate support.
- SSH config import currently understands the first `IdentityFile` and `ProxyJump`, rejects
  `ProxyCommand`, and ignores `CertificateFile`, `IdentityAgent`, `IdentitiesOnly`, and
  `ForwardAgent`. Later leaves must extend this parser without changing existing imported
  profiles silently.
- The editor's “SSH.id, Key, Certificate, FIDO2” row and “Add Telnet” row are presentation
  placeholders, not working capabilities. They must not be used as evidence of support.

## Security rationale

OpenSSH documents certificates as a public certificate paired with a separate signing
identity. OpenSSH also warns that a remote user able to access a forwarded agent socket can
perform authentication operations with loaded keys, even though private key material is not
exported. Destination constraints improve this only when every participating OpenSSH
component cooperates. TermiRust therefore does not offer a persistent “always forward” policy
in v1.

Primary references:

- OpenBSD `ssh_config(5)`: https://man.openbsd.org/OpenBSD-current/man/ssh_config
- OpenBSD `ssh-add(1)`: https://man.openbsd.org/ssh-add.1
- OpenBSD `ssh-keygen(1)`: https://man.openbsd.org/ssh-keygen.1
- Mosh architecture and limitations: https://mosh.org/

## Deferred runtime work

- E07.2: stored model migration, common authentication adapter, OpenSSH certificate parsing,
  key/certificate binding, target and jump-host authentication, SFTP parity, and Docker proof.
- E07.3: local agent protocol, exact identity selection, forwarded-channel ownership,
  cancellation, destination-constraint reporting, confirmation UX, and hostile socket tests.
- E07.4: key generation, secure permissions/storage, public-key deployment, and recovery.
- E07.5 onward: Mosh, weak/local transport decisions, proxy/forwarding depth, and SFTP transfer
  management remain separate because their trust and lifecycle models differ.
