#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOURCE="$ROOT_DIR/dist/mobile/controller"
IOS_DIR="${MULTIPLEX_IOS_DIR:-${TERMIRUST_IOS_DIR:-$ROOT_DIR/apps/ios}}"
ANDROID_DIR="${MULTIPLEX_ANDROID_DIR:-${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/apps/android}}"
MODE="${1:---check}"
PLATFORM="${2:---all}"
LIB="libmultiplex_controller_bindings.so"

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

IOS_FRAMEWORK="$IOS_DIR/Frameworks/MultiplexControllerSecurity.xcframework"
IOS_SWIFT="$IOS_DIR/MultiplexMobile/Generated/MultiplexControllerSecurity.swift"
IOS_FIXTURE="$IOS_DIR/MultiplexMobileTests/Fixtures/controller-v2.json"
ANDROID_KOTLIN="$ANDROID_DIR/app/src/main/java/com/multiplex/controller/security/multiplex_controller_bindings.kt"
ANDROID_FIXTURE="$ANDROID_DIR/app/src/test/resources/controller-v2.json"
ANDROID_TEST_NATIVE="$ANDROID_DIR/app/src/test/native"
FIXTURE="$ROOT_DIR/crates/multiplex-controller-security/tests/vectors/controller-v2.json"
ROUTE_FIXTURE="$ROOT_DIR/tests/fixtures/controller-routes/route-plan-v1.json"
IOS_ROUTE_FIXTURE="$IOS_DIR/MultiplexMobileTests/Fixtures/route-plan-v1.json"
ANDROID_ROUTE_FIXTURE="$ANDROID_DIR/app/src/test/resources/route-plan-v1.json"

if [[ "$MODE" == "--write" ]]; then
  if [[ "$DO_IOS" -eq 1 ]]; then
    rm -rf "$IOS_FRAMEWORK"
    mkdir -p "$(dirname "$IOS_FRAMEWORK")" "$(dirname "$IOS_SWIFT")" "$(dirname "$IOS_FIXTURE")"
    cp -R "$SOURCE/ios/MultiplexControllerSecurity.xcframework" "$IOS_FRAMEWORK"
    cp "$SOURCE/ios/Sources/MultiplexControllerSecurity.swift" "$IOS_SWIFT"
    cp "$FIXTURE" "$IOS_FIXTURE"
    cp "$ROUTE_FIXTURE" "$IOS_ROUTE_FIXTURE"
  fi
  if [[ "$DO_ANDROID" -eq 1 ]]; then
    mkdir -p "$(dirname "$ANDROID_KOTLIN")" "$(dirname "$ANDROID_FIXTURE")"
    cp "$SOURCE/android/kotlin/com/multiplex/controller/security/multiplex_controller_bindings.kt" "$ANDROID_KOTLIN"
    cp "$FIXTURE" "$ANDROID_FIXTURE"
    cp "$ROUTE_FIXTURE" "$ANDROID_ROUTE_FIXTURE"
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
  diff -qr "$SOURCE/ios/MultiplexControllerSecurity.xcframework" "$IOS_FRAMEWORK"
  cmp "$SOURCE/ios/Sources/MultiplexControllerSecurity.swift" "$IOS_SWIFT"
  cmp "$FIXTURE" "$IOS_FIXTURE"
  cmp "$ROUTE_FIXTURE" "$IOS_ROUTE_FIXTURE"
fi
if [[ "$DO_ANDROID" -eq 1 ]]; then
  cmp "$SOURCE/android/kotlin/com/multiplex/controller/security/multiplex_controller_bindings.kt" "$ANDROID_KOTLIN"
  cmp "$FIXTURE" "$ANDROID_FIXTURE"
  cmp "$ROUTE_FIXTURE" "$ANDROID_ROUTE_FIXTURE"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    cmp "$SOURCE/android/jniLibs/$abi/$LIB" "$ANDROID_DIR/app/src/main/jniLibs/$abi/$LIB"
  done
fi
printf 'Swift and Kotlin Controller bindings match generated artifacts (%s).\n' "${PLATFORM#--}"
