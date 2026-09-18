#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOURCE="$ROOT_DIR/dist/mobile/controller"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/apps/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/apps/android}"
MODE="${1:---check}"
PLATFORM="${2:---all}"
LIB="libtermirust_controller_bindings.so"

if [[ "$MODE" != "--check" && "$MODE" != "--write" ]] \
  || [[ "$PLATFORM" != "--all" && "$PLATFORM" != "--ios" && "$PLATFORM" != "--android" ]]; then
  printf 'Usage: scripts/sync/mobile-controller-bindings.sh [--check|--write] [--all|--ios|--android]\n' >&2
  exit 2
fi
# A Mac without the Android NDK can still build and sync the iOS half.
DO_IOS=0
DO_ANDROID=0
[[ "$PLATFORM" == "--all" || "$PLATFORM" == "--ios" ]] && DO_IOS=1
[[ "$PLATFORM" == "--all" || "$PLATFORM" == "--android" ]] && DO_ANDROID=1
[[ -f "$SOURCE/artifacts.sha256" ]] || {
  printf 'Controller binding artifacts are missing. Run scripts/build/mobile-controller-bindings.sh first.\n' >&2
  exit 1
}

IOS_FRAMEWORK="$IOS_DIR/Frameworks/TermiRustControllerSecurity.xcframework"
IOS_SWIFT="$IOS_DIR/TermiRustMobile/Generated/TermiRustControllerSecurity.swift"
IOS_FIXTURE="$IOS_DIR/TermiRustMobileTests/Fixtures/controller-v1.json"
ANDROID_KOTLIN="$ANDROID_DIR/app/src/main/java/com/termirust/controller/security/termirust_controller_bindings.kt"
ANDROID_FIXTURE="$ANDROID_DIR/app/src/test/resources/controller-v1.json"
ANDROID_TEST_NATIVE="$ANDROID_DIR/app/src/test/native"
FIXTURE="$ROOT_DIR/crates/termirust-controller-security/tests/vectors/controller-v1.json"

if [[ "$MODE" == "--write" ]]; then
  if [[ "$DO_IOS" -eq 1 ]]; then
    rm -rf "$IOS_FRAMEWORK"
    mkdir -p "$(dirname "$IOS_FRAMEWORK")" "$(dirname "$IOS_SWIFT")" "$(dirname "$IOS_FIXTURE")"
    cp -R "$SOURCE/ios/TermiRustControllerSecurity.xcframework" "$IOS_FRAMEWORK"
    cp "$SOURCE/ios/Sources/TermiRustControllerSecurity.swift" "$IOS_SWIFT"
    cp "$FIXTURE" "$IOS_FIXTURE"
  fi
  if [[ "$DO_ANDROID" -eq 1 ]]; then
    mkdir -p "$(dirname "$ANDROID_KOTLIN")" "$(dirname "$ANDROID_FIXTURE")"
    cp "$SOURCE/android/kotlin/com/termirust/controller/security/termirust_controller_bindings.kt" "$ANDROID_KOTLIN"
    cp "$FIXTURE" "$ANDROID_FIXTURE"
    rm -rf "$ANDROID_TEST_NATIVE"
    mkdir -p "$ANDROID_TEST_NATIVE"
    cp -R "$SOURCE/kotlin-test/." "$ANDROID_TEST_NATIVE/"
    for abi in arm64-v8a armeabi-v7a x86 x86_64; do
      mkdir -p "$ANDROID_DIR/app/src/main/jniLibs/$abi"
      cp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
    done
  fi
  printf 'Controller bindings synced (%s).\n' "${PLATFORM#--}"
  exit 0
fi

if [[ "$DO_IOS" -eq 1 ]]; then
  diff -qr "$SOURCE/ios/TermiRustControllerSecurity.xcframework" "$IOS_FRAMEWORK"
  cmp "$SOURCE/ios/Sources/TermiRustControllerSecurity.swift" "$IOS_SWIFT"
  cmp "$FIXTURE" "$IOS_FIXTURE"
fi
if [[ "$DO_ANDROID" -eq 1 ]]; then
  cmp "$SOURCE/android/kotlin/com/termirust/controller/security/termirust_controller_bindings.kt" "$ANDROID_KOTLIN"
  cmp "$FIXTURE" "$ANDROID_FIXTURE"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    cmp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
  done
fi
printf 'Swift and Kotlin Controller bindings match generated artifacts (%s).\n' "${PLATFORM#--}"
