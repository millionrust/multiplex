# TermiRust Experiment: Persistent Tmux Sessions + Mobile Access

> Historical planning document, archived after the September 2026 repository
> restructure. This is not an active implementation brief or completion report.
> Original paths and proposals below are preserved for reference; current mobile
> sources live in `apps/ios` and `apps/android`. Consult current engineering evidence
> before using these instructions.

## Objective

Client feedback asks for two related capabilities:

1. Persistent terminals: closing and reopening TermiRust should not kill the working shell.
2. Mobile access: iOS and Android users should be able to access and control the same operational terminal environments from their phone.

The best solution is not to keep desktop PTYs alive inside TermiRust. The durable session should live on the target machine, and every client, desktop or mobile, should attach to that durable session through a secure SSH path.

## Research Summary

### Tmux Is The Correct Persistence Primitive

Tmux is a terminal multiplexer. Its server manages sessions, windows, and panes; clients can detach and later reattach. This gives exactly the persistence the client described: the shell keeps running after TermiRust disconnects.

The tmux command that maps best to TermiRust is:

```bash
tmux new-session -A -s <session-name>
```

`new-session -A` attaches to an existing named session if it exists, or creates it if not. That gives us a single idempotent "open my persistent shell" command.

Useful references:

- https://github.com/tmux/tmux/wiki/Getting-Started
- https://man7.org/linux/man-pages/man1/tmux.1.html
- https://github.com/tmux/tmux/wiki/Control-Mode

### Mobile Should Not Depend On The Desktop App

There are two possible mobile designs:

1. Phone connects directly to the SSH host and attaches to the same tmux session.
2. Phone remotely controls the desktop TermiRust process.

The second option is weaker for this client. It requires the desktop to remain online, reachable, authenticated, and acting as a remote shell relay. It also turns the desktop app into a high-risk server.

The better design is direct mobile SSH access with shared encrypted configuration. The durable state is on the remote host in tmux, not inside the desktop app.

### Mature Products Use Gateways Only When Needed

Apache Guacamole, Cloudflare Browser SSH, Tailscale SSH, and Teleport all point to the same broad pattern: if the client cannot directly reach a host, use an identity-aware gateway or private-network overlay. Do not expose raw SSH or ad-hoc web terminals publicly.

Useful references:

- https://guacamole.apache.org/doc/1.5.0/gug/guacamole-architecture.html
- https://guacamole.apache.org/doc/gug/introduction.html
- https://tailscale.com/docs/features/tailscale-ssh
- https://tailscale.com/docs/features/tailscale-ssh/tailscale-ssh-console
- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/ssh/
- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/ssh/ssh-browser-rendering/
- https://goteleport.com/docs/reference/architecture/

### Mobile Security Must Be Designed First

Mobile SSH access means storing or using credentials on phones. Secure storage, device lock, biometric unlock, and revocation are core requirements.

Use:

- iOS Keychain for secrets and private-key material where applicable.
- Android Keystore-backed encryption for local secret wrapping.
- OWASP MASVS/MASTG as the mobile security baseline.

Useful references:

- https://developer.apple.com/documentation/security/keychain-services
- https://developer.apple.com/documentation/security/storing-keys-in-the-keychain
- https://developer.android.com/privacy-and-security/cryptography
- https://mas.owasp.org/MASTG/
- https://mas.owasp.org/MASTG/tests/android/MASVS-STORAGE/MASTG-TEST-0001/

## Recommended Architecture

### Core Principle

TermiRust should become a multi-device SSH client whose shared session identity is:

```text
Host profile + tmux session name
```

The desktop app, iOS app, and Android app all connect to the host independently and attach to the same tmux session.

```text
Desktop TermiRust ─┐
                   ├─ SSH ─ target host ─ tmux session
iOS TermiRust ─────┤
Android TermiRust ─┘
```

For private infrastructure that phones cannot reach directly:

```text
Mobile/Desktop ─ HTTPS/WebSocket or overlay network ─ TermiRust Gateway ─ SSH ─ target host ─ tmux
```

The gateway should be optional and designed as a separate server component, not hidden inside the desktop app.

