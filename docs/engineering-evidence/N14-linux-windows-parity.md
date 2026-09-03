# N14 Linux and Windows Parity Evidence

Date: 2026-09-03

## Implemented

- CI now compiles and tests the complete workspace on `windows-2022`; Linux and macOS remain in
  the primary matrix, and pushes to the active `test` branch are no longer omitted.
- Separate CI jobs execute the canonical native Android and iOS/iPadOS verifiers; the Android job
  also runs lint, so desktop-only success cannot conceal a broken mobile build.
- Linux and Windows use native `notify-rust` backends instead of the former unavailable stub.
  Version 4.17.0 is pinned because 4.18.0 requires Rust 1.89 while the workspace MSRV is 1.88.
- Existing native keyring features cover Apple Keychain, Windows Credential Manager, and Linux
  Secret Service; `rfd` remains the native file-dialog boundary on all three desktops.
- Revisioned metadata stores now take real cross-process advisory locks through `fs2` on Unix and
  Windows. Interactive stores retain their bounded two-second acquisition deadline; health scans
  retain shared locks; lease ownership retains non-blocking exclusivity. The replica repository
  additionally opens its Windows lock without following a reparse point and compares recovery
  evidence with Windows volume/file identity rather than Unix inode fields.
- Release builds use the pinned Rust toolchain and current hosted runners. The retired
  `macos-13` label was replaced by `macos-15-intel`.
- macOS ZIP, Linux DEB/tarball, and Windows ZIP packages contain the GUI plus all five required
  sidecars, including the optional self-hosted relay operator. Missing files and bundle failures
  stop the workflow.
- Every artifact receives a SHA-256 checksum, SPDX JSON SBOM, and GitHub provenance attestation.
  Unsigned artifacts can only become draft prereleases.

## Local Evidence

```text
./scripts/verify-release-workflow.sh
PASS

./scripts/verify-release-package.sh target/debug
PASS: desktop application and all required sidecars are present

cargo build --release --locked \
  -p termirust -p termirust-cli -p termirust-session-host -p termirust-mcp \
  -p termirust-relay-server
cargo bundle --release
# Stage termirust-cli, termirust-session-host, termirust-mcp,
# termirust-mcp-authorize, and termirust-relay beside the app executable.
./scripts/verify-release-package.sh target/release/bundle/osx/TermiRust.app/Contents/MacOS
PASS: release package contains the desktop application and all required sidecars

target/release/bundle/osx/TermiRust.app/Contents/MacOS/termirust-cli --help
PASS: packaged CLI launches and advertises the expected Session, Project, and Controller surface

cargo check --workspace --all-targets --all-features --locked
PASS on macOS arm64

cargo test -p termirust-store --all-targets --locked
PASS: 126 tests passed, 1 benchmark ignored by design

cargo clippy -p termirust-store --all-targets --locked -- -D warnings
PASS

cargo check -p termirust-store --all-targets \
  --target x86_64-pc-windows-msvc --locked
PASS from macOS with the Windows standard library target

cargo clippy -p termirust-store --all-targets \
  --target x86_64-pc-windows-msvc --locked -- -D warnings
PASS from macOS with the Windows standard library target

./scripts/verify-controller-security-vectors.sh --check
PASS after the dependency lock changed
```

## External Exit Gates

N14 is not complete until the committed workflow passes on Ubuntu Wayland and Windows, and a human
installs each produced package to verify terminal input/resize, secure storage, notifications,
dialogs, window restore, screen-reader interaction, upgrade, and rollback. macOS signing and
notarization and Windows code signing require owner-held credentials. These cannot be fabricated or
marked complete by a macOS development machine. Cross-target compilation proves Windows source
portability for the metadata store, but it does not execute Windows locking, reparse-point, secure
storage, notification, UI, or installer behavior.
