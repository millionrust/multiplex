#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/mobile/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/mobile/android}"
IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-platform=iOS Simulator,name=iPhone 17 Pro}"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"

run_live_ssh=false
run_live_controller=false

usage() {
  cat <<'USAGE'
Usage: scripts/verify-mobile-mvp.sh [--live-ssh] [--live-controller]

Runs the local mobile MVP verification gates:
  - Rust shared protocol and mobile FFI tests
  - Mobile script syntax checks
  - iOS unit/build tests
  - Android unit tests and debug build

Use --live-ssh to also run the iOS and Android direct SSH/tmux smoke tests.
Those smoke tests require either Docker Desktop to be running or these env vars:
  TERMIRUST_MOBILE_TEST_SSH_HOST
  TERMIRUST_MOBILE_TEST_SSH_PORT
  TERMIRUST_MOBILE_TEST_SSH_USER
  TERMIRUST_MOBILE_TEST_SSH_KEY
  TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY

Use --live-controller to run the real Rust Host/Controller lifecycle against
eligible iOS and Android simulator/device destinations.

Path overrides:
  TERMIRUST_IOS_DIR
  TERMIRUST_ANDROID_DIR
  TERMIRUST_IOS_DESTINATION
  ANDROID_HOME
  TERMIRUST_MOBILE_TEST_SSH_IMAGE
  TERMIRUST_MOBILE_REBUILD_SSH_IMAGE=1
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --live-ssh)
      run_live_ssh=true
      shift
      ;;
    --live-controller)
      run_live_controller=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 1
      ;;
  esac
done

require_path() {
  local path="$1"
  local label="$2"
  if [[ ! -e "$path" ]]; then
    echo "$label not found: $path" >&2
    exit 1
  fi
}

run_step() {
  local label="$1"
  shift
  printf '\n==> %s\n' "$label"
  "$@"
}

prepare_android_test_native() {
  local host_os host_arch resource_dir library_name
  host_os="$(uname -s)"
  host_arch="$(uname -m)"

  case "$host_os-$host_arch" in
    Darwin-arm64) resource_dir="darwin-aarch64"; library_name="libtermirust_controller_bindings.dylib" ;;
    Darwin-x86_64) resource_dir="darwin-x86-64"; library_name="libtermirust_controller_bindings.dylib" ;;
    Linux-aarch64|Linux-arm64) resource_dir="linux-aarch64"; library_name="libtermirust_controller_bindings.so" ;;
    Linux-x86_64) resource_dir="linux-x86-64"; library_name="libtermirust_controller_bindings.so" ;;
    *)
      echo "Unsupported Android unit-test host: $host_os $host_arch" >&2
      return 1
      ;;
  esac

  cargo build --locked -p termirust-controller-bindings --release
  mkdir -p "$ANDROID_DIR/app/src/test/native/$resource_dir"
  cp "$ROOT_DIR/target/release/$library_name" \
    "$ANDROID_DIR/app/src/test/native/$resource_dir/$library_name"
}

require_path "$IOS_DIR/TermiRustMobile.xcodeproj" "iOS project"
require_path "$ANDROID_DIR/gradlew" "Android Gradle wrapper"

cd "$ROOT_DIR"

run_step "Rust shared protocol tests" cargo test -p termirust-protocol
run_step "Rust mobile FFI tests" cargo test -p termirust-mobile-ffi
run_step "Android host Controller binding" prepare_android_test_native
run_step "Mobile helper script syntax" bash -n \
  scripts/sync-mobile-ffi-artifacts.sh \
  scripts/test-mobile-ios-direct-ssh.sh \
  scripts/test-mobile-android-direct-ssh.sh \
  scripts/test-mobile-ios-controller-host.sh \
  scripts/test-mobile-android-controller-host.sh

run_step "iOS unit and build tests" \
  xcodebuild test \
    -project "$IOS_DIR/TermiRustMobile.xcodeproj" \
    -scheme TermiRustMobile \
    -destination "$IOS_DESTINATION" \
    -quiet

run_step "Android unit tests and debug build" \
  env ANDROID_HOME="$ANDROID_HOME" ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}" \
    "$ANDROID_DIR/gradlew" \
      -p "$ANDROID_DIR" \
      testDebugUnitTest \
      assembleDebug

if [[ "$run_live_ssh" == true ]]; then
  run_step "iOS direct SSH/tmux smoke" "$ROOT_DIR/scripts/test-mobile-ios-direct-ssh.sh"
  run_step "Android direct SSH/tmux smoke" "$ROOT_DIR/scripts/test-mobile-android-direct-ssh.sh"
else
  cat <<'NOTE'

Skipped live SSH/tmux smoke tests.
Run with --live-ssh after starting Docker Desktop, or set TERMIRUST_MOBILE_TEST_SSH_* env vars for a reachable SSH host.
NOTE
fi

if [[ "$run_live_controller" == true ]]; then
  run_step "iOS Controller/Host golden run" "$ROOT_DIR/scripts/test-mobile-ios-controller-host.sh"
  run_step "Android Controller/Host golden run" "$ROOT_DIR/scripts/test-mobile-android-controller-host.sh"
else
  cat <<'NOTE'

Skipped live Controller/Host golden runs.
Run with --live-controller when eligible iOS and Android destinations are available.
NOTE
fi
