#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARTIFACTS="$ROOT_DIR/dist/mobile/screens"
GENERATED="$ARTIFACTS/ios/Sources/TermiRustRemoteScreens.swift"
FRAMEWORKS="$ARTIFACTS/ios/TermiRustRemoteScreens.xcframework/ios-arm64"
MODULE_MAP="$FRAMEWORKS/TermiRustRemoteScreensFFI.framework/Modules/module.modulemap"
NATIVE="$ARTIFACTS/kotlin-test/darwin-aarch64"
FIXTURE="$ROOT_DIR/crates/termirust-screen-bindings/tests/vectors/screen-session-v1.json"
RUNNER="$ROOT_DIR/tests/swift/screen_binding_conformance.swift"

for required_path in "$GENERATED" "$MODULE_MAP" \
  "$NATIVE/libtermirust_screen_bindings.dylib" "$FIXTURE" "$RUNNER"; do
  [[ -e "$required_path" ]] || {
    printf 'Missing Remote Screens binding input: %s\n' "$required_path" >&2
    printf 'Run scripts/build/mobile-screen-bindings.sh --ios first.\n' >&2
    exit 1
  }
done

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termirust-screens-swift.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

swiftc \
  -parse-as-library \
  -module-name TermiRustRemoteScreensConformance \
  -F "$FRAMEWORKS" \
  -L "$NATIVE" \
  -ltermirust_screen_bindings \
  "$GENERATED" \
  "$RUNNER" \
  -o "$TEMP_DIR/screen-binding-conformance"

DYLD_LIBRARY_PATH="$NATIVE${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
  "$TEMP_DIR/screen-binding-conformance" "$FIXTURE"
