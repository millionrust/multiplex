# OpenSSH Certificate Authentication

TermiRust accepts an existing OpenSSH user certificate and its matching private key as one
explicit authentication method. It does not issue certificates, manage certificate authorities,
or silently retry the private key when certificate authentication fails.

## Validation boundary

Before network authentication, the desktop client verifies that the certificate file is regular,
bounded to 64 KiB, parseable, a user certificate, currently valid, authorized for the requested
username when principals are present, and bound to the configured private key. OpenSSH on the
server remains authoritative for CA trust, revocation, source restrictions, and certificate
extensions.

All terminal, remote-exec, jump-host, and SFTP connections use the same adapter. Errors identify
the failed validation class without returning private-key paths, certificate paths, passphrases,
or key material. A restored workspace retains the certificate reference but never persists a
private-key passphrase.

## Compatibility

The profile fields are flat and defaulted so old state remains readable. `CertificateFile` is
imported from SSH config only when a supported `IdentityFile` signer is also present. Mobile vault
export rejects certificate profiles until the mobile schema can represent them, preventing a
credential downgrade.
