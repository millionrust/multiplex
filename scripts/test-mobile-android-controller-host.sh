#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/mobile/android}"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
ADB="${ADB:-$ANDROID_HOME/platform-tools/adb}"
EMULATOR="${EMULATOR:-$ANDROID_HOME/emulator/emulator}"
RESOURCE_PATH="$ANDROID_DIR/app/src/androidTest/assets/controller-live.json"
FIXTURE_BINARY="$ROOT_DIR/target/debug/examples/mobile_controller_fixture"
HOST_BINARY="$ROOT_DIR/target/debug/termirust-session-host"
AVD=""
SERIAL="${ANDROID_SERIAL:-}"
FIXTURE_ROOT=""
FIXTURE_PID=""
FIXTURE_LOG=""
CONFIG_PATH=""
RESOURCE_BACKUP=""
EMULATOR_PID=""
EMULATOR_LOG=""
INSTRUMENTATION_REPORT=""
REVERSE_PORT=""

status_line() {
  printf '[%s] %s\n' "$1" "$2"
}

usage() {
  echo "usage: $0 [--avd name] [--serial device-serial]" >&2
  echo "Without an option, use the sole connected device or first installed AVD." >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --avd)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      AVD="$2"
      shift 2
      ;;
    --serial)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      SERIAL="$2"
      shift 2
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

restore_resource() {
  if [[ -n "$RESOURCE_BACKUP" && -f "$RESOURCE_BACKUP" ]]; then
    cp -p "$RESOURCE_BACKUP" "$RESOURCE_PATH"
    rm -f "$RESOURCE_BACKUP"
    RESOURCE_BACKUP=""
  fi
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

cleanup() {
  set +e
  restore_resource
  if [[ -n "$REVERSE_PORT" && -n "$SERIAL" ]]; then
    "$ADB" -s "$SERIAL" reverse --remove "tcp:$REVERSE_PORT" >/dev/null 2>&1 || true
  fi
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
  if [[ -n "$EMULATOR_PID" ]]; then
    if [[ -n "$SERIAL" ]]; then
      "$ADB" -s "$SERIAL" emu kill >/dev/null 2>&1 || true
    fi
    wait "$EMULATOR_PID" 2>/dev/null || true
  fi
  [[ -z "$CONFIG_PATH" ]] || rm -f "$CONFIG_PATH"
  if [[ -n "$FIXTURE_ROOT" && "$FIXTURE_ROOT" == /tmp/tri.* ]]; then
    rm -rf "$FIXTURE_ROOT"
  fi
  [[ -z "$FIXTURE_LOG" || "$FIXTURE_LOG" != /tmp/tri.*.log ]] || rm -f "$FIXTURE_LOG"
  [[ -z "$EMULATOR_LOG" || "$EMULATOR_LOG" != /tmp/tri.*.emulator.log ]] || rm -f "$EMULATOR_LOG"
  [[ -z "$INSTRUMENTATION_REPORT" || "$INSTRUMENTATION_REPORT" != /tmp/tri.*.instrumentation.log ]] || \
    rm -f "$INSTRUMENTATION_REPORT"
}
trap cleanup EXIT INT TERM

for command in cargo jq nc; do
  command -v "$command" >/dev/null 2>&1 || {
    status_line FAIL "$command is required"
    exit 1
  }
done
[[ -x "$ADB" ]] || { status_line FAIL "adb not found under ANDROID_HOME"; exit 1; }
[[ -x "$ANDROID_DIR/gradlew" ]] || { status_line FAIL "Android Gradle wrapper is missing"; exit 1; }
[[ -f "$RESOURCE_PATH" ]] || { status_line FAIL "Android live-test resource is missing"; exit 1; }

if [[ -z "$SERIAL" ]]; then
  device_list="$("$ADB" devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
  device_count="$(printf '%s\n' "$device_list" | awk 'NF { count += 1 } END { print count + 0 }')"
  if [[ "$device_count" -eq 0 ]]; then
    [[ -x "$EMULATOR" ]] || { status_line FAIL "Android emulator executable is missing"; exit 1; }
    if [[ -z "$AVD" ]]; then
      AVD="$("$EMULATOR" -list-avds 2>/dev/null | awk 'NF { print; exit }')"
    fi
    [[ -n "$AVD" ]] || {
      status_line FAIL "no authorized Android device or installed AVD; create an AVD or pass --serial"
      exit 1
    }
    EMULATOR_LOG="$(mktemp /tmp/tri.XXXXXX.emulator.log)"
    status_line RUN "starting owned Android emulator $AVD"
    "$EMULATOR" -avd "$AVD" -no-window -no-audio -no-boot-anim -no-snapshot-save \
      >"$EMULATOR_LOG" 2>&1 &
    EMULATOR_PID=$!
    for _ in {1..120}; do
      SERIAL="$("$ADB" devices | awk 'NR > 1 && $1 ~ /^emulator-/ && $2 == "device" { print $1; exit }')"
      [[ -z "$SERIAL" ]] || break
      kill -0 "$EMULATOR_PID" 2>/dev/null || {
        status_line FAIL "owned Android emulator exited before connecting"
        sed -n '1,40p' "$EMULATOR_LOG" >&2
        exit 1
      }
      sleep 1
    done
    [[ -n "$SERIAL" ]] || { status_line FAIL "Android emulator did not connect"; exit 1; }
  elif [[ "$device_count" -eq 1 ]]; then
    SERIAL="$(printf '%s\n' "$device_list" | awk 'NF { print; exit }')"
  elif [[ "$device_count" -gt 1 ]]; then
    status_line FAIL "multiple Android devices are connected; pass --serial"
    exit 1
  fi
fi

[[ -n "$SERIAL" ]] || {
  status_line FAIL "no authorized Android device; connect one or pass --avd with an installed AVD"
  exit 1
}
"$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 || {
  status_line FAIL "Android target $SERIAL is not authorized and online"
  exit 1
}
for _ in {1..180}; do
  if [[ "$("$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == 1 ]]; then
    break
  fi
  sleep 1
