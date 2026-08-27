#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="termirust-controller-bindings"
LIB_STEM="termirust_controller_bindings"
OUTPUT_DIR="$ROOT_DIR/dist/mobile/controller"
BUILD_IOS=0
BUILD_ANDROID=0
PINNED_RUST_VERSION="1.97.1"
UNIFFI_VERSION="0.32.0"
PINNED_XCODE_VERSION="26.6"
PINNED_IOS_SDK_VERSION="26.5"
PINNED_ANDROID_NDK_VERSION="27.0.12077973"

usage() {
  cat <<'USAGE'
Usage: scripts/build-mobile-controller-bindings.sh [--clean] [--all|--ios|--android] [--output DIR]

Builds pinned UniFFI 0.32.0 Swift and Kotlin Controller-security bindings in
isolated temporary target directories. Output is promoted only after every
requested artifact and checksum succeeds.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clean)
      # Every invocation already uses a new isolated build root.
      shift
      ;;
    --all)
      BUILD_IOS=1
      BUILD_ANDROID=1
      shift
      ;;
    --ios)
      BUILD_IOS=1
      shift
      ;;
    --android)
      BUILD_ANDROID=1
      shift
      ;;
    --output)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      OUTPUT_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$BUILD_IOS" -eq 0 && "$BUILD_ANDROID" -eq 0 ]]; then
  BUILD_IOS=1
  BUILD_ANDROID=1
fi

cd "$ROOT_DIR"
[[ "$(rustc --version)" == "rustc $PINNED_RUST_VERSION "* ]] || {
  printf 'Expected rustc %s, found %s.\n' "$PINNED_RUST_VERSION" "$(rustc --version)" >&2
  exit 1
}
if [[ "$BUILD_IOS" -eq 1 ]]; then
  [[ "$(xcodebuild -version | head -1)" == "Xcode $PINNED_XCODE_VERSION" ]] || {
    printf 'Expected Xcode %s.\n' "$PINNED_XCODE_VERSION" >&2
    exit 1
  }
  [[ "$(xcrun --sdk iphoneos --show-sdk-version)" == "$PINNED_IOS_SDK_VERSION" ]] || {
    printf 'Expected iOS SDK %s.\n' "$PINNED_IOS_SDK_VERSION" >&2
    exit 1
  }
fi
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termirust-controller-bindings.XXXXXX")"
trap 'rm -rf "$BUILD_ROOT"' EXIT
GENERATED="$BUILD_ROOT/generated"
STAGED="$BUILD_ROOT/output"
mkdir -p "$GENERATED" "$STAGED"

HOST_TARGET="$BUILD_ROOT/host-target"
CARGO_TARGET_DIR="$HOST_TARGET" cargo build --locked -p "$CRATE" --release \
  --features bindgen-cli --lib --bin uniffi-bindgen

"$HOST_TARGET/release/uniffi-bindgen" generate \
  "$HOST_TARGET/release/lib${LIB_STEM}.dylib" \
  --language swift \
  --language kotlin \
  --out-dir "$GENERATED" \
  --no-format

normalize_generated_text() {
  local source="$1"
  local normalized="$source.normalized"
  awk '
    {
      sub(/[[:space:]]+$/, "")
      lines[NR] = $0
      if ($0 != "") {
        last = NR
      }
    }
    END {
      for (line = 1; line <= last; line++) {
        print lines[line]
      }
    }
  ' "$source" > "$normalized"
  mv "$normalized" "$source"
}

while IFS= read -r generated_text; do
  normalize_generated_text "$generated_text"
done < <(find "$GENERATED" -type f \( -name '*.h' -o -name '*.kt' -o -name '*.modulemap' -o -name '*.swift' \) | LC_ALL=C sort)

[[ "$("$HOST_TARGET/release/uniffi-bindgen" --version)" == "uniffi-bindgen $UNIFFI_VERSION" ]] || {
  printf 'Expected uniffi-bindgen %s.\n' "$UNIFFI_VERSION" >&2
  exit 1
}