## Desktop MVP: Persistent Session Mode

Add per-host settings:

- `persistent_session_enabled: bool`
- `persistent_session_name: Option<String>`
- `persistent_session_strategy: Tmux`
- `persistent_session_detach_others: bool`
- `persistent_session_install_hint_shown: bool`

Default session name:

```text
tr-<stable-profile-id-or-short-hash>
```

Connection command:

```bash
exec tmux new-session -A -s '<safe-session-name>'
```

If "detach other clients" is enabled:

```bash
exec tmux new-session -A -D -s '<safe-session-name>'
```

Safer bootstrap command:

```bash
if command -v tmux >/dev/null 2>&1; then
  if tmux has-session -t '<session>' 2>/dev/null; then
    exec tmux attach-session -t '<session>'
  else
    exec tmux new-session -s '<session>' -c '<startup-dir>'
  fi
else
  printf 'TermiRust persistent sessions require tmux on this host.\n' >&2
  exec "${SHELL:-/bin/sh}"
fi
```

For startup commands that should run only once, create the session detached with the command and then attach:

```bash
tmux new-session -d -s '<session>' -c '<startup-dir>' '<startup-command-or-shell>'
exec tmux attach-session -t '<session>'
```

Recommended behavior:

- Start a normal SSH shell.
- If persistent mode is enabled, immediately exec into tmux.
- Environment variables should be available before tmux starts.
- Startup directory/startup command should run only when the tmux session is created, not every time a user reattaches.
- If tmux is missing, show a clear error with install guidance and offer fallback to a normal shell.
- Closing a TermiRust tab should close the SSH client, not kill tmux.
- Killing the tmux session should require an explicit `Kill Persistent Session` action.

## Desktop UX

Host editor:

- Add `Persistent Session` toggle.
- Add optional `Session Name` field.
- Add `Detach other clients when connecting` advanced toggle.
- Show detected tmux status after first connect: `tmux available`, `missing`, or `unknown`.

Workspace:

- Show badge: `tmux`.
- Add actions:
  - `Detach`
  - `List persistent sessions`
  - `Kill persistent session`
  - `Rename persistent session`

## Mobile MVP: Direct SSH + Tmux Attach

Build mobile around these flows:

1. User signs in or imports an encrypted vault.
2. Mobile downloads/decrypts host profiles and identities.
3. User taps a host.
4. Mobile opens SSH directly.
5. If host has persistent mode, mobile attaches to the same tmux session.

Recommended mobile stack:

- iOS: SwiftUI UI, SwiftNIO SSH or libssh2 wrapper for SSH, Keychain for secrets.
- Android: Kotlin UI, SSHJ or libssh2 wrapper for SSH, Android Keystore-backed encrypted storage.
- Shared protocol/data model: reuse TermiRust's JSON bundle schema, versioned and encrypted.

Do not attempt to share live terminal bytes from desktop to phone in the first mobile version.

## Optional Gateway: Team/Enterprise Mode

For teams, NAT, private networks, audit, and revocation, add a separate TermiRust Gateway later.

Gateway responsibilities:

- Identity-aware login.
- Device pairing and revocation.
- Host/vault sync.
- Optional SSH proxy for mobile clients.
- Audit events: who connected, host, timestamp, duration.
- Optional terminal session recording for enterprise plans.

Gateway should not store plaintext private keys by default. Prefer:

- Client-side encrypted vault.
- Device-specific wrapping keys.
- Short-lived access tokens.
- Optional customer-managed gateway for sensitive teams.

## Architecture Options

### Option A: Tmux-Only Desktop Persistence

Pros:

- Fastest.
- Low risk.
- Fits current codebase.
- Solves "close and reopen app, terminal continues".

Cons:

- Does not solve phone access by itself.
- Requires tmux on target hosts.

Verdict: Do first.

### Option B: Mobile Direct SSH + Shared Encrypted Vault

Pros:

- Best MVP for mobile.
- Desktop does not need to be online.
- Security model is familiar: SSH remains SSH.
- Tmux gives shared session continuity.

Cons:

- Mobile must handle SSH keys/passwords safely.
- Phones must reach the target network.
- Key sync/revocation needs careful design.

