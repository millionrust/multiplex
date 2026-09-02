# Building TermiRust for Distribution

This doc covers per-platform release builds and packaging. None of this
is required for `cargo run` development; it only matters when shipping
binaries to users.

## Common prerequisites

- Rust toolchain matching `rust-toolchain.toml` (or stable if absent).
- `cargo install cargo-bundle` for the macOS app bundle and Linux
  packages. `cargo-bundle` reads the `[package.metadata.bundle]`
  section in `Cargo.toml`.
- App-icon vector master at `assets/icons/app.svg`, with bundle exports at
  `assets/icons/app.png` (512×512) and `assets/icons/app@2x.png`
  (1024×1024 retina).

## macOS

### Unsigned `.app` (testing)

```bash
cargo bundle --release
open target/release/bundle/osx/TermiRust.app
```

### Signed + notarized (distribution)

You need an active Apple Developer Program membership ($99/yr) and a
Developer ID Application certificate in your login keychain.

```bash
cargo bundle --release
codesign --deep --force --options runtime \
  --sign "Developer ID Application: <Your Name> (TEAMID)" \
  target/release/bundle/osx/TermiRust.app

# Zip and submit for notarization
ditto -c -k --keepParent \
  target/release/bundle/osx/TermiRust.app TermiRust.zip
xcrun notarytool submit TermiRust.zip \
  --apple-id "<your-apple-id>" \
  --password "<app-specific-password>" \
  --team-id "TEAMID" \
  --wait

# Staple the ticket so the bundle works offline
xcrun stapler staple target/release/bundle/osx/TermiRust.app
```

The minimum supported macOS version is set in `Cargo.toml`
(`osx_minimum_system_version`).

## Windows

### Unsigned MSI (testing)

```powershell
cargo install cargo-wix
cargo wix init
cargo wix --release
```

The MSI lands in `target/wix/`.

### Signed MSI (distribution)

You need a Windows code-signing certificate from a CA (DigiCert,
Sectigo, etc.). Cost varies; expect $200–$500/yr for a standard cert
or more for an EV cert that bypasses SmartScreen prompts.

```powershell
signtool sign /tr http://timestamp.digicert.com /td sha256 ^
  /fd sha256 /a target\wix\TermiRust-0.1.0-x86_64.msi
```

## Linux

### `.deb` and `.rpm`

```bash
cargo bundle --release --format deb
cargo bundle --release --format rpm
```

Outputs land in `target/release/bundle/{deb,rpm}/`.

### AppImage

```bash
cargo install cargo-appimage
cargo appimage
```

### Snap / Flatpak

Both formats need their own packaging recipes (`snapcraft.yaml` /
flatpak manifest). These aren't included yet; PRs welcome.

## Auto-update

TermiRust does not yet ship an auto-updater. The intended path:

1. Wire the `self_update` crate into a periodic check.
2. Host signed update manifests on a static origin (R2, S3, GitHub
   Releases — anything HTTPS will do).
3. Surface "Update available" in Settings, gated behind a user
   preference.

Until that lands, distribute releases via GitHub Releases and let
package managers (Homebrew, scoop, AUR) pick them up.
