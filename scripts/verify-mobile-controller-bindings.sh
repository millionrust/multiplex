#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ "$MODE" != "--rebuild-twice" ]]; then
  printf 'Usage: scripts/verify-mobile-controller-bindings.sh --rebuild-twice\n' >&2
  exit 2
fi

cd "$ROOT_DIR"
"$ROOT_DIR/scripts/build-mobile-controller-bindings.sh" --clean --all
SECOND="$(mktemp -d "${TMPDIR:-/tmp}/termirust-controller-second.XXXXXX")"
trap 'rm -rf "$SECOND"' EXIT
"$ROOT_DIR/scripts/build-mobile-controller-bindings.sh" --clean --all --output "$SECOND/artifacts"

for path in abi-symbols-v1.txt android ios provenance-v1.txt; do
  diff -qr "$ROOT_DIR/dist/mobile/controller/$path" "$SECOND/artifacts/$path"
done
if ! diff -u \
  "$ROOT_DIR/dist/mobile/controller/artifacts.sha256" \
  "$SECOND/artifacts/artifacts.sha256"; then
  printf 'The aggregate artifact manifests differ after all shipped artifact trees matched.\n' >&2
  exit 1
fi
"$ROOT_DIR/scripts/sync-mobile-controller-bindings.sh" --check
cargo test --locked -p termirust-controller-bindings --all-targets
"$ROOT_DIR/scripts/test-swift-controller-bindings.sh"

if rg -n -i 'URLSession|Network\.framework|java\.net\.Socket|okhttp|terminal parser|vault decrypt|license check|analytics|account service' \
  "$ROOT_DIR/dist/mobile/controller/ios/Sources" \
  "$ROOT_DIR/dist/mobile/controller/android/kotlin"; then
  printf 'Generated Controller bindings contain a forbidden transport/product surface.\n' >&2
  exit 1
fi

printf 'Controller bindings are reproducible and conformant across two clean builds.\n'
