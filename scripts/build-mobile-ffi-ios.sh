#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

IOS_TARGETS=(
  aarch64-apple-ios
  aarch64-apple-ios-sim
  x86_64-apple-ios
)

for target in "${IOS_TARGETS[@]}"; do
  rustup target add "$target"
done

for target in "${IOS_TARGETS[@]}"; do
  cargo build -p termirust-mobile-ffi --release --target "$target"
done

DIST_DIR="$ROOT_DIR/dist/mobile/ios"
SIM_DIR="$DIST_DIR/simulator"
HEADER="$ROOT_DIR/crates/termirust-mobile-ffi/include/termirust_mobile.h"
LIB_NAME="libtermirust_mobile_ffi.a"

rm -rf "$SIM_DIR"
mkdir -p "$SIM_DIR"

lipo -create \
  "$ROOT_DIR/target/aarch64-apple-ios-sim/release/$LIB_NAME" \
  "$ROOT_DIR/target/x86_64-apple-ios/release/$LIB_NAME" \
  -output "$SIM_DIR/$LIB_NAME"

"$ROOT_DIR/scripts/create-ios-static-xcframework.sh" \
  TermiRustMobileCrypto \
  "$ROOT_DIR/target/aarch64-apple-ios/release/$LIB_NAME" \
  "$SIM_DIR/$LIB_NAME" \
  "$(dirname "$HEADER")" \
  "$DIST_DIR/TermiRustMobileCrypto.xcframework"

printf 'Built %s\n' "$DIST_DIR/TermiRustMobileCrypto.xcframework"
