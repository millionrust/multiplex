# Building Multiplex for Distribution

This doc covers per-platform release builds and packaging. None of this
is required for `cargo run` development; it only matters when shipping
binaries to users.

## Common prerequisites

- Rust toolchain matching `rust-toolchain.toml` (or stable if absent).
- `cargo install cargo-bundle` for the macOS app bundle and Linux
  packages. `cargo-bundle` reads the `[package.metadata.bundle]`
  section in `crates/termirust-desktop/Cargo.toml`.
- App-icon vector master at `crates/termirust-desktop/assets/icons/app.svg`, with bundle exports at
  `crates/termirust-desktop/assets/icons/app.png` (512×512) and `crates/termirust-desktop/assets/icons/app@2x.png`
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

`cargo bundle` reads the package manifest in the current directory, so run the
bundle commands below from `crates/termirust-desktop`; output still lands in the
workspace `target/` directory.

### Unsigned `.app` (testing)

```bash
(cd crates/termirust-desktop && cargo bundle --release)
open target/release/bundle/osx/Multiplex.app
```

Release packaging runs `scripts/build/sign-macos-app.sh` after adding helper binaries and
again after merging the universal executable slices. This binds the bundle's Info.plist and
resources to `com.millionrust.multiplex`, replacing the linker's binary-only signature.
It defaults to ad-hoc signing; `MULTIPLEX_CODESIGN_IDENTITY` selects a certificate instead.
Ad-hoc signing does not preserve macOS permission identity across different app versions.

Screen display discovery and capture startup preflight Screen Recording permission without
prompting. Settings → Remote Devices requests access when sharing is explicitly enabled and
provides a link to the macOS privacy pane. After changing the grant, quit and reopen the app.
The background listener checks its own permission and never requests access automatically.

### Signed + notarized (distribution)

You need an active Apple Developer Program membership ($99/yr) and a
Developer ID Application certificate in your login keychain.

```bash
(cd crates/termirust-desktop && cargo bundle --release)
codesign --deep --force --options runtime \
  --sign "Developer ID Application: <Your Name> (TEAMID)" \
  target/release/bundle/osx/Multiplex.app

# Zip and submit for notarization
ditto -c -k --keepParent \
  target/release/bundle/osx/Multiplex.app Multiplex.zip
xcrun notarytool submit Multiplex.zip \
  --apple-id "<your-apple-id>" \
  --password "<app-specific-password>" \
  --team-id "TEAMID" \
  --wait

# Staple the ticket so the bundle works offline
xcrun stapler staple target/release/bundle/osx/Multiplex.app
```

The minimum supported macOS version is set in `crates/termirust-desktop/Cargo.toml`
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
  /fd sha256 /a target\wix\Multiplex-0.1.0-x86_64.msi
```

## Linux

### `.deb` and `.rpm`

```bash
(cd crates/termirust-desktop && cargo bundle --release --format deb)
(cd crates/termirust-desktop && cargo bundle --release --format rpm)
```

Outputs land in `target/release/bundle/{deb,rpm}/`.

The automated release workflow builds its `.deb` explicitly so `/usr/bin` contains the desktop
executable and all required sidecars. It also publishes a portable `.tar.gz`. The generic
`cargo bundle` commands above are developer-only until their contents pass
`scripts/verify/release-package.sh`.

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

Multiplex does not yet ship an auto-updater. The intended path:

1. Wire the `self_update` crate into a periodic check.
2. Host signed update manifests on a static origin (R2, S3, GitHub
   Releases — anything HTTPS will do).
3. Surface "Update available" in Settings, gated behind a user
   preference.

Until that lands, distribute releases via GitHub Releases and let
package managers (Homebrew, scoop, AUR) pick them up.

## Build caches

CI uses pinned [mr-boxington](https://mr-boxington.jdx.dev/github-action) 1.29.0 after
installing Rust 1.98.1. The shared local action installs its Cargo shim, so existing Cargo
commands inside shell, Python, and mobile build scripts are covered. Linux/macOS baselines,
Windows tests, Android, and iOS have separate cache generations; Windows test shards share
one generation. Default-branch pushes populate the cache, while pull requests and the manual
Windows probe only restore it. The action saves compiler work only when a job succeeds.
The first run of each new generation is cold; warm-run timings and cache-hit reports are
needed before claiming a speedup.

Linux CI fetches the locked dependency graph and saves the portable
`cargo-sources-v1-<Cargo.lock hash>` cache. Releases restore those downloads and the compiler
cache in dedicated `release-shipping-*-v2` namespaces, separate from test builds. Pushes to
`dev` warm these caches and build complete candidates; superseded branch runs are cancelled.
Pull requests do not run the release workflow. Tags and manual runs restore shipping work too.

Candidates and tags both use Cargo's standard release code generation: 16 codegen units,
local ThinLTO (`lto=false`), and optimization level 3. The root manifest still offers the
one-unit/cross-crate ThinLTO profile for local builds; CI overrides it for build speed. Android and iOS share one cached
`target` directory across their mobile libraries, avoiding repeated dependency and UniFFI
generator compilation. Cargo still fingerprints inputs, and ABI, page-alignment, manifest,
and package checks still run. The independent two-clean-build reproducibility checks leave
`MULTIPLEX_MOBILE_CARGO_TARGET_DIR` unset. `MBX_TARGET_VIEWS=0` preserves packaging paths.

A completed run uploads a flat `release-candidate` artifact for 30 days. Tagging the exact same
commit (or dispatching again) discovers that successful run, verifies the complete package set,
SBOM presence, commit and every checksum, and promotes those bytes without compiling again.
Candidate discovery accepts only successful push/manual runs of the same workflow in this
repository. A manual publication tag must resolve to the checked-out commit and match the
workspace version. Missing or expired candidates fall back to normal cached builds; a corrupt
candidate fails verification. Releases remain drafts for a person to publish.

The under-five-minute target applies to promotion of an existing complete candidate, excluding
runner queue delays. Cold builds and changed-source compilation have no measured five-minute
guarantee. Desktop jobs upload Cargo HTML timings as `build-timings-*` artifacts so subsequent
optimization can use measured crate durations. Mac universal packaging waits only for the Mac
builds, rather than the entire desktop matrix. Already-compressed packages are uploaded without
an extra compression pass. Android enables Gradle's task build cache.

mr-boxington's per-job report counts only compilations it looked up: Cargo's reuse of a
restored `target` directory is left out, so "0 hits" beside a short build step is a warm
build, not a cold one. Judge a run by its step times.

The macOS jobs cache the pinned `cargo-bundle` executable by runner OS, architecture, and
Rust version. A cache miss installs it with parallel code generation and no LTO; warm builds
restore it directly. The former background installer and its five-minute polling loop are gone.


Packaging checks, native tests, checksums and attestations still run. Release workflows
create drafts only; publication remains a deliberate step after verification.

Reference: [Pintail's CI](https://github.com/chittihq/pintail/blob/dev/.github/workflows/ci.yml)
uses the same pinned-toolchain/dependency-cache pattern. Its
[Docker build](https://github.com/chittihq/pintail/blob/dev/Dockerfile) separates dependency
compilation from application sources and exports container layers through GHCR.
Multiplex builds native macOS, Windows and mobile artifacts, so that container cache
is not a replacement for its platform-specific caches.
