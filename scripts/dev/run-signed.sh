#!/usr/bin/env bash
# Cargo runner for macOS. `cargo build` gives each binary an ad-hoc signature whose
# identifier changes with every build, so the Keychain treats each rebuild as a new app
# and asks for access again. Before running the desktop app this re-signs it with a
# stable identifier and a local code-signing identity, so "Always Allow" survives
# rebuilds. Every other binary (tests, examples, other crates) runs unchanged.
#
# The identity is $TERMIRUST_CODESIGN_IDENTITY, or else the first "Apple Development"
# identity in the login keychain. With neither, the binary runs as built.
set -euo pipefail

binary=$1
shift

if [[ "$(uname -s)" == "Darwin" && "$(basename "$binary")" == "multiplex" && "$binary" != */deps/* ]]; then
  identity=${TERMIRUST_CODESIGN_IDENTITY:-}
  if [[ -z "$identity" ]]; then
    identity=$(security find-identity -v -p codesigning 2>/dev/null |
      sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Apple Development: .*"$/\1/p' | head -n 1 || true)
  fi
  signature=$(codesign -dv "$binary" 2>&1 || true)
  if [[ -n "$identity" && "$signature" == *"Signature=adhoc"* ]]; then
    if ! codesign --force --sign "$identity" --identifier com.termirust.desktop.dev \
      "$binary" 2>/dev/null; then
      echo "run-signed: could not sign $binary; the Keychain may ask for access again" >&2
    fi
  fi
fi

exec "$binary" "$@"
