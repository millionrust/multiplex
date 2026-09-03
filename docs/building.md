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

Build the command-line sidecars that release packages install beside the desktop app:

```bash
cargo build --release --locked \
  -p termirust-cli -p termirust-session-host -p termirust-mcp -p termirust-relay-server
```

The MCP package also builds `termirust-mcp-authorize`; install both MCP executables together.
Official workflow artifacts contain all six required executables: `termirust`, `termirust-cli`,
`termirust-session-host`, `termirust-mcp`, `termirust-mcp-authorize`, and `termirust-relay`. Do not
distribute a bare `termirust` executable: durable local Sessions and MCP actions depend on those
siblings. The relay supplies the optional operator workflow documented in
[`self-hosted-relay.md`](self-hosted-relay.md).
Inspection is read-only by default, while action capabilities require local scoped approval. The
capability and security contracts are documented in [`mcp.md`](mcp.md).

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

The current automated release workflow produces a portable ZIP containing the desktop executable
and all five sidecars. MSI generation remains a separate Windows qualification step and must not
be claimed from the ZIP build alone.

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

The automated release workflow builds its `.deb` explicitly so `/usr/bin` contains the desktop
executable and all required sidecars. It also publishes a portable `.tar.gz`. The generic
`cargo bundle` commands above are developer-only until their contents pass
`scripts/verify-release-package.sh`.

### AppImage

```bash
cargo install cargo-appimage
cargo appimage
```

### Snap / Flatpak

Both formats need their own packaging recipes (`snapcraft.yaml` /
flatpak manifest). These aren't included yet; PRs welcome.

## Artifact integrity

Every automated package is accompanied by a SHA-256 checksum, an SPDX JSON SBOM, and GitHub build
provenance. Verify the checksum before installation and, for GitHub releases, verify provenance
with `gh attestation verify <artifact> -R jacobsam/terminal`.

Packaging is fail-closed: a missing sidecar, failed bundle, empty output, checksum failure, or SBOM
failure stops the workflow. Signing and platform-store distribution are separate release gates;
an unsigned dry-run artifact is not a public-release approval.

## Auto-update

TermiRust does not yet ship an auto-updater. The intended path:

1. Wire the `self_update` crate into a periodic check.
2. Host signed update manifests on a static origin (R2, S3, GitHub
   Releases — anything HTTPS will do).
3. Surface "Update available" in Settings, gated behind a user
   preference.

Until that lands, distribute releases via GitHub Releases and let
package managers (Homebrew, scoop, AUR) pick them up.
