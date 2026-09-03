#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/mobile/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/mobile/android}"
IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-platform=iOS Simulator,name=iPhone 17 Pro}"
MODE="local"
CURRENT_PID=""
STEP_INDEX=0
TEMP_DIR=""
LAST_LOG=""

usage() {
  cat <<'USAGE'
Usage: scripts/verify-product-model.sh [--local|--live]

  --local  Verify the deterministic Rust/Swift/Kotlin product model. Runtime-only
           dependencies are reported as explicit skips. This is the default.
  --live   Run the local baseline, require Docker for the desktop/Host golden run,
           then require eligible iOS and Android destinations for mobile and
           Controller smokes.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --local)
      MODE="local"
      shift
      ;;
    --live)
      MODE="live"
      shift
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

status_line() {
  printf '%s: %s\n' "$1" "$2"
}

cleanup() {
  if [[ -n "$TEMP_DIR" && -d "$TEMP_DIR" ]]; then
    rm -rf "$TEMP_DIR"
  fi
}

terminate_owned_child() {
  if [[ -n "$CURRENT_PID" ]] && kill -0 "$CURRENT_PID" 2>/dev/null; then
    pkill -TERM -P "$CURRENT_PID" 2>/dev/null || true
    kill -TERM "$CURRENT_PID" 2>/dev/null || true
    wait "$CURRENT_PID" 2>/dev/null || true
  fi
  CURRENT_PID=""
}

interrupted() {
  status_line "FAIL" "verification interrupted; owned child processes were cancelled"
  terminate_owned_child
  cleanup
  exit 130
}

trap interrupted HUP INT TERM
trap cleanup EXIT

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termirust-product-model.XXXXXX")"

print_failure_log() {
  tail -n 160 "$1" \
    | sed \
        -e "s|$ROOT_DIR|<rust-repository>|g" \
        -e "s|$IOS_DIR|<swift-repository>|g" \
        -e "s|$ANDROID_DIR|<kotlin-repository>|g" \
        -e "s|$HOME|<home>|g"
}

run_step() {
  local label="$1"
  shift
  STEP_INDEX=$((STEP_INDEX + 1))
  LAST_LOG="$TEMP_DIR/step-$STEP_INDEX.log"
  status_line "RUN" "$label"

  set +e
  "$@" >"$LAST_LOG" 2>&1 &
  CURRENT_PID=$!
  wait "$CURRENT_PID"
  local exit_code=$?
  CURRENT_PID=""
  set -e

  if [[ $exit_code -eq 0 ]]; then
    status_line "PASS" "$label"
    return 0
  fi

  status_line "FAIL" "$label (exit $exit_code)"
  print_failure_log "$LAST_LOG" >&2
  return "$exit_code"
}

require_file() {
  local path="$1"
  local label="$2"
  if [[ ! -f "$path" ]]; then
    status_line "FAIL" "$label is missing"
    exit 1
  fi
}

ios_runtime_available() {
  command -v xcrun >/dev/null 2>&1 || return 1
  xcrun simctl list devices available 2>/dev/null \
    | grep -Eq '^[[:space:]]+.+\([0-9A-Fa-f-]{8,}\)[[:space:]]+\((Booted|Shutdown)\)'
}

docker_available() {
  command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1
}

require_file "$IOS_DIR/scripts/verify-ios-unified-routes.sh" "iOS unified-route verifier"
require_file "$ANDROID_DIR/scripts/verify-android-unified-routes.sh" "Android unified-route verifier"
require_file "$ROOT_DIR/scripts/test-mobile-controller-ssh-transports.sh" "native mobile SSH Controller verifier"
require_file "$ROOT_DIR/scripts/test-mobile-controller-relay-transport.sh" "native iOS relay verifier"
require_file "$ROOT_DIR/scripts/test-mobile-android-relay-transport.sh" "native Android relay verifier"

cd "$ROOT_DIR"
status_line "MODE" "$MODE"

