#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOURCE="$ROOT_DIR/dist/mobile/screens"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/apps/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/apps/android}"
MODE="${1:---check}"
LIB="libtermirust_screen_bindings.so"

if [[ "$MODE" != "--check" && "$MODE" != "--write" ]]; then
  printf 'Usage: scripts/sync/mobile-screen-bindings.sh [--check|--write]\n' >&2
  exit 2
fi
[[ -f "$SOURCE/artifacts.sha256" ]] || {
  printf 'Remote Screens binding artifacts are missing. Run scripts/build/mobile-screen-bindings.sh first.\n' >&2
  exit 1
}

IOS_FRAMEWORK="$IOS_DIR/Frameworks/TermiRustRemoteScreens.xcframework"
IOS_SWIFT="$IOS_DIR/TermiRustMobile/Generated/TermiRustRemoteScreens.swift"
ANDROID_KOTLIN="$ANDROID_DIR/app/src/main/java/com/termirust/screens/termirust_screen_bindings.kt"

if [[ "$MODE" == "--write" ]]; then
  rm -rf "$IOS_FRAMEWORK"
  mkdir -p "$(dirname "$IOS_FRAMEWORK")" "$(dirname "$IOS_SWIFT")"
  cp -R "$SOURCE/ios/TermiRustRemoteScreens.xcframework" "$IOS_FRAMEWORK"
  cp "$SOURCE/ios/Sources/TermiRustRemoteScreens.swift" "$IOS_SWIFT"

  mkdir -p "$(dirname "$ANDROID_KOTLIN")"
  cp "$SOURCE/android/kotlin/com/termirust/screens/termirust_screen_bindings.kt" "$ANDROID_KOTLIN"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    mkdir -p "$ANDROID_DIR/app/src/main/jniLibs/$abi"
    cp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
  done
  printf 'Remote Screens bindings synced to Swift and Kotlin repositories.\n'
  exit 0
fi

diff -qr "$SOURCE/ios/TermiRustRemoteScreens.xcframework" "$IOS_FRAMEWORK"
cmp "$SOURCE/ios/Sources/TermiRustRemoteScreens.swift" "$IOS_SWIFT"
cmp "$SOURCE/android/kotlin/com/termirust/screens/termirust_screen_bindings.kt" "$ANDROID_KOTLIN"
for abi in arm64-v8a armeabi-v7a x86 x86_64; do
  cmp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
done
printf 'Swift and Kotlin Remote Screens bindings match generated artifacts.\n'