Verdict: Best mobile MVP.

### Option C: Desktop Relay

Pros:

- Phone can access hosts reachable only from the desktop.
- Could demo quickly on one controlled network.

Cons:

- Desktop must stay online.
- Hard NAT/firewall story.
- Makes desktop a remote shell server.
- High security and support burden.

Verdict: Avoid for the first paid client build.

### Option D: Hosted Gateway / SSH Broker

Pros:

- Best enterprise/team experience.
- Central policy, audit, revocation, and mobile access.
- Works when phones cannot reach private hosts directly.

Cons:

- Requires backend operations.
- Higher security burden.
- Larger scope.

Verdict: Phase 2 after tmux + direct mobile MVP.

## Proposed Phases

### Phase 1: Tmux Persistence Experiment

Goal: prove persistent desktop sessions.

Deliverables:

- Add tmux fields to `HostProfile`, `DraftProfile`, `ConnectRequest`, and `RestorableConnection`.
- Generate a safe tmux session name.
- Add shell bootstrap generation for tmux.
- Connect saved hosts into persistent tmux sessions.
- Preserve existing startup directory, environment, and startup command behavior.
- Add tests for:
  - tmux command generation
  - attach existing vs create new behavior
  - restored workspace reconnects into same tmux session
  - tmux missing fallback

### Phase 2: Desktop UX Polish

Goal: make persistence understandable to users.

Deliverables:

- Host editor toggle and session name field.
- `tmux` badge on panes/workspaces.
- Explicit kill/detach/list actions.
- Better error copy when tmux is missing.

### Phase 3: Mobile Architecture Prototype

Goal: validate mobile feasibility without building a full product yet.

Deliverables:

- Small iOS proof of concept that connects to one SSH host and attaches tmux.
- Small Android proof of concept that connects to one SSH host and attaches tmux.
- Import encrypted TermiRust bundle manually.
- Store credentials using platform secure storage.

### Phase 4: Sync And Pairing

Goal: make multi-device use practical.

Deliverables:

- Encrypted vault sync format with explicit schema versions.
- Device pairing flow.
- Device-specific vault wrapping keys.
- Per-device revoke.
- Conflict handling for host/profile edits.

### Phase 5: Gateway

Goal: support teams and private networks.

Deliverables:

- Optional self-hosted gateway.
- OIDC login.
- SSH proxy.
- Audit trail.
- Policy controls.
- Mobile gateway mode.

## Security Requirements

Minimum requirements before real client deployment:

- Never store plaintext passwords or keys in app JSON.
- Use iOS Keychain and Android Keystore-backed storage.
- Require device lock before enabling mobile secret storage.
- Support biometric unlock as convenience, not sole security.
- Support known-host pinning on mobile too.
- Display host-key mismatch as a hard stop.
- Support remote device revoke before team rollout.
- Avoid copying terminal output into cloud logs unless explicitly enabled.
- Make gateway mode opt-in and auditable.

## Current Codebase Fit

This repo already has many pieces needed for Phase 1:

- `HostProfile`, `DraftProfile`, `ConnectRequest`, and `RestorableConnection` can carry tmux settings.
- `startup_bytes_for_request` already generates remote shell bootstrap commands.
- SSH panes already reconnect and restore workspaces.
- Session logs already persist connection history.
- Portable/encrypted export exists and can become the basis for mobile vault import.

Likely files touched for Phase 1:

- `src/models.rs`
- `src/ui/shell.rs`
- `src/ui/app/editor.rs`
- `src/ui/app/mod.rs`
- `src/ui/app/workspace.rs`
- `src/storage.rs` tests if persisted schema migration behavior changes

## Recommended Client-Facing Position

Tell the client:

> We will implement persistent terminals using tmux on the target machines, so sessions survive app restarts and network drops. Then the mobile app will connect securely to the same hosts and attach to those same tmux sessions. This avoids depending on the desktop app being online and keeps SSH as the security boundary. For private networks, we can later add an optional identity-aware gateway for team access, audit, and revocation.

This is the strongest architecture because it solves the immediate need quickly while leaving a clean path to paid team/mobile features.