run_step "Rust workspace formatting, compile, Clippy, tests, docs, and policy" \
  "$ROOT_DIR/scripts/verify-rust.sh" workspace
run_step "shared terminal and route fixtures are synchronized" \
  "$ROOT_DIR/scripts/sync-terminal-conformance-fixture.sh" --check
run_step "Universal Session fixture is synchronized" \
  "$ROOT_DIR/scripts/sync-universal-session-fixture.sh" --check
run_step "remote route fixture contract" \
  python3 "$ROOT_DIR/scripts/verify-remote-route-acceptance.py"
run_step "mobile route capability contract" \
  python3 "$ROOT_DIR/scripts/verify-mobile-route-contract.py"
run_step "mobile cross-route acceptance contract" \
  python3 "$ROOT_DIR/scripts/verify-mobile-cross-route-acceptance.py"

run_step "iOS strict Swift 6 source and lifecycle verification" \
  "$IOS_DIR/scripts/verify-ios-unified-routes.sh"
if grep -Eq 'no eligible iOS destination|no eligible iOS runtime' "$LAST_LOG"; then
  status_line "SKIPPED" "iOS runtime execution (no eligible iOS destination is installed)"
else
  status_line "PASS" "iOS generic device build"
fi

run_step "Android unit tests and debug APK verification" \
  env GRADLE_OPTS="${GRADLE_OPTS:-} -Dorg.gradle.daemon=false" \
  "$ANDROID_DIR/scripts/verify-android-unified-routes.sh"
run_step "Rust repository diff hygiene" git diff --check
run_step "Swift repository diff hygiene" git -C "$IOS_DIR" diff --check
run_step "Kotlin repository diff hygiene" git -C "$ANDROID_DIR" diff --check

if [[ "$MODE" == "local" ]]; then
  if docker_available; then
    status_line "SKIPPED" "live Docker SSH fixtures (local mode; Docker is available)"
  else
    status_line "SKIPPED" "live Docker SSH fixtures (Docker daemon is unavailable)"
  fi
  if ios_runtime_available; then
    status_line "SKIPPED" "iOS runtime smokes (local mode; a destination is available)"
  else
    status_line "SKIPPED" "iOS runtime smokes (no eligible iOS destination is installed)"
  fi
  status_line "PASS" "deterministic local product-model baseline"
  exit 0
fi

if ! docker_available; then
  status_line "FAIL" "live preflight requires Docker; start Docker Desktop and wait for 'docker info' to succeed"
  exit 1
fi
status_line "PASS" "live preflight Docker daemon"

run_step "real bundled desktop and Host golden run" \
  "$ROOT_DIR/scripts/verify-desktop-host-golden-run.sh"

if ! ios_runtime_available; then
  status_line "FAIL" "live preflight requires an iOS runtime; install one in Xcode Settings > Components and set TERMIRUST_IOS_DESTINATION"
  exit 1
fi
status_line "PASS" "live preflight iOS destination"

run_step "real iOS direct SSH and tmux smoke" "$ROOT_DIR/scripts/test-mobile-ios-direct-ssh.sh"
run_step "real Android direct SSH and tmux smoke" \
  "$ROOT_DIR/scripts/test-mobile-android-direct-ssh.sh"
run_step "real Android Controller and Host golden run" \
  "$ROOT_DIR/scripts/test-mobile-android-controller-host.sh"
run_step "real native mobile SSH Controller transports" \
  "$ROOT_DIR/scripts/test-mobile-controller-ssh-transports.sh"
run_step "real native iOS self-hosted relay transport" \
  "$ROOT_DIR/scripts/test-mobile-controller-relay-transport.sh"
run_step "real native Android self-hosted relay transport" \
  "$ROOT_DIR/scripts/test-mobile-android-relay-transport.sh"
run_step "real private-network Controller route smoke" \
  "$ROOT_DIR/scripts/verify-controller-lan.sh"
run_step "real SSH Controller route smoke" "$ROOT_DIR/scripts/test-controller-ssh.sh"
status_line "PASS" "live product-model verification"
