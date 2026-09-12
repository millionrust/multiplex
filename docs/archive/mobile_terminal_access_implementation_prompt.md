# TermiRust Mobile Terminal Access Implementation Prompt

> Historical planning document, archived after the September 2026 repository
> restructure. This is not an active implementation brief or completion report.
> Original paths and proposals below are preserved for reference; current mobile
> sources live in `apps/ios` and `apps/android`. Consult current engineering evidence
> before using these instructions.

## Role

You are a senior product-minded systems engineer designing and implementing TermiRust mobile access. Build this in disciplined phases with clean architecture, clean commits, and no unnecessary disruption to the existing desktop app.

Do not rush into a desktop relay. The durable session should live on the SSH target through tmux, and mobile should connect securely to the same host and attach to the same named session.

## Goal

Design and implement the mobile access path for TermiRust so iOS and Android users can control their operational terminal sessions from a phone.

Target architecture:

```text
Desktop TermiRust ─┐
                   ├─ SSH ─ target host ─ tmux named session
iOS TermiRust ─────┤
Android TermiRust ─┘
```

Optional later architecture for private networks and enterprise:

```text
Mobile/Desktop ─ HTTPS/WebSocket or overlay network ─ TermiRust Gateway ─ SSH ─ target host ─ tmux
```

The first mobile implementation should use direct SSH plus shared encrypted host configuration. Do not make the desktop app a required relay for mobile.

## Research Basis

Mature products converge on these patterns:

- Direct SSH is the simplest and strongest security boundary when the phone can reach the host.
- Tmux provides session continuity across desktop and mobile.
- Gateways are useful for private networks, audit, policy, and teams, but should be explicit infrastructure.
- Mobile secrets must use platform-backed secure storage.

References:

- https://github.com/tmux/tmux/wiki/Getting-Started
- https://man7.org/linux/man-pages/man1/tmux.1.html
- https://guacamole.apache.org/doc/1.5.0/gug/guacamole-architecture.html
- https://guacamole.apache.org/doc/gug/introduction.html
- https://tailscale.com/docs/features/tailscale-ssh
- https://tailscale.com/docs/features/tailscale-ssh/tailscale-ssh-console
- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/ssh/
- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/ssh/ssh-browser-rendering/
- https://goteleport.com/docs/reference/architecture/
- https://developer.apple.com/documentation/security/keychain-services
- https://developer.apple.com/documentation/security/storing-keys-in-the-keychain
- https://developer.android.com/privacy-and-security/cryptography
- https://mas.owasp.org/MASTG/

## Non-Negotiable Engineering Rules

- Keep the existing desktop code stable.
- Do not remove desktop features to make mobile easier.
- Do not store plaintext passwords, private keys, or vault secrets in JSON.
- Do not create a hidden remote-control server inside the desktop app for the first version.
- Do not expose raw SSH through a public unauthenticated endpoint.
- Keep code modular and platform boundaries clear.
- Use the existing TermiRust data model where practical.
- Version every shared schema.
- Keep commits small and meaningful.
- Commit without co-author trailers. Do not include `Co-authored-by`.
- Run formatters and tests before every commit.
- Write migration notes when schema changes.

## Recommended Product Strategy

Build mobile in this order:

1. Define shared host/vault schema.
2. Build read-only vault import on mobile.
3. Build direct SSH connection from mobile.
4. Attach to tmux persistent sessions.
5. Add encrypted sync and device pairing.
6. Add optional gateway only after direct mobile works.

Do not start with a gateway unless the client explicitly requires private-network access on day one.

## Repository And Folder Strategy

Before creating files, inspect the existing repo structure and decide whether mobile belongs in this repo or a companion repo.

If this repo will become a monorepo, use a clean structure like:

```text
apps/
  desktop/
  ios/
  android/
crates/
  termirust-core/
  termirust-vault/
  termirust-protocol/
docs/
  mobile/
```

If the current desktop app is not ready for a monorepo move, avoid moving desktop files in the first mobile phase. Instead, add only:

```text
docs/mobile/
crates/termirust-vault/
crates/termirust-protocol/
```

Do not move large desktop folders unless the user explicitly approves a repo restructuring commit.

## Shared Core Design

Create shared logic for:

