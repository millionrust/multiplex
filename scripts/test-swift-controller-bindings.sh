#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS="$ROOT_DIR/dist/mobile/controller"
GENERATED="$ARTIFACTS/ios/Sources/TermiRustControllerSecurity.swift"
FRAMEWORKS="$ARTIFACTS/ios/TermiRustControllerSecurity.xcframework/ios-arm64"
MODULE_MAP="$FRAMEWORKS/TermiRustControllerSecurityFFI.framework/Modules/module.modulemap"
NATIVE="$ARTIFACTS/kotlin-test/darwin-aarch64"
FIXTURE="$ROOT_DIR/crates/termirust-controller-security/tests/vectors/controller-v1.json"
RUNNER="$ROOT_DIR/tests/swift/controller_binding_conformance.swift"

for required_path in "$GENERATED" "$MODULE_MAP" \
  "$NATIVE/libtermirust_controller_bindings.dylib" "$FIXTURE" "$RUNNER"; do
  [[ -e "$required_path" ]] || {
    printf 'Missing Controller binding input: %s\n' "$required_path" >&2
    exit 1
  }
done

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termirust-controller-swift.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

swiftc \
  -parse-as-library \
  -module-name TermiRustControllerSecurityConformance \
  -F "$FRAMEWORKS" \
  -L "$NATIVE" \
  -ltermirust_controller_bindings \
  "$GENERATED" \
  "$RUNNER" \
  -o "$TEMP_DIR/controller-binding-conformance"

DYLD_LIBRARY_PATH="$NATIVE${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
  "$TEMP_DIR/controller-binding-conformance" "$FIXTURE"
