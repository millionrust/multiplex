#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOURCE="$ROOT_DIR/dist/mobile/screens"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/apps/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/apps/android}"
MODE="${1:---check}"
PLATFORM="${2:---all}"
LIB="libmultiplex_screen_bindings.so"

if [[ "$MODE" != "--check" && "$MODE" != "--write" ]] \
  || [[ "$PLATFORM" != "--all" && "$PLATFORM" != "--ios" && "$PLATFORM" != "--android" ]]; then
  printf 'Usage: scripts/sync/mobile-screen-bindings.sh [--check|--write] [--all|--ios|--android]\n' >&2
  exit 2
fi
# A Mac without the Android NDK can still build and sync the iOS half.
DO_IOS=0
DO_ANDROID=0
[[ "$PLATFORM" == "--all" || "$PLATFORM" == "--ios" ]] && DO_IOS=1
[[ "$PLATFORM" == "--all" || "$PLATFORM" == "--android" ]] && DO_ANDROID=1
[[ -f "$SOURCE/artifacts.sha256" ]] || {
  printf 'Remote Screens binding artifacts are missing. Run scripts/build/mobile-screen-bindings.sh first.\n' >&2
  exit 1
}

IOS_FRAMEWORK="$IOS_DIR/Frameworks/MultiplexRemoteScreens.xcframework"
IOS_SWIFT="$IOS_DIR/MultiplexMobile/Generated/MultiplexRemoteScreens.swift"
ANDROID_KOTLIN="$ANDROID_DIR/app/src/main/java/com/multiplex/screens/multiplex_screen_bindings.kt"

if [[ "$MODE" == "--write" ]]; then
  if [[ "$DO_IOS" -eq 1 ]]; then
    rm -rf "$IOS_FRAMEWORK"
    mkdir -p "$(dirname "$IOS_FRAMEWORK")" "$(dirname "$IOS_SWIFT")"
    cp -R "$SOURCE/ios/MultiplexRemoteScreens.xcframework" "$IOS_FRAMEWORK"
    cp "$SOURCE/ios/Sources/MultiplexRemoteScreens.swift" "$IOS_SWIFT"
  fi
  if [[ "$DO_ANDROID" -eq 1 ]]; then
    mkdir -p "$(dirname "$ANDROID_KOTLIN")"
    cp "$SOURCE/android/kotlin/com/multiplex/screens/multiplex_screen_bindings.kt" "$ANDROID_KOTLIN"
    for abi in arm64-v8a armeabi-v7a x86 x86_64; do
      mkdir -p "$ANDROID_DIR/app/src/main/jniLibs/$abi"
      cp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
    done
  fi
  printf 'Remote Screens bindings synced (%s).\n' "${PLATFORM#--}"
  exit 0
fi

if [[ "$DO_IOS" -eq 1 ]]; then
  diff -qr "$SOURCE/ios/MultiplexRemoteScreens.xcframework" "$IOS_FRAMEWORK"
  cmp "$SOURCE/ios/Sources/MultiplexRemoteScreens.swift" "$IOS_SWIFT"
fi
if [[ "$DO_ANDROID" -eq 1 ]]; then
  cmp "$SOURCE/android/kotlin/com/multiplex/screens/multiplex_screen_bindings.kt" "$ANDROID_KOTLIN"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    cmp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
  done
fi
printf 'Swift and Kotlin Remote Screens bindings match generated artifacts (%s).\n' "${PLATFORM#--}"
