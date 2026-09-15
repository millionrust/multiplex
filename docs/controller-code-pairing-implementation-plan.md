# Controller reachability and code pairing: implementation plan

Status: Done; not yet run on an Android device. See [completion evidence](completion-evidence/controller-code-pairing.md).

## Goal

Pairing a phone becomes: turn on remote access, press **Pair phone**, pick the computer on the
phone (or type its Tailscale name), type the six-digit code. The desktop listens on every private
address it has, so there is no network to choose and no route to repair when an address changes.

Decisions taken with the product owner on 2026-09-15:

- On a LAN the phone finds the computer with Bonjour. Over Tailscale the user types the computer's
  MagicDNS name or 100.x address once; after pairing the phone keeps every address it learns.
- The QR code and code-comparison flow stay, under "Other ways to pair".
- The computer advertises itself with Bonjour whenever remote access is on, using an opaque host ID
  rather than the computer's name.

## Security position

- The listener binds each eligible private address (RFC 1918, 100.64.0.0/10, fc00::/7) on one
  port. Wildcard, loopback, link-local and public addresses stay refused. Unpaired peers can only
  run a pairing handshake, and only while pairing mode is open.
- A six-digit code has 20 bits of entropy. Mixing it into Noise as a pre-shared key would let an
  active attacker test all codes offline against a single recorded handshake, so the code is
  instead the password of a balanced PAKE (CPace over Ristretto255). An attacker gets one guess per
  pairing attempt; the Host allows three attempts per code, then discards it.
- The PAKE output (a uniformly random 32-byte key) is bound into the existing Noise XX pairing
  prologue. A peer without the key cannot complete the handshake, so the phone learns the Host
  static key authenticated by the code. Later connections keep the unchanged Noise IK path.
- The code is shown only on the desktop, is single-use, expires after five minutes, and is never
  logged, persisted in plaintext, or sent over the wire.
- These changes amend controller-security v1. The independent cryptographic review gate recorded in
  `docs/decisions/controller-security-v1.md` still applies and is not claimed complete.

## Phases

### P1: Listen on all private addresses

- `ControllerListenPolicy` becomes `enabled`, `port`, `discovery`. The old exact-route fields are
  still read from existing files and dropped on the next save.
- `bind_private_addresses` binds every eligible address on the persisted port. A generated port is
  replaced only when no address can use it.
- The runtime accepts from every bound address, binds addresses that appear and closes addresses
  that disappear, and reports the current set to the desktop. Losing all networks leaves the
  listener waiting instead of failing.
- Pairing offers carry every listening address; the QR envelope becomes schema 2 with `routes`.
- Desktop: the per-network list is replaced by an On/Off control and the list of addresses in use.

### P2: Bonjour advertisement

- Advertise `_termirust._tcp` on LAN interfaces while the listener runs, with TXT `v=1` and
  `id=<first 16 bytes of the host fingerprint, hex>`. VPN interfaces are not advertised.

### P3: Code pairing protocol

- `termirust-controller-security`: `cpace` module (CPace, Ristretto255, SHA-512), code pairing
  prologue binding, attempt accounting, ADR amendment, golden vectors, property tests.
- Listener: `PairingMode` offers (code, nonce, expiry, attempts), preface purpose `PairCode`,
  CPace exchange, then the existing XX registration and acknowledgement without SAS.

### P4: Desktop pairing mode UI

- Remote access On/Off, addresses in use (with the Tailscale MagicDNS name when the `tailscale`
  CLI reports one), **Pair phone** dialog with the code, a countdown and attempts left, and the
  QR flow under "Other ways to pair".

### P5: Bindings and mobile apps

- UniFFI: `code_pairing_start(code, device key, nonce)` session without SAS.
- iOS and Android: list computers found with Bonjour or enter an address, enter the code, name the
  phone. `PairedHostRecord` schema 2 stores every route; reconnect tries each route and refreshes
  them from Bonjour by host ID.

### P6: Docs, gates, evidence

- Update `docs/remote-terminals.md`, controller ADRs, verify scripts, and completion evidence.

## Verification

- Listener tests cover multi-address binding, port fallback, interface churn, and refusal of
  public and wildcard addresses.
- Security tests cover CPace vectors, wrong-code failure, attempt exhaustion, transcript binding,
  and that a recorded failed attempt gives no offline test for other codes.
- iOS builds and installs with Xcode 27.0 on the workstation. Android builds and passes its unit
  tests with NDK 27.1.12297006; see the completion evidence.
