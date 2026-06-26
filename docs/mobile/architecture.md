# TermiRust Mobile Terminal Access Architecture

## Decision

TermiRust mobile access starts with direct SSH from each client to the target
host. Desktop, iOS, and Android all attach to the same named tmux session on the
target host.

```text
Desktop TermiRust --\
iOS TermiRust -------+-- SSH -- target host -- tmux named session
Android TermiRust --/
```

The desktop app is not a relay in the first mobile release. A gateway can be
added later for private networks, enterprise policy, audit, and device
management, but it must be explicit infrastructure rather than a hidden desktop
server.

The current companion mobile app roots are:

- `/Users/jacob/Projects/terminal_app/terminal_swift`
- `/Users/jacob/Projects/terminal_app/terminal_kotlin`

Desktop source remains in the current repository for this phase. Shared schema
and protocol code should be added to the desktop repo first, then consumed by
the companion apps through a stable exported schema or generated bindings.

## MVP Data Flow

1. Desktop exports a mobile-compatible encrypted vault.
2. User imports the vault on iOS or Android.
3. Mobile stores secrets in platform-backed secure storage.
4. Mobile lists imported hosts.
5. Mobile opens SSH directly to the target host.
6. Mobile verifies the host key before authentication.
7. If persistent session is enabled, mobile runs the same tmux attach/create
   bootstrap as desktop.
8. Terminal input/output stays between the mobile app and the SSH target.

Tmux is the continuity layer. Closing desktop, closing mobile, losing network,
or reconnecting should detach the SSH client but leave the tmux session alive.

## Shared Schema

The mobile vault schema must be versioned from day one.

Minimum shared records:

- `schema_version`
- `export_id`
- `created_at`
- `updated_at`
- `source_device_id`
- `hosts`
- `groups`
- `tags`
- `identity_metadata`
- `known_hosts`
- `persistent_session`
- `sync_metadata`
- `device_records`
- `device_keys`

Host records must include enough information for direct SSH:

- display label
- host/address
- port
- username
- authentication metadata
- jump-host metadata, if supported in mobile MVP
- startup directory
- startup command
- persistent tmux enabled flag
- persistent tmux session name
- detach-others flag
- known-host pin reference

Secrets must not be stored as plaintext JSON. Passwords, private keys, key
passphrases, vault wrapping keys, and device pairing keys must be encrypted or
stored only in the platform secure store.

Device records use `device_id` as the stable identifier. Desktop mobile exports
include the exporting desktop as an active device record with `platform:
desktop`. Mobile clients must preserve and decode the `devices` array even when
they do not yet expose a device-management UI. A device is considered revoked
when its matching record has `revoked_at_millis` set.

The current encrypted mobile vault is protected by a user passphrase. The
schema also includes `device_keys` for the next pairing phase: each record maps
one active `device_id` to an encrypted vault key using an explicit
`wrapping_algorithm`. Mobile clients must preserve unknown or unused device-key
records so paired-device sync can be added without replacing the vault format.
If a device is revoked, its matching device-key record must not be used.

## Platform Security

### iOS

- Store passwords, private-key material, vault wrapping keys, and device keys in
  Keychain.
- Prefer Keychain accessibility classes that require the device to be unlocked.
- Biometric unlock can be a convenience layer, but device passcode protection is
  the security base.
- Use CryptoKit or another audited primitive for local vault encryption.
- Host-key mismatch is a hard stop.

### Android

- Use Android Keystore-backed keys for local secret protection.
- Store encrypted vault metadata outside the Keystore, but only ciphertext and
  non-secret metadata may live there.
- Prefer AES-GCM for local encrypted blobs when the app controls the format.
- Require device lock before saving imported secrets.
- Host-key mismatch is a hard stop.

## Mobile App Stacks

### iOS

- UI: SwiftUI.
- SSH: SwiftNIO SSH or a maintained libssh2 wrapper.
- Secrets: Keychain.
- Local crypto: CryptoKit where possible.
- Terminal rendering: use a proven terminal component/parser for MVP; evaluate
  sharing Rust terminal logic through FFI only after direct SSH works.

### Android

- UI: Kotlin with Jetpack Compose.
- SSH: SSHJ or a maintained libssh2 wrapper.
- Secrets: Android Keystore-backed encryption.
- Local crypto: Android cryptography APIs with recommended algorithms.
- Terminal rendering: use a proven terminal component/parser for MVP; evaluate
  sharing Rust terminal logic later.

## Terminal UX Requirements

The first mobile terminal should prioritize reliable operations over full
desktop parity.

Required MVP behavior:

- VT escape rendering.
- Scrollback.
- Copy selection.
- Paste with multiline confirmation.
- Font size controls.
- Dark/light appearance support.
- Orientation and resize handling.
- Keyboard accessory row with Esc, Tab, Ctrl, Alt, arrows, slash, pipe, and dash.

Logs must not contain passwords, private keys, raw terminal output, or full
environment variables.

## Host-Key Verification

Known-host pinning must be part of the mobile MVP:

- First connection shows the host-key fingerprint and requires user approval.
- Approved keys are stored as known-host records.
- Mismatch blocks the connection and shows a clear warning.
- Importing a desktop vault may include known-host pins, but mobile must still
  enforce mismatch blocking locally.

## Persistent Tmux Behavior

Mobile should use the same semantics as desktop:

- If persistent mode is disabled, open a normal SSH shell.
- If persistent mode is enabled, attach to the configured tmux session.
- If the tmux session does not exist, create it.
- Startup directory and startup command apply only when the tmux session is
  first created.
- If tmux is missing, show install guidance and open a normal shell.
- Detach-others must be explicit because it can disconnect another client from
  the same tmux session.

## Optional Gateway

Gateway mode is deferred until direct mobile access works or the client confirms
private-network access is mandatory for day one.

Future gateway shape:

```text
Mobile/Desktop -- HTTPS/WebSocket or overlay network -- TermiRust Gateway -- SSH -- target host -- tmux
```

Gateway responsibilities:

- OIDC or equivalent strong identity.
- Device pairing and revocation.
- Optional SSH proxy mode.
- Policy controls.
- Audit events for user, device, host, timestamp, duration, and result.
- Optional session recording.

The gateway must not store plaintext private keys by default. Prefer
client-side encrypted vaults, short-lived tokens, device-specific wrapping keys,
and customer-managed deployments for sensitive teams.

## References

- tmux: https://github.com/tmux/tmux/wiki/Getting-Started
- tmux manual: https://man7.org/linux/man-pages/man1/tmux.1.html
- Apache Guacamole architecture: https://guacamole.apache.org/doc/gug/guacamole-architecture.html
- Tailscale SSH: https://tailscale.com/docs/features/tailscale-ssh
- Apple Keychain Services: https://developer.apple.com/documentation/security/keychain-services
- Android cryptography: https://developer.android.com/privacy-and-security/cryptography
- OWASP MAS cryptography guidance: https://mas.owasp.org/MASTG/0x04g-Testing-Cryptography/