HOST_RESOURCE_PREFIX="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
case "$HOST_RESOURCE_PREFIX" in
  darwin-arm64) HOST_RESOURCE_PREFIX="darwin-aarch64" ;;
  darwin-x86_64) HOST_RESOURCE_PREFIX="darwin-x86-64" ;;
esac
mkdir -p "$STAGED/kotlin-test/$HOST_RESOURCE_PREFIX"
cp "$HOST_TARGET/release/lib${LIB_STEM}.dylib" \
  "$STAGED/kotlin-test/$HOST_RESOURCE_PREFIX/"

nm -gj "$HOST_TARGET/release/lib${LIB_STEM}.dylib" \
  | sed 's/^_//' \
  | LC_ALL=C sort \
  | grep -E "^(ffi_${LIB_STEM}|uniffi_${LIB_STEM})" \
  > "$STAGED/abi-symbols-v1.txt"

build_ios() {
  local targets=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
  local libraries="$BUILD_ROOT/ios-libraries"
  local headers="$BUILD_ROOT/ios-headers"
  mkdir -p "$libraries" "$headers" "$STAGED/ios/Sources"
  cp "$GENERATED/TermiRustControllerSecurityFFI.h" "$headers/"
  cp "$GENERATED/TermiRustControllerSecurityFFI.modulemap" "$headers/module.modulemap"
  cp "$GENERATED/TermiRustControllerSecurity.swift" "$STAGED/ios/Sources/"

  for target in "${targets[@]}"; do
    rustup target add "$target"
    local target_dir="$BUILD_ROOT/ios-$target"
    CARGO_TARGET_DIR="$target_dir" cargo build --locked -p "$CRATE" --release --lib --target "$target"
    cp "$target_dir/$target/release/lib${LIB_STEM}.a" "$libraries/$target.a"
    rm -rf "$target_dir"
  done

  lipo -create \
    "$libraries/aarch64-apple-ios-sim.a" \
    "$libraries/x86_64-apple-ios.a" \
    -output "$libraries/simulator.a"

  xcodebuild -create-xcframework \
    -library "$libraries/aarch64-apple-ios.a" \
    -headers "$headers" \
    -library "$libraries/simulator.a" \
    -headers "$headers" \
    -output "$STAGED/ios/TermiRustControllerSecurity.xcframework"

  cat > "$STAGED/ios/TermiRustControllerSecurity.xcframework/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AvailableLibraries</key>
	<array>
		<dict>
			<key>BinaryPath</key>
			<string>aarch64-apple-ios.a</string>
			<key>HeadersPath</key>
			<string>Headers</string>
			<key>LibraryIdentifier</key>
			<string>ios-arm64</string>
			<key>LibraryPath</key>
			<string>aarch64-apple-ios.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>ios</string>
		</dict>
		<dict>
			<key>BinaryPath</key>
			<string>simulator.a</string>
			<key>HeadersPath</key>
			<string>Headers</string>
			<key>LibraryIdentifier</key>
			<string>ios-arm64_x86_64-simulator</string>
			<key>LibraryPath</key>
			<string>simulator.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
				<string>x86_64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>ios</string>
			<key>SupportedPlatformVariant</key>
			<string>simulator</string>
		</dict>
	</array>
	<key>CFBundlePackageType</key>
	<string>XFWK</string>
	<key>XCFrameworkFormatVersion</key>
	<string>1.0</string>
</dict>
</plist>
PLIST
}