- Host profile schema.
- Persistent tmux session settings.
- Encrypted vault export/import.
- Known-host records.
- Device records.
- Sync metadata.

Recommended Rust crates:

```text
crates/termirust-protocol
crates/termirust-vault
```

Keep platform-specific UI outside these crates.

The shared schema should include:

```text
schema_version
hosts
groups
tags
identities metadata
known_hosts
persistent_session settings
created_at
updated_at
device_id
```

Private keys and passwords must be encrypted or stored only in platform key stores. If private key export is supported, it must be encrypted with a user-controlled passphrase or device-pairing key.

## Mobile MVP Requirements

### iOS

Recommended stack:

- SwiftUI for UI.
- SwiftNIO SSH or a carefully maintained libssh2 wrapper for SSH.
- iOS Keychain for credentials and private-key material where applicable.
- CryptoKit or audited crypto library for local vault encryption.

Required flows:

- Import encrypted TermiRust vault.
- Store secrets in Keychain.
- List hosts.
- Connect to host over SSH.
- Attach to tmux persistent session if enabled.
- Render terminal.
- Send keyboard input.
- Handle resize/orientation changes.
- Display host-key verification and mismatch warnings.

### Android

Recommended stack:

- Kotlin.
- Jetpack Compose for UI.
- SSHJ or a carefully maintained libssh2 wrapper for SSH.
- Android Keystore-backed encryption for secrets.
- Encrypted local storage for vault metadata.

Required flows:

- Import encrypted TermiRust vault.
- Store secrets with Keystore-backed protection.
- List hosts.
- Connect to host over SSH.
- Attach to tmux persistent session if enabled.
- Render terminal.
- Send keyboard input.
- Handle resize/orientation changes.
- Display host-key verification and mismatch warnings.

## Terminal Rendering

For the first mobile prototype, prioritize correctness and usability over full desktop parity.

Required:

- VT escape sequence rendering.
- Scrollback.
- Copy selection.
- Paste confirmation for multiline paste.
- Font size controls.
- Dark/light appearance compatibility.
- Software keyboard accessory row for common terminal keys:
  - Esc
  - Tab
  - Ctrl
  - Alt
  - arrows
  - slash
  - pipe
  - dash

Do not hand-roll a weak terminal parser if a stable mobile terminal component or shared parser can be used. If reusing the existing Rust terminal state is practical through FFI later, evaluate it after the direct SSH MVP works.

## Security Requirements

Minimum release bar:

- iOS secrets use Keychain.
- Android secrets use Keystore-backed encryption.
- Require device lock before storing secrets.
- Biometric unlock may be convenience, not the only protection.
- Known-host pinning is supported.
- Host-key mismatch is a hard stop.
- Vault file never stores plaintext private keys or passwords.
- Logs must not include passwords, private keys, raw terminal output, or full environment variables.
- Support device revoke before team rollout.
- Encrypted sync must use per-device keys or a passphrase-derived key.

## Desktop Changes Needed For Mobile

Keep desktop changes focused:

- Ensure export/import schema is versioned.
- Include persistent tmux settings in export/import.
- Add a mobile-compatible encrypted vault export mode if the existing export is not suitable.
- Add docs explaining how mobile attaches to the same tmux session.

Do not add a desktop relay server in this phase.

## Optional Gateway Design

Add this only after direct mobile works or if the client confirms private-network access is mandatory.

Gateway responsibilities:

- OIDC or strong identity login.
- Device pairing.
- Device revocation.
- Optional SSH proxy.
- Audit events:
  - user
  - device
  - host
  - timestamp
  - duration
  - result
- Policy controls.
- Optional session recording.

Gateway must not store plaintext private keys by default. Prefer:

- Client-side encrypted vault.
- Short-lived access tokens.
- Device-specific wrapping keys.
- Customer-managed gateway deployment for sensitive teams.

## Implementation Plan

### Phase 1: Mobile Architecture Document

1. Create `docs/mobile/architecture.md`.
2. Document direct SSH plus tmux as the MVP architecture.
3. Document gateway as a later option.
4. Document security model and platform storage.
5. Run markdown lint if available.
6. Commit:

```bash
git add docs/mobile/architecture.md
git commit -m "Document mobile terminal access architecture"
```

