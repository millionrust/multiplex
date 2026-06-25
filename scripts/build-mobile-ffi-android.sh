#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ANDROID_API="${ANDROID_API:-23}"
ANDROID_SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
ANDROID_NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"

if [[ -z "$ANDROID_NDK" ]]; then
  ANDROID_NDK="$(find "$ANDROID_SDK/ndk" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -1)"
fi

if [[ -z "$ANDROID_NDK" || ! -d "$ANDROID_NDK" ]]; then
  printf 'Android NDK not found. Set ANDROID_NDK_HOME or install the NDK in %s/ndk.\n' "$ANDROID_SDK" >&2
  exit 1
fi

HOST_TAG="darwin-x86_64"
TOOLCHAIN="$ANDROID_NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"

if [[ ! -d "$TOOLCHAIN" ]]; then
  printf 'Android NDK LLVM toolchain not found at %s.\n' "$TOOLCHAIN" >&2
  exit 1
fi

TARGETS=(
  aarch64-linux-android
  armv7-linux-androideabi
  i686-linux-android
  x86_64-linux-android
)

for target in "${TARGETS[@]}"; do
  rustup target add "$target"
done

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/aarch64-linux-android${ANDROID_API}-clang"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$TOOLCHAIN/armv7a-linux-androideabi${ANDROID_API}-clang"
export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$TOOLCHAIN/i686-linux-android${ANDROID_API}-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$TOOLCHAIN/x86_64-linux-android${ANDROID_API}-clang"

DIST_DIR="$ROOT_DIR/dist/mobile/android/jniLibs"
rm -rf "$DIST_DIR"

for target in "${TARGETS[@]}"; do
  cargo build -p termirust-mobile-ffi --release --target "$target"

  case "$target" in
    aarch64-linux-android) abi="arm64-v8a" ;;
    armv7-linux-androideabi) abi="armeabi-v7a" ;;
    i686-linux-android) abi="x86" ;;
    x86_64-linux-android) abi="x86_64" ;;
    *)
      printf 'No Android ABI mapping for target %s.\n' "$target" >&2
      exit 1
      ;;
  esac

  mkdir -p "$DIST_DIR/$abi"
  cp "$ROOT_DIR/target/$target/release/libtermirust_mobile_ffi.so" "$DIST_DIR/$abi/"
done

printf 'Built Android JNI libraries in %s\n' "$DIST_DIR"
