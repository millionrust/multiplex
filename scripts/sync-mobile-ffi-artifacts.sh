#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-/Users/jacob/Projects/terminal_app/terminal_swift}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-/Users/jacob/Projects/terminal_app/terminal_kotlin}"

usage() {
  cat <<'USAGE'
Usage: scripts/sync-mobile-ffi-artifacts.sh [ios|android|all]

Builds TermiRust's shared Rust mobile FFI artifacts and copies them into the
companion mobile app repositories.

Environment overrides:
  TERMIRUST_IOS_DIR       iOS app repo path
  TERMIRUST_ANDROID_DIR   Android app repo path
USAGE
}

target="${1:-all}"
case "$target" in
  ios|android|all) ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac

sync_ios() {
  if [[ ! -d "$IOS_DIR" ]]; then
    echo "iOS app directory not found: $IOS_DIR" >&2
    exit 1
  fi

  "$ROOT_DIR/scripts/build-mobile-ffi-ios.sh"

  local source="$ROOT_DIR/dist/mobile/ios/TermiRustMobileCrypto.xcframework"
  local destination="$IOS_DIR/Frameworks/TermiRustMobileCrypto.xcframework"

  if [[ ! -d "$source" ]]; then
    echo "Expected iOS XCFramework was not built at $source" >&2
    exit 1
  fi

  rm -rf "$destination"
  mkdir -p "$(dirname "$destination")"
  cp -R "$source" "$destination"
  echo "Synced iOS XCFramework to $destination"
}

sync_android() {
  if [[ ! -d "$ANDROID_DIR" ]]; then
    echo "Android app directory not found: $ANDROID_DIR" >&2
    exit 1
  fi

  "$ROOT_DIR/scripts/build-mobile-ffi-android.sh"

  local source="$ROOT_DIR/dist/mobile/android/jniLibs"
  local destination="$ANDROID_DIR/app/src/main/jniLibs"

  if [[ ! -d "$source" ]]; then
    echo "Expected Android JNI libraries were not built at $source" >&2
    exit 1
  fi

  rm -rf "$destination"
  mkdir -p "$(dirname "$destination")"
  cp -R "$source" "$destination"
  echo "Synced Android JNI libraries to $destination"
}

case "$target" in
  ios)
    sync_ios
    ;;
  android)
    sync_android
    ;;
  all)
    sync_ios
    sync_android
    ;;
esac