build_android() {
  local android_api="${ANDROID_API:-26}"
  local android_sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  local android_ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
  if [[ -z "$android_ndk" ]]; then
    android_ndk="$(find "$android_sdk/ndk" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -1)"
  fi
  [[ -d "$android_ndk" ]] || {
    printf 'Android NDK not found. Set ANDROID_NDK_HOME or install it under %s/ndk.\n' "$android_sdk" >&2
    exit 1
  }
  [[ "$(basename "$android_ndk")" == "$PINNED_ANDROID_NDK_VERSION" ]] || {
    printf 'Expected Android NDK %s, found %s.\n' \
      "$PINNED_ANDROID_NDK_VERSION" "$(basename "$android_ndk")" >&2
    exit 1
  }

  local host_tag="darwin-x86_64"
  local toolchain="$android_ndk/toolchains/llvm/prebuilt/$host_tag/bin"
  local readelf="$toolchain/llvm-readelf"
  [[ -x "$readelf" ]] || { printf 'Android llvm-readelf missing at %s.\n' "$readelf" >&2; exit 1; }

  local targets=(aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android)
  mkdir -p "$STAGED/android/kotlin/com/termirust/controller/security"
  cp "$GENERATED/com/termirust/controller/security/${LIB_STEM}.kt" \
    "$STAGED/android/kotlin/com/termirust/controller/security/"

  export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/aarch64-linux-android${android_api}-clang"
  export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$toolchain/armv7a-linux-androideabi${android_api}-clang"
  export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$toolchain/i686-linux-android${android_api}-clang"
  export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$toolchain/x86_64-linux-android${android_api}-clang"

  for target in "${targets[@]}"; do
    rustup target add "$target"
    local abi
    case "$target" in
      aarch64-linux-android) abi="arm64-v8a" ;;
      armv7-linux-androideabi) abi="armeabi-v7a" ;;
      i686-linux-android) abi="x86" ;;
      x86_64-linux-android) abi="x86_64" ;;
    esac
    local target_dir="$BUILD_ROOT/android-$target"
    RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384" \
      CARGO_TARGET_DIR="$target_dir" cargo build --locked -p "$CRATE" --release --lib --target "$target"
    mkdir -p "$STAGED/android/jniLibs/$abi"
    cp "$target_dir/$target/release/lib${LIB_STEM}.so" "$STAGED/android/jniLibs/$abi/"
    while read -r alignment; do
      if (( alignment < 0x4000 )); then
        printf '%s LOAD alignment %s is below 0x4000.\n' "$target" "$alignment" >&2
        exit 1
      fi
    done < <("$readelf" -lW "$STAGED/android/jniLibs/$abi/lib${LIB_STEM}.so" | awk '$1 == "LOAD" { print $NF }')
    rm -rf "$target_dir"
  done
}

if [[ "$BUILD_IOS" -eq 1 ]]; then
  build_ios
fi
if [[ "$BUILD_ANDROID" -eq 1 ]]; then
  build_android
fi

cat > "$STAGED/provenance-v1.txt" <<'PROVENANCE'
schema=termirust-controller-bindings-provenance-v1
uniffi=0.32.0
rust=1.97.1
xcode=26.6
ios_sdk=26.5
swift_target=aarch64-apple-ios
swift_simulator_targets=aarch64-apple-ios-sim,x86_64-apple-ios
android_targets=aarch64-linux-android,armv7-linux-androideabi,i686-linux-android,x86_64-linux-android
android_api=26
android_ndk=27.0.12077973
android_page_alignment=16384
kotlin_ffi=jna
jna=5.17.0
PROVENANCE

(
  cd "$STAGED"
  find . -type f ! -path './kotlin-test/*' ! -name artifacts.sha256 \
    | LC_ALL=C sort | while read -r path; do
    shasum -a 256 "$path"
  done
) > "$STAGED/artifacts.sha256"

PARENT_DIR="$(dirname "$OUTPUT_DIR")"
mkdir -p "$PARENT_DIR"
NEXT_DIR="$(mktemp -d "$PARENT_DIR/.controller-next.XXXXXX")"
cp -R "$STAGED/." "$NEXT_DIR/"
BACKUP_DIR="$PARENT_DIR/.controller-previous.$$"
if [[ -e "$OUTPUT_DIR" ]]; then
  mv "$OUTPUT_DIR" "$BACKUP_DIR"
fi
if mv "$NEXT_DIR" "$OUTPUT_DIR"; then
  rm -rf "$BACKUP_DIR"
else
  [[ ! -e "$BACKUP_DIR" ]] || mv "$BACKUP_DIR" "$OUTPUT_DIR"
  exit 1
fi

printf 'Built Controller bindings at %s\n' "$OUTPUT_DIR"
