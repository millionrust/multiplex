#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BASE_SIMULATOR_ID=${TERMIRUST_IOS_SIMULATOR_ID:-7F76A1D5-5CC3-44DD-8883-DA554B851C99}
PORT=${TERMIRUST_RELAY_TEST_PORT:-48787}
FIXTURE=$(mktemp -d "${TMPDIR:-/tmp}/termirust-relay-mobile.XXXXXX")
SERVER_PID=
HOST_PID=
TEST_SIMULATOR_ID=

cleanup() {
    [ -z "$HOST_PID" ] || kill "$HOST_PID" >/dev/null 2>&1 || true
    [ -z "$SERVER_PID" ] || kill "$SERVER_PID" >/dev/null 2>&1 || true
    [ -z "$TEST_SIMULATOR_ID" ] || xcrun simctl delete "$TEST_SIMULATOR_ID" >/dev/null 2>&1 || true
    find "$FIXTURE" -depth -delete >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

for command in cargo openssl xcodebuild xcodegen xcrun; do
    command -v "$command" >/dev/null 2>&1 || {
        printf '%s\n' "$command is required for the native relay transport test." >&2
        exit 1
    }
done

if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    printf '%s\n' "Test port $PORT is already in use." >&2
    exit 1
fi

cat > "$FIXTURE/server.ext" <<'EOF'
subjectAltName=IP:127.0.0.1
extendedKeyUsage=serverAuth
keyUsage=digitalSignature,keyEncipherment
EOF
openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
    -keyout "$FIXTURE/ca.key" -out "$FIXTURE/ca.pem" \
    -subj '/CN=TermiRust Relay Test CA' >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes \
    -keyout "$FIXTURE/server.key" -out "$FIXTURE/server.csr" \
    -subj '/CN=127.0.0.1' >/dev/null 2>&1
openssl x509 -req -days 1 -in "$FIXTURE/server.csr" \
    -CA "$FIXTURE/ca.pem" -CAkey "$FIXTURE/ca.key" -CAcreateserial \
    -extfile "$FIXTURE/server.ext" -out "$FIXTURE/server.pem" >/dev/null 2>&1
openssl x509 -in "$FIXTURE/ca.pem" -outform der -out "$FIXTURE/ca.der"
cat "$FIXTURE/server.pem" "$FIXTURE/ca.pem" > "$FIXTURE/server-chain.pem"
PIN=$(openssl x509 -in "$FIXTURE/server.pem" -pubkey -noout \
    | openssl pkey -pubin -outform der \
    | openssl dgst -sha256 -binary \
    | openssl base64 -A)

cd "$ROOT_DIR"
cargo build -p termirust-relay-server --bin termirust-relay --locked
"$ROOT_DIR/target/debug/termirust-relay" provision \
    --state "$FIXTURE/state/relay.json" \
    --endpoint "wss://127.0.0.1:$PORT/relay/v1" \
    --spki-pin "sha256/$PIN" \
    --output-dir "$FIXTURE/packages" >/dev/null
TERMIRUST_RELAY_TEST_DIAGNOSTICS=1 "$ROOT_DIR/target/debug/termirust-relay" run \
    --state "$FIXTURE/state/relay.json" \
    --bind "127.0.0.1:$PORT" \
    --cert "$FIXTURE/server-chain.pem" \
    --key "$FIXTURE/server.key" >"$FIXTURE/server.log" 2>&1 &
SERVER_PID=$!
sleep 1
kill -0 "$SERVER_PID" 2>/dev/null || {
    printf '%s\n' 'Relay fixture failed to start.' >&2
    exit 1
}

cargo run -p termirust-relay-client --example relay_echo_host \
    --features test-support --locked -- \
    "$FIXTURE/packages/host-route.json" "$FIXTURE/ca.der" 33 >"$FIXTURE/host.log" 2>&1 &
HOST_PID=$!

TEST_SIMULATOR_ID=$(xcrun simctl clone "$BASE_SIMULATOR_ID" "TermiRust Relay Test $$")
xcrun simctl boot "$TEST_SIMULATOR_ID" >/dev/null 2>&1 || true
xcrun simctl bootstatus "$TEST_SIMULATOR_ID" -b >/dev/null
xcrun simctl keychain "$TEST_SIMULATOR_ID" add-root-cert "$FIXTURE/ca.pem"
export TERMIRUST_MOBILE_RELAY_PACKAGE
TERMIRUST_MOBILE_RELAY_PACKAGE=$(base64 < "$FIXTURE/packages/controller-route.json" | tr -d '\n')
xcrun simctl spawn "$TEST_SIMULATOR_ID" launchctl setenv \
    TERMIRUST_MOBILE_RELAY_PACKAGE "$TERMIRUST_MOBILE_RELAY_PACKAGE"

cd "$ROOT_DIR/mobile/ios"
xcodegen generate --spec project.yml >/dev/null
if ! xcodebuild \
    -project TermiRustMobile.xcodeproj \
    -scheme TermiRustMobile \
    -destination "platform=iOS Simulator,id=$TEST_SIMULATOR_ID" \
    -derivedDataPath "$FIXTURE/derived" \
    -only-testing:TermiRustMobileTests/AppleRelayControllerTransportLiveTests \
    test CODE_SIGNING_ALLOWED=NO \
    -test-timeouts-enabled YES \
    -default-test-execution-time-allowance 60 >"$FIXTURE/xcode.log" 2>&1; then
    tail -n 120 "$FIXTURE/xcode.log" >&2
    tail -n 40 "$FIXTURE/server.log" >&2
    tail -n 40 "$FIXTURE/host.log" >&2
    exit 1
fi
if grep -q 'Executed 0 tests' "$FIXTURE/xcode.log"; then
    printf '%s\n' 'Xcode executed zero relay transport tests.' >&2
    exit 1
fi
if grep -Eq "Test Case '.*' skipped|with [1-9][0-9]* test(s)? skipped" "$FIXTURE/xcode.log"; then
    printf '%s\n' 'The live relay transport test was skipped.' >&2
    tail -n 80 "$FIXTURE/xcode.log" >&2
    exit 1
fi

attempt=0
while kill -0 "$HOST_PID" >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 10 ]; then
        printf '%s\n' 'Relay Host fixture did not observe both controller connections.' >&2
        tail -n 80 "$FIXTURE/xcode.log" >&2
        tail -n 40 "$FIXTURE/host.log" >&2
        exit 1
    fi
    sleep 1
done
if ! wait "$HOST_PID"; then
    tail -n 40 "$FIXTURE/server.log" >&2
    tail -n 40 "$FIXTURE/host.log" >&2
    exit 1
fi
HOST_PID=
printf '%s\n' 'Native iOS self-hosted relay transport and reconnect passed.'
