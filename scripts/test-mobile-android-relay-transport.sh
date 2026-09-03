#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/mobile/android}"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
ADB="${ADB:-$ANDROID_HOME/platform-tools/adb}"
EMULATOR="${EMULATOR:-$ANDROID_HOME/emulator/emulator}"
PORT="${TERMIRUST_RELAY_TEST_PORT:-48787}"
PACKAGE_ASSET="$ANDROID_DIR/app/src/androidTest/assets/relay-live.json"
CA_ASSET="$ANDROID_DIR/app/src/androidTest/assets/relay-live-ca.txt"
FIXTURE=""
SERVER_PID=""
HOST_PID=""
EMULATOR_PID=""
SERIAL="${ANDROID_SERIAL:-}"
AVD=""
PACKAGE_BACKUP=""
CA_BACKUP=""
REPORT=""
EMULATOR_LOG=""

status_line() { printf '[%s] %s\n' "$1" "$2"; }
usage() { echo "usage: $0 [--avd name] [--serial device-serial]" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --avd) [[ $# -ge 2 ]] || { usage; exit 2; }; AVD="$2"; shift 2 ;;
    --serial) [[ $# -ge 2 ]] || { usage; exit 2; }; SERIAL="$2"; shift 2 ;;
    *) usage; exit 2 ;;
  esac
done

restore_assets() {
  if [[ -n "$PACKAGE_BACKUP" && -f "$PACKAGE_BACKUP" ]]; then
    cp -p "$PACKAGE_BACKUP" "$PACKAGE_ASSET"
    rm -f "$PACKAGE_BACKUP"
    PACKAGE_BACKUP=""
  fi
  if [[ -n "$CA_BACKUP" && -f "$CA_BACKUP" ]]; then
    cp -p "$CA_BACKUP" "$CA_ASSET"
    rm -f "$CA_BACKUP"
    CA_BACKUP=""
  fi
}

cleanup() {
  set +e
  restore_assets
  [[ -z "$SERIAL" ]] || "$ADB" -s "$SERIAL" reverse --remove "tcp:$PORT" >/dev/null 2>&1 || true
  [[ -z "$HOST_PID" ]] || kill "$HOST_PID" >/dev/null 2>&1 || true
  [[ -z "$SERVER_PID" ]] || kill "$SERVER_PID" >/dev/null 2>&1 || true
  if [[ -n "$EMULATOR_PID" ]]; then
    [[ -z "$SERIAL" ]] || "$ADB" -s "$SERIAL" emu kill >/dev/null 2>&1 || true
    wait "$EMULATOR_PID" >/dev/null 2>&1 || true
  fi
  [[ -z "$FIXTURE" ]] || rm -rf "$FIXTURE"
  [[ -z "$EMULATOR_LOG" ]] || rm -f "$EMULATOR_LOG"
}
trap cleanup EXIT INT TERM

for command in cargo openssl lsof base64; do
  command -v "$command" >/dev/null 2>&1 || { status_line FAIL "$command is required"; exit 1; }
done
[[ -x "$ADB" ]] || { status_line FAIL "adb not found under ANDROID_HOME"; exit 1; }
[[ -x "$ANDROID_DIR/gradlew" ]] || { status_line FAIL "Android Gradle wrapper is missing"; exit 1; }
[[ -f "$PACKAGE_ASSET" && -f "$CA_ASSET" ]] || {
  status_line FAIL "Android relay test placeholder assets are missing"
  exit 1
}
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  status_line FAIL "test port $PORT is already in use"
  exit 1
fi

if [[ -z "$SERIAL" ]]; then
  device_list="$("$ADB" devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
  device_count="$(printf '%s\n' "$device_list" | awk 'NF { count += 1 } END { print count + 0 }')"
  if [[ "$device_count" -eq 0 ]]; then
    [[ -x "$EMULATOR" ]] || { status_line FAIL "Android emulator executable is missing"; exit 1; }
    [[ -n "$AVD" ]] || AVD="$("$EMULATOR" -list-avds 2>/dev/null | awk 'NF { print; exit }')"
    [[ -n "$AVD" ]] || { status_line FAIL "no authorized device or installed AVD"; exit 1; }
    EMULATOR_LOG="$(mktemp "${TMPDIR:-/tmp}/termirust-relay-emulator.XXXXXX.log")"
    status_line RUN "starting owned Android emulator $AVD"
    "$EMULATOR" -avd "$AVD" -no-window -no-audio -no-boot-anim -no-snapshot-save \
      >"$EMULATOR_LOG" 2>&1 &
    EMULATOR_PID=$!
    for _ in {1..120}; do
      SERIAL="$("$ADB" devices | awk 'NR > 1 && $1 ~ /^emulator-/ && $2 == "device" { print $1; exit }')"
      [[ -z "$SERIAL" ]] || break
      kill -0 "$EMULATOR_PID" 2>/dev/null || {
        status_line FAIL "owned Android emulator exited before connecting"
        sed -n '1,60p' "$EMULATOR_LOG" >&2
        exit 1
      }
      sleep 1
    done
  elif [[ "$device_count" -eq 1 ]]; then
    SERIAL="$(printf '%s\n' "$device_list" | awk 'NF { print; exit }')"
  else
    status_line FAIL "multiple Android devices are connected; pass --serial"
    exit 1
  fi
fi

[[ -n "$SERIAL" ]] || { status_line FAIL "Android target did not connect"; exit 1; }
"$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 || {
  status_line FAIL "Android target $SERIAL is not authorized and online"
  exit 1
}
for _ in {1..180}; do
  [[ "$("$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == 1 ]] && break
  sleep 1
done
[[ "$("$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == 1 ]] || {
  status_line FAIL "Android target did not finish booting"
  exit 1
}
export ANDROID_HOME ANDROID_SERIAL="$SERIAL"

FIXTURE="$(mktemp -d "${TMPDIR:-/tmp}/termirust-relay-android.XXXXXX")"
cat >"$FIXTURE/server.ext" <<'EOF'
subjectAltName=IP:127.0.0.1
extendedKeyUsage=serverAuth
keyUsage=digitalSignature,keyEncipherment
EOF
openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -keyout "$FIXTURE/ca.key" -out "$FIXTURE/ca.pem" \
  -subj '/CN=TermiRust Android Relay Test CA' >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes \
  -keyout "$FIXTURE/server.key" -out "$FIXTURE/server.csr" \
  -subj '/CN=127.0.0.1' >/dev/null 2>&1
openssl x509 -req -days 1 -in "$FIXTURE/server.csr" \
  -CA "$FIXTURE/ca.pem" -CAkey "$FIXTURE/ca.key" -CAcreateserial \
  -extfile "$FIXTURE/server.ext" -out "$FIXTURE/server.pem" >/dev/null 2>&1
openssl x509 -in "$FIXTURE/ca.pem" -outform der -out "$FIXTURE/ca.der"
cat "$FIXTURE/server.pem" "$FIXTURE/ca.pem" >"$FIXTURE/server-chain.pem"
PIN="$(openssl x509 -in "$FIXTURE/server.pem" -pubkey -noout \
  | openssl pkey -pubin -outform der \
  | openssl dgst -sha256 -binary | openssl base64 -A)"

status_line RUN "building and provisioning the disposable Rust relay"
cd "$ROOT_DIR"
cargo build -p termirust-relay-server --bin termirust-relay --locked
"$ROOT_DIR/target/debug/termirust-relay" provision \
  --state "$FIXTURE/state/relay.json" \
  --endpoint "wss://127.0.0.1:$PORT/relay/v1" \
  --spki-pin "sha256/$PIN" \
  --output-dir "$FIXTURE/packages" >/dev/null
TERMIRUST_RELAY_TEST_DIAGNOSTICS=1 "$ROOT_DIR/target/debug/termirust-relay" run \
  --state "$FIXTURE/state/relay.json" --bind "127.0.0.1:$PORT" \
  --cert "$FIXTURE/server-chain.pem" --key "$FIXTURE/server.key" \
  >"$FIXTURE/server.log" 2>&1 &
SERVER_PID=$!
sleep 1
kill -0 "$SERVER_PID" 2>/dev/null || {
  status_line FAIL "relay fixture failed to start"
  sed -n '1,80p' "$FIXTURE/server.log" >&2
  exit 1
}
cargo run -p termirust-relay-client --example relay_echo_host \
  --features test-support --locked -- \
  "$FIXTURE/packages/host-route.json" "$FIXTURE/ca.der" 33 >"$FIXTURE/host.log" 2>&1 &
HOST_PID=$!

PACKAGE_BACKUP="$FIXTURE/relay-live.backup.json"
CA_BACKUP="$FIXTURE/relay-live-ca.backup.txt"
cp -p "$PACKAGE_ASSET" "$PACKAGE_BACKUP"
cp -p "$CA_ASSET" "$CA_BACKUP"
cp "$FIXTURE/packages/controller-route.json" "$PACKAGE_ASSET"
base64 <"$FIXTURE/ca.der" | tr -d '\n' >"$CA_ASSET"

status_line RUN "building Android production and relay instrumentation APKs"
cd "$ANDROID_DIR"
./gradlew :app:assembleDebug :app:assembleDebugAndroidTest --console=plain
restore_assets
APP_APK="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
TEST_APK="$ANDROID_DIR/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
[[ -f "$APP_APK" && -f "$TEST_APK" ]] || {
  status_line FAIL "Android production or instrumentation APK is missing"
  exit 1
}

status_line RUN "installing APKs and routing device loopback to the relay"
"$ADB" -s "$SERIAL" install -r "$APP_APK" >/dev/null
"$ADB" -s "$SERIAL" install -r "$TEST_APK" >/dev/null
"$ADB" -s "$SERIAL" reverse "tcp:$PORT" "tcp:$PORT" >/dev/null
REPORT="$FIXTURE/instrumentation.log"
status_line RUN "executing native Android relay echo and reconnect"
set +e
"$ADB" -s "$SERIAL" shell am instrument -w -r \
  -e class com.termirust.mobile.controller.RelayControllerTransportInstrumentedTest \
  com.termirust.mobile.test/androidx.test.runner.AndroidJUnitRunner | tee "$REPORT"
instrument_status=${PIPESTATUS[0]}
set -e
if [[ $instrument_status -ne 0 ]] || ! grep -q 'OK (1 test)' "$REPORT"; then
  status_line FAIL "Android relay transport instrumentation failed"
  tail -n 80 "$REPORT" >&2
  tail -n 40 "$FIXTURE/server.log" >&2
  tail -n 40 "$FIXTURE/host.log" >&2
  exit 1
fi

for _ in {1..100}; do
  kill -0 "$HOST_PID" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$HOST_PID" 2>/dev/null; then
  status_line FAIL "relay Host did not observe both Android connections"
  tail -n 40 "$FIXTURE/host.log" >&2
  exit 1
fi
if ! wait "$HOST_PID"; then
  HOST_PID=""
  status_line FAIL "relay Host fixture failed"
  tail -n 40 "$FIXTURE/host.log" >&2
  exit 1
fi
HOST_PID=""
status_line PASS "native Android self-hosted relay transport and reconnect completed"
