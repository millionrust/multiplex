# Desktop updates

Status: accepted

Reviewed: 2026-09-22

## Why

Every release so far is an unsigned prerelease, and a person running one learns about the next
only by visiting the releases page. `update-trust.md` chose a verifier for signed update metadata
and said that no platform updater is authorized by it alone. This record authorizes one for the
desktop app, with the trust it can honestly claim today.

## Decision

`crates/multiplex-desktop/src/update/` checks, downloads, and installs desktop updates:

- **Check** twenty seconds after launch and every six hours, and from Settings → About, against
  `api.github.com/repos/millionrust/multiplex/releases`. The newest non-draft release whose tag
  is a plain `vX.Y.Z` above this build is offered; prereleases count.
- **Download** in the background, only from
  `github.com/millionrust/multiplex/releases/download/`, only the file named for this copy, bounded
  by the size the API gave, and checked against the `.sha256` published in the same release.
  Settings → About can turn background downloads off; then the app only says a version exists.
- **Install** only when the person chooses "Restart to Update". The app starts itself with
  `--apply-update <package> <version> <pid>` and quits. That process refuses any package outside
  its staging folder, waits for the app to exit, and then:
  - on macOS, unpacks the universal zip beside the running bundle with `ditto`, requires its
    `CFBundleShortVersionString` to be the offered version, swaps it in, and puts the old bundle
    back if the swap fails;
  - on Windows, stops the background service, which holds `multiplex.exe` open, and runs the
    per-user MSI with `msiexec /passive /norestart`. No administrator rights are needed.
  It then restarts an installed background service from the new files and opens the app. A failed
  install leaves the old version in place and is recorded, so the app offers that version's release
  page instead of trying the same package again.
- **Who updates:** the macOS app when it runs from an `.app` bundle, and Windows when it runs from
  the MSI's folder, `%LOCALAPPDATA%\Programs\Multiplex`. A portable Windows copy and Linux are told
  a version exists and pointed at its page: a `.deb` needs `sudo`, and a folder the person unpacked
  is theirs. Development and test builds never check. `MULTIPLEX_DISABLE_UPDATES` turns checking
  off.
- **Phones** are out of scope: they update through their stores.

## What the checksum does and does not prove

The `.sha256` comes from the same GitHub release as the package, so it proves the download arrived
whole, not who built it. Anyone who can publish a release on this repository can ship an update.
That is the same trust a person already extends by downloading an unsigned build from the same
page, and the release workflow only ever creates drafts (`scripts/verify/release-workflow.sh`), so
publishing stays a deliberate act by a maintainer. Signed update metadata through
`multiplex-update-trust`, and platform code signing, replace this when the project has keys.

## Contract with the release workflow

The updater looks for these exact asset names, so renaming them breaks updates for every copy in
the field: `Multiplex-macos-universal.zip`, `Multiplex-windows-x86_64.msi`,
`Multiplex-windows-aarch64.msi`, each with a `.sha256` beside it in `shasum` format.
