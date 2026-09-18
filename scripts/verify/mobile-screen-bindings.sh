#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MODE="${1:-}"
if [[ "$MODE" != "--rebuild-twice" ]]; then
  printf 'Usage: scripts/verify/mobile-screen-bindings.sh --rebuild-twice\n' >&2
  exit 2
fi

cd "$ROOT_DIR"
"$ROOT_DIR/scripts/build/mobile-screen-bindings.sh" --clean --all
SECOND="$(mktemp -d "${TMPDIR:-/tmp}/termirust-screens-second.XXXXXX")"
trap 'rm -rf "$SECOND"' EXIT
"$ROOT_DIR/scripts/build/mobile-screen-bindings.sh" --clean --all --output "$SECOND/artifacts"

for path in abi-symbols-v1.txt android ios provenance-v1.txt; do
  diff -qr "$ROOT_DIR/dist/mobile/screens/$path" "$SECOND/artifacts/$path"
done
if ! diff -u \
  "$ROOT_DIR/dist/mobile/screens/artifacts.sha256" \
  "$SECOND/artifacts/artifacts.sha256"; then
  printf 'The aggregate artifact manifests differ after all shipped artifact trees matched.\n' >&2
  exit 1
fi
"$ROOT_DIR/scripts/sync/mobile-screen-bindings.sh" --check
cargo test --locked -p termirust-screen-bindings --all-targets

# The screen boundary owns pixels and input, never the connection that carries them.
if rg -n -i 'URLSession|Network\.framework|java\.net\.Socket|okhttp|Keychain|Keystore|analytics' \
  "$ROOT_DIR/dist/mobile/screens/ios/Sources" \
  "$ROOT_DIR/dist/mobile/screens/android/kotlin"; then
  printf 'Generated Remote Screens bindings contain a forbidden transport or storage surface.\n' >&2
  exit 1
fi

printf 'Remote Screens bindings are reproducible and conformant across two clean builds.\n'
