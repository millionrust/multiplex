# ADR: Desktop Controller Host identity and device authority

- Status: Accepted for local desktop implementation
- Decision date: 2026-08-27
- Decision owner: TermiRust engineering
- Release status: No Controller route is enabled; D06 still gates remote exposure

## Context

Controller pairing requires one stable desktop identity and a durable authority record before any LAN, SSH, relay, iOS, or Android route can be trusted. The older mobile-vault approval records only protect offline encrypted imports and are not authenticated live Controller authority.

## Decision

The Host static X25519 private key exists only in the operating-system credential store under the `com.termirust.controller.identity` service. Versioned `controller-devices.json` metadata contains only the public key and fingerprint, identity generation, opaque secret reference, bounded pairing offers, public device records, capabilities, and revocation epochs. Writes use the shared atomic writer, compare-and-swap revisions, a process lock, private permissions, size limits, and symlink rejection.

Startup loads the recorded secret and derives its public key again. Missing, locked, denied, invalid, or mismatched private material disables authority and never creates a replacement around existing metadata. Explicit reset first persists `ResetRequired`, advances identity and revocation generations, consumes offers, and revokes devices. Only then may it delete the old credential and commit a new identity. An interrupted reset remains disabled instead of accepting either identity ambiguously.

Pairing composes the exact Controller-v1 state machine from the Controller security ADR over injected byte transport. The Host persists a device and consumes its offer before sending the final acknowledgement. A lost acknowledgement is an explicit uncertain state and reconciles by offer plus authenticated device public key without duplicating trust. Every request rechecks identity generation, device key/status, revocation epoch, session generation, deadline, and one closed capability. Revocation is committed before injected channels close.

Settings exposes the public fingerprint, route state, trusted device names/status/capabilities, rename, revoke, and type-to-confirm identity reset. `Add Controller` stays disabled until a later approved route adapter exists. This goal creates no listener, PTY bridge, mobile client, relay, account, or network discovery.

## Consequences

- JSON, exports, backups, sync, logs, filenames, QR data, and clipboard paths cannot contain the Host private key.
- Deleting the credential does not silently create a new identity; users must perform the destructive reset flow.
- Reset permanently invalidates every prior Controller and pending offer.
- Route goals can depend on one transport-neutral authority without changing trust semantics.
- Live remote control remains unavailable until a separately reviewed and approved route is implemented.
