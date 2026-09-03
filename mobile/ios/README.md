# TermiRust Mobile for iOS and iPadOS

This folder contains the unified native TermiRust mobile application.

## Architecture

- **Connections** are saved direct-SSH destinations. Their SSH credentials are
  device-local, known-host pins are mandatory, and optional remote tmux owns continuity.
- **Devices** are paired TermiRust desktops. A paired Device lists durable Device Sessions;
  the desktop Host service owns replay, activity truth, and single-writer coordination.
- Device access uses one explicit private-network, SSH Controller, or self-hosted relay
  selection. Only a configured transport is selectable; route changes close the source first.
- The route types use separate credential stores and never silently transfer credentials,
  terminal ownership, or replay guarantees between these two paths.

## Current State

Implemented:

- One adaptive SwiftUI target for iPhone and iPad with Connections and Devices tabs.
- Permanent Direct SSH and Device Session labels on terminal routes.
- Privacy covers and input cleanup when the app becomes inactive or enters the background.
- Direct-SSH host list, import entry point, terminal detail view, and keyboard accessory row.
- Versioned mobile vault models.
- Plaintext fixture import for tests and encrypted production vault import through the shared Rust crypto library.
- `NativeMobileVaultDecryptor` Swift adapter for the Rust shared crypto XCFramework.
- Keychain wrapper using `kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`.
- Tmux bootstrap script generation.
- SwiftNIO SSH password and unencrypted OpenSSH Ed25519 private-key transport with pinned known-host verification, PTY shell startup, tmux bootstrap injection, terminal input, resize, and disconnect.
- Transcript-level terminal buffering for common redraw/control sequences such as carriage return, backspace, ANSI SGR, line erase, cursor movement, and clear screen.
- Canonical route, terminal, schema, tmux, and lifecycle verification.
- Shared Controller route phases, trust/capability projection, persisted explicit selection,
  route-scoped Keychain credential references, bounded same-route retry, and visible recovery.

Not finished yet:

- Encrypted private-key passphrase prompts and RSA/ECDSA private-key parsing.
- Full terminal emulator integration. SwiftTerm is the right candidate, but this Xcode install is missing the Metal Toolchain needed to build SwiftTerm. Re-enable SwiftTerm after installing it with `xcodebuild -downloadComponent MetalToolchain` or through Xcode Settings > Components.
- The production Device transport set currently installs private LAN/VPN only. Direct SSH
  remains available under Connections; SSH Controller and self-hosted relay stay visibly
  unconfigured until their dedicated native transport adapters are supplied.

## Build

```bash
cd /Users/jacob/Projects/terminal
scripts/sync-mobile-ffi-artifacts.sh ios

cd /Users/jacob/Projects/terminal_app/terminal_swift
./scripts/verify-ios-unified-routes.sh
./scripts/verify-ios-controller.sh --stage route-contract
```

Use `./scripts/verify-ios-unified-routes.sh --require-runtime` in release CI. Without an
eligible iOS destination, the default gate performs strict Swift 6 source and lifecycle
test type-checks and reports the missing runtime instead of claiming a device build.