Do not include co-author trailers.

### Phase 2: Shared Vault And Protocol Schema

1. Add shared schema crate or module without moving desktop code unnecessarily.
2. Include schema versions.
3. Include persistent session fields.
4. Add serialization/deserialization tests.
5. Add compatibility tests with existing exported desktop state.
6. Run:

```bash
cargo fmt
cargo check
cargo test
```

7. Commit:

```bash
git add Cargo.toml crates src tests
git commit -m "Add shared vault schema for mobile clients"
```

Do not include co-author trailers.

### Phase 3: Desktop Mobile Export

1. Add or adapt encrypted export for mobile import.
2. Keep old export behavior intact.
3. Add clear schema versioning.
4. Add tests for export/import round trip.
5. Run:

```bash
cargo fmt
cargo check
cargo test
```

6. Commit:

```bash
git add src crates tests
git commit -m "Add mobile-compatible encrypted vault export"
```

Do not include co-author trailers.

### Phase 4: iOS Prototype

1. Create the iOS app in the agreed folder or separate repo.
2. Implement vault import.
3. Store secrets in Keychain.
4. List imported hosts.
5. Connect to one SSH host.
6. Attach to the configured tmux session.
7. Implement basic terminal display and keyboard input.
8. Add host-key pinning UI.
9. Run iOS build and tests.
10. Commit:

```bash
git add apps/ios docs/mobile
git commit -m "Prototype iOS SSH tmux terminal access"
```

Do not include co-author trailers.

### Phase 5: Android Prototype

1. Create the Android app in the agreed folder or separate repo.
2. Implement vault import.
3. Store secrets using Android Keystore-backed protection.
4. List imported hosts.
5. Connect to one SSH host.
6. Attach to the configured tmux session.
7. Implement basic terminal display and keyboard input.
8. Add host-key pinning UI.
9. Run Android build and tests.
10. Commit:

```bash
git add apps/android docs/mobile
git commit -m "Prototype Android SSH tmux terminal access"
```

Do not include co-author trailers.

### Phase 6: Sync And Device Pairing

1. Design device identity.
2. Add pairing flow.
3. Add device-specific vault wrapping keys.
4. Add revoke metadata.
5. Add conflict handling.
6. Add tests for vault encryption and schema migration.
7. Commit:

```bash
git add apps crates docs/mobile
git commit -m "Add mobile vault pairing and device metadata"
```

Do not include co-author trailers.

### Phase 7: Gateway Prototype

Only start this phase after direct mobile is proven or client confirms it is required.

1. Create gateway architecture doc.
2. Choose deployment model.
3. Add OIDC login.
4. Add SSH proxy mode.
5. Add audit events.
6. Add mobile gateway connection mode.
7. Add security tests and threat model.
8. Commit:

```bash
git add gateway docs/mobile
git commit -m "Prototype optional mobile SSH gateway"
```

Do not include co-author trailers.

## Manual Verification

Verify the direct mobile MVP with:

1. A host reachable from desktop and phone.
2. Tmux installed on the host.
3. Desktop TermiRust opens a persistent session.
4. Mobile imports the same host profile.
5. Mobile connects to the same host.
6. Mobile attaches to the same tmux session.
7. Commands entered on desktop are visible from mobile where tmux semantics allow it.
8. Disconnecting mobile does not kill the session.
9. Reconnecting mobile resumes the session.
10. Host-key mismatch blocks connection.
11. Removing the mobile secret prevents reconnect.

## Acceptance Criteria

- Mobile architecture is documented.
- Shared schema is versioned.
- Persistent tmux session settings are included in mobile-compatible export.
- iOS prototype can import a vault, connect by SSH, and attach to tmux.
- Android prototype can import a vault, connect by SSH, and attach to tmux.
- Secrets are stored with platform-backed secure storage.
- Host-key pinning exists on mobile.
- Desktop remains stable.
- No unrelated desktop code is removed.
- Commits are clear, regular, and do not include `Co-authored-by`.

## Final Report Format

At the end, report:

- Architecture chosen.
- Files and folders added.
- Commits created.
- Tests and builds run.
- Manual verification results.
- Security limitations still open.
- Gateway status: not started, planned, or implemented.

Keep the report concise and brutally clear. Do not hide risks.
