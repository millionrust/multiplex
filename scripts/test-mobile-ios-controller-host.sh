#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/mobile/ios}"
RESOURCE_PATH="$IOS_DIR/TermiRustMobileTests/Fixtures/controller-v1.json"
FIXTURE_BINARY="$ROOT_DIR/target/debug/examples/mobile_controller_fixture"
HOST_BINARY="$ROOT_DIR/target/debug/termirust-session-host"
IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-}"
IOS_DEVELOPMENT_TEAM="${TERMIRUST_IOS_DEVELOPMENT_TEAM:-}"
FIXTURE_ROOT=""
FIXTURE_PID=""
FIXTURE_LOG=""
CONFIG_PATH=""
RESOURCE_BACKUP=""
XCODEBUILD_SIGNING_ARGS=()

status_line() {
  printf '[%s] %s\n' "$1" "$2"
}

shutdown_fixture() {
  if [[ -n "$CONFIG_PATH" && -f "$CONFIG_PATH" ]]; then
    local address port
    address="$(jq -r '.control_address // empty' "$CONFIG_PATH" 2>/dev/null || true)"
    port="$(jq -r '.control_port // empty' "$CONFIG_PATH" 2>/dev/null || true)"
    if [[ -n "$address" && -n "$port" ]]; then
      jq -c '{token:.control_token,command:"shutdown"}' "$CONFIG_PATH" 2>/dev/null \
        | nc -w 2 "$address" "$port" >/dev/null 2>&1 || true
    fi
  fi
}

restore_resource() {
  if [[ -n "$RESOURCE_BACKUP" && -f "$RESOURCE_BACKUP" ]]; then
    cp -p "$RESOURCE_BACKUP" "$RESOURCE_PATH"
    rm -f "$RESOURCE_BACKUP"
    RESOURCE_BACKUP=""
  fi
}

cleanup() {
  set +e
  restore_resource
  shutdown_fixture
  if [[ -n "$FIXTURE_PID" ]]; then
    for _ in {1..50}; do
      if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
        wait "$FIXTURE_PID" 2>/dev/null || true
        FIXTURE_PID=""
        break
      fi
      sleep 0.1
    done
  fi
  if [[ -n "$FIXTURE_PID" ]] && kill -0 "$FIXTURE_PID" 2>/dev/null; then
    kill "$FIXTURE_PID" 2>/dev/null || true
    wait "$FIXTURE_PID" 2>/dev/null || true
  fi
  if [[ -n "$CONFIG_PATH" ]]; then
    rm -f "$CONFIG_PATH"
  fi
  if [[ -n "$FIXTURE_ROOT" && "$FIXTURE_ROOT" == /tmp/tri.* ]]; then
    rm -rf "$FIXTURE_ROOT"
  fi
  if [[ -n "$FIXTURE_LOG" && "$FIXTURE_LOG" == /tmp/tri.*.log ]]; then
    rm -f "$FIXTURE_LOG"
  fi
}
trap cleanup EXIT INT TERM

[[ -d "$IOS_DIR/TermiRustMobile.xcodeproj" ]] || {
  status_line FAIL "iOS project not found at $IOS_DIR"
  exit 1
}
[[ -f "$RESOURCE_PATH" ]] || {
  status_line FAIL "declared iOS test resource not found at $RESOURCE_PATH"
  exit 1
}
for command in cargo jq nc xcodebuild xcrun; do
  command -v "$command" >/dev/null 2>&1 || {
    status_line FAIL "$command is required"
    exit 1
  }
done
if [[ -z "$IOS_DESTINATION" ]]; then
  simulator_id="$(
    xcrun simctl list devices available 2>/dev/null \
      | awk -F '[()]' '/iPhone/ { print $2; exit }'
  )"
  [[ -n "$simulator_id" ]] || {
    status_line FAIL "no available iPhone simulator; set TERMIRUST_IOS_DESTINATION for an eligible device"
    exit 1
  }
  IOS_DESTINATION="platform=iOS Simulator,id=$simulator_id"
fi

if [[ "$IOS_DESTINATION" == platform=iOS,* ]]; then
  if [[ -z "$IOS_DEVELOPMENT_TEAM" ]]; then
    IOS_DEVELOPMENT_TEAM="$({
      security find-identity -v -p codesigning 2>/dev/null || true
    } | sed -n 's/.*Apple Development:.*(\([[:alnum:]]\{10\}\)).*/\1/p' | head -1)"
  fi
  [[ -n "$IOS_DEVELOPMENT_TEAM" ]] || {
    status_line FAIL "physical iOS testing requires TERMIRUST_IOS_DEVELOPMENT_TEAM or an Apple Development signing identity"
    exit 1
  }
  XCODEBUILD_SIGNING_ARGS=(
    -allowProvisioningUpdates
    CODE_SIGN_STYLE=Automatic
    "DEVELOPMENT_TEAM=$IOS_DEVELOPMENT_TEAM"
  )
fi

status_line RUN "building the production Session Host and live Controller fixture"
cd "$ROOT_DIR"
cargo build -p termirust-session-host >/dev/null
cargo build -p termirust-controller-listener --example mobile_controller_fixture >/dev/null

FIXTURE_ROOT="$(mktemp -d /tmp/tri.XXXXXX)"
rmdir "$FIXTURE_ROOT"
FIXTURE_LOG="${FIXTURE_ROOT}.log"
CONFIG_PATH="${FIXTURE_ROOT}.json"
"$FIXTURE_BINARY" \
  --root "$FIXTURE_ROOT" \
  --session-host-bin "$HOST_BINARY" \
  --config "$CONFIG_PATH" >"$FIXTURE_LOG" 2>&1 &
FIXTURE_PID=$!

for _ in {1..100}; do
  if [[ -f "$CONFIG_PATH" ]]; then
    break
  fi
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    status_line FAIL "Controller fixture exited before readiness"
    sed -n '1,40p' "$FIXTURE_LOG" >&2
    exit 1
  fi
  sleep 0.05
done
[[ -f "$CONFIG_PATH" ]] || {
  status_line FAIL "Controller fixture did not become ready"
  exit 1
}

RESOURCE_BACKUP="${FIXTURE_ROOT}.resource-backup.json"
cp -p "$RESOURCE_PATH" "$RESOURCE_BACKUP"
cp "$CONFIG_PATH" "$RESOURCE_PATH"

status_line RUN "building iOS tests for $IOS_DESTINATION"
cd "$IOS_DIR"
xcodebuild build-for-testing -quiet \
  -project TermiRustMobile.xcodeproj \
  -scheme TermiRustMobile \
  -destination "$IOS_DESTINATION" \
  "${XCODEBUILD_SIGNING_ARGS[@]}"
restore_resource

status_line RUN "pairing iOS with the real Rust Host and exercising terminal lifecycle"
xcodebuild test-without-building -quiet \
  -project TermiRustMobile.xcodeproj \
  -scheme TermiRustMobile \
  -destination "$IOS_DESTINATION" \
  "${XCODEBUILD_SIGNING_ARGS[@]}" \
  -only-testing:TermiRustMobileTests/ControllerPairingFleetTests/testLiveRustControllerPairingTerminalLifecycleAndRevocation

shutdown_fixture
for _ in {1..50}; do
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    wait "$FIXTURE_PID"
    FIXTURE_PID=""
    break
  fi
  sleep 0.1
done
[[ -z "$FIXTURE_PID" ]] || {
  status_line FAIL "Controller fixture did not stop cleanly"
  exit 1
}

status_line PASS "real iOS Controller pairing, terminal lifecycle, and revocation completed"
