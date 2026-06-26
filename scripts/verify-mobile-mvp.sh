#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-/Users/jacob/Projects/terminal_app/terminal_swift}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-/Users/jacob/Projects/terminal_app/terminal_kotlin}"
IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-platform=iOS Simulator,name=iPhone 17 Pro}"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"

run_live_ssh=false

usage() {
  cat <<'USAGE'
Usage: scripts/verify-mobile-mvp.sh [--live-ssh]

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

require_path "$IOS_DIR/TermiRustMobile.xcodeproj" "iOS project"
require_path "$ANDROID_DIR/gradlew" "Android Gradle wrapper"

cd "$ROOT_DIR"

run_step "Rust shared protocol tests" cargo test -p termirust-protocol
run_step "Rust mobile FFI tests" cargo test -p termirust-mobile-ffi
run_step "Mobile helper script syntax" bash -n \
  scripts/sync-mobile-ffi-artifacts.sh \
  scripts/test-mobile-ios-direct-ssh.sh \
  scripts/test-mobile-android-direct-ssh.sh

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
