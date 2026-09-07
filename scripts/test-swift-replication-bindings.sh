#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
if [[ "$(uname -s)" != Darwin ]]; then
  printf 'This Swift conformance runner requires macOS.\n' >&2
  exit 1
fi

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termirust-replication-swift.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

cargo build --locked -p termirust-replication-bindings --lib
cargo build --locked -p termirust-controller-bindings --features bindgen-cli --bin uniffi-bindgen
TARGET_DIR="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c \
  'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug"

# Reuse the workspace's pinned generator; the generated namespace is independent.
"$TARGET_DIR/uniffi-bindgen" generate \
  "$TARGET_DIR/libtermirust_replication_bindings.dylib" \
  --language swift --language kotlin --no-format --out-dir "$TEMP_DIR"

swiftc -parse-as-library -module-name ReplicationCustodyConformance \
  -I "$TEMP_DIR" \
  -Xcc "-fmodule-map-file=$TEMP_DIR/TermiRustReplicationSecurityFFI.modulemap" \
  -L "$TARGET_DIR" -ltermirust_replication_bindings \
  "$TEMP_DIR/TermiRustReplicationSecurity.swift" \
  "$ROOT_DIR/tests/swift/replication_custody_conformance.swift" \
  -o "$TEMP_DIR/conformance"
DYLD_LIBRARY_PATH="$TARGET_DIR${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
  "$TEMP_DIR/conformance"
