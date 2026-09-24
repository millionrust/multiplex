# The Homebrew cask

`multiplex.rb` is the source of the cask. It lives here rather than only in the tap so it is
reviewed with the code it installs; `.github/workflows/homebrew.yml` copies it to the tap with the
version and checksum of a published release, and changes nothing else.

## Installing

```sh
brew install --cask millionrust/tap/multiplex
```

## How a release reaches the tap

Releases are published by a person, never by a workflow, so the cask job runs on
`release: published` — the moment a draft is made public — and can also be run by hand with a tag.
It skips prereleases, because a cask tracks stable releases and every release up to 0.0.4 is
marked prerelease. It reads the release's own `Multiplex-macos-universal.zip.sha256` rather than
hashing a download of its own, so the cask carries the number the release published.

It needs two things in this repository's settings:

- `HOMEBREW_TAP_TOKEN` — a fine-grained token with contents write on the tap repository, because
  `GITHUB_TOKEN` reaches only this one.
- `HOMEBREW_TAP_REPOSITORY` — a variable, only if the tap is not `millionrust/homebrew-tap`.

## Why `auto_updates true`

The app updates itself from the same releases (`docs/decisions/desktop-updates.md`). That line
tells Homebrew to leave it alone on `brew upgrade`, so the two never fight over the same bundle;
the app's own "Restart to Update" stays the single path. `brew upgrade --greedy` still forces it
for anyone who wants that.

## What uninstalling does

`brew uninstall` quits the app, runs `multiplex --uninstall-cleanup` — which removes the
background service's login entry and any terminal profiles pointing at this copy — and then
unloads a LaunchAgent left by an older build. Your data stays: saved hosts, vaults, pinned host
keys, snippets, session history, and durable sessions are all under
`~/Library/Application Support/multiplex`, and only `brew zap` removes that. Secrets in the login
keychain are never touched by either.

## Submitting to homebrew-cask

Two things to clear first: releases must stop being marked prerelease (or wait for a stable
release), and the project has to pass the notability bar the cask repository applies. The same
file is submitted; only the tap it lands in changes.

## The part Homebrew does not fix

The app is unsigned, so Homebrew quarantines it like any other download and macOS still warns on
first open. Only a Developer ID and notarisation remove that, the keychain prompt after every
update, and the Local Network prompt. The cask says so in its `caveats`.