done
[[ "$("$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == 1 ]] || {
  status_line FAIL "Android target did not finish booting"
  exit 1
}
export ANDROID_HOME
export ANDROID_SERIAL="$SERIAL"

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
  [[ ! -f "$CONFIG_PATH" ]] || break
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    status_line FAIL "Controller fixture exited before readiness"
    sed -n '1,40p' "$FIXTURE_LOG" >&2
    exit 1
  fi
  sleep 0.05
done
[[ -f "$CONFIG_PATH" ]] || { status_line FAIL "Controller fixture did not become ready"; exit 1; }

RESOURCE_BACKUP="${FIXTURE_ROOT}.resource-backup.json"
cp -p "$RESOURCE_PATH" "$RESOURCE_BACKUP"
REVERSE_PORT="$(jq -r '.control_port' "$CONFIG_PATH")"
if [[ "$SERIAL" == emulator-* ]]; then
  jq '.control_address = "10.0.2.2"' "$CONFIG_PATH" >"$RESOURCE_PATH"
  REVERSE_PORT=""
else
  "$ADB" -s "$SERIAL" reverse "tcp:$REVERSE_PORT" "tcp:$REVERSE_PORT" >/dev/null
  jq '.control_address = "127.0.0.1"' "$CONFIG_PATH" >"$RESOURCE_PATH"
fi

status_line RUN "building Android production and instrumentation APKs"
cd "$ANDROID_DIR"
./gradlew :app:assembleDebug :app:assembleDebugAndroidTest --console=plain
restore_resource

APP_APK="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
TEST_APK="$ANDROID_DIR/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
[[ -f "$APP_APK" && -f "$TEST_APK" ]] || {
  status_line FAIL "Android production or instrumentation APK is missing"
  exit 1
}

status_line RUN "installing the Android test build on $SERIAL"
"$ADB" -s "$SERIAL" install -r "$APP_APK" >/dev/null
"$ADB" -s "$SERIAL" install -r "$TEST_APK" >/dev/null

INSTRUMENTATION_REPORT="$(mktemp /tmp/tri.XXXXXX.instrumentation.log)"
status_line RUN "pairing Android with the real Rust Host and exercising terminal lifecycle"
set +e
"$ADB" -s "$SERIAL" shell am instrument -w -r \
  -e class com.termirust.mobile.controller.LiveRustControllerGoldenTest \
  com.termirust.mobile.test/androidx.test.runner.AndroidJUnitRunner | tee "$INSTRUMENTATION_REPORT"
instrument_status=${PIPESTATUS[0]}
set -e
if [[ $instrument_status -ne 0 ]] || ! grep -q 'OK (1 test)' "$INSTRUMENTATION_REPORT"; then
  status_line FAIL "Android Controller instrumentation golden run failed"
  sed -n '1,80p' "$FIXTURE_LOG" >&2
  exit 1
fi
rm -f "$INSTRUMENTATION_REPORT"
INSTRUMENTATION_REPORT=""

shutdown_fixture
for _ in {1..50}; do
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    wait "$FIXTURE_PID"
    FIXTURE_PID=""
    break
  fi
  sleep 0.1
done
[[ -z "$FIXTURE_PID" ]] || { status_line FAIL "Controller fixture did not stop cleanly"; exit 1; }

status_line PASS "real Android Controller pairing, terminal lifecycle, reconnect, and revocation completed"
