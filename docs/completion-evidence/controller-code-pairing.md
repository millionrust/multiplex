# Controller Reachability and Code Pairing — Completion Evidence

Plan: [docs/controller-code-pairing-implementation-plan.md](../controller-code-pairing-implementation-plan.md).
Branch `dev`. Recorded 2026-09-15 on macOS (Darwin 27.0), rustc 1.97.1, Xcode 27.0, tmux 3.7c.

## Result by phase

| Phase | State | Commits |
| ----- | ----- | ------- |
| P1 Listen on every private address | Done | `47fdf6f` |
| P2 Bonjour advertisement | Done | `443bc84` |
| P3 Six-digit code pairing (CPace bound into Noise XX) | Done | `f1e33bd`, `84dcf50` |
| P4 Desktop pairing mode UI | Done | `f1e33bd`, `e1df71f`, `3f78199` |
| P5 Bindings and mobile apps | iOS done and installed on a device; Android written, not built | `1d77a18`, `f82899f`, `e47c140`, `416a1cf` |
| P6 Docs, gates, evidence | Done | `6fc32ca`, this document |

## What was verified, and how

### Listener and security crates

| Suite | Result |
| ----- | ------ |
| `cargo test -p termirust-controller-security` | 38 passed |
| `cargo test -p termirust-controller-listener` | 74 passed |
| `cargo test -p termirust-controller-bindings` | 8 passed |
| `cargo test -p termirust-tmux` | 39 passed |
| `scripts/verify/controller-security-vectors.sh --check` | vectors and checksums verified |

Named coverage:

- **Binding** (`tests/bind_policy.rs`): every private address binds on one port; a disabled policy
  binds nothing; wildcard, loopback, and public addresses never reach a socket; a computer with no
  private network waits instead of failing; a port taken on one address stays for the others; a
  generated port moves only when no address can use it; a fixed port is never replaced.
- **CPace** (`termirust-controller-security/src/cpace.rs`): generator, shares, secret, and ISK
  match the draft-irtf-cfrg-cpace Ristretto255 vectors; invalid points are rejected; matching
  codes agree and any difference disagrees; hostile shares are rejected; codes are exactly six
  uniform digits and redacted in `Debug`. Code pairing vectors live in
  `tests/vectors/controller-code-v1.json`; the Controller v1 offer, handshake, SAS, key, and frame
  vectors are unchanged.
- **Code pairing over TCP** (`tests/code_pairing_route.rs`): the right code pairs without a SAS
  and the phone learns the Host key; wrong codes spend attempts and the code closes after three.
  Writing this test found that the offer envelope refused session generation 0, which a new Host
  starts with (`84dcf50`, fixed on iOS and Android too).
- **Bonjour** (`src/discovery.rs`): only LAN addresses are announced, under an opaque name; a
  VPN-only computer announces nothing. The announcement was also observed with `dns-sd -B
  _termirust._tcp` on the workstation.

### Desktop

`cargo test -p termirust --bin termirust`: 629 passed, 73 failed, 4 ignored. All 73 failures need
the Docker SSH fixture, which cannot start while `DOCKER_HOST` points at a remote machine (72 report
`unable to start docker ssh fixture`; one needs the fixture's generated key file). The same 73 fail
without these changes. Pairing-specific tests cover pairing mode events, code grouping, the
Tailscale hint, and that the Remote Devices view stays inside a 1000 px window with a live code.

### iOS

Built with Xcode 27.0 for an iPhone 16 (iOS 27) and installed with `devicectl`. The controller
framework was rebuilt with the pinned `scripts/build/mobile-controller-bindings.sh --ios` (pins
moved from Xcode 26.6 to 27.0 in `416a1cf`); two rebuilds produced identical native libraries.

Pairing a phone end to end on a real network was not recorded here.

### Android

Not built. The Android SDK and NDK 27.0.12077973 live on a volume that was not mounted, so the
Kotlin changes were reviewed but not compiled, and `app/src/main/jniLibs` still hold controller
bindings from before code pairing. To finish:

```bash
scripts/build/mobile-controller-bindings.sh --all
scripts/sync/mobile-controller-bindings.sh --write
ANDROID_HOME=/Volumes/Footages/android ./apps/android/gradlew -p apps/android assembleDebug
```

## Open items

- Independent cryptographic review of the CPace composition, as required by
  `docs/decisions/controller-security-v1.md`. Not claimed.
- Android build, binding rebuild, and a device run.
- An end-to-end pairing run from a phone on LAN and over Tailscale, recorded as evidence.
