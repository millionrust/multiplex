#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CONTAINER_NAME="termirust-mobile-controller-ssh-$$"
IMAGE_NAME="termirust-mobile-controller-ssh:local"
SIMULATOR_ID=${TERMIRUST_IOS_SIMULATOR_ID:-7F76A1D5-5CC3-44DD-8883-DA554B851C99}

command -v docker >/dev/null 2>&1 || {
    printf '%s\n' 'Docker is required for the native mobile SSH Controller transport test.' >&2
    exit 1
}
docker info >/dev/null 2>&1 || {
    printf '%s\n' 'Docker is installed but not running.' >&2
    exit 1
}

cleanup() {
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

cd "$ROOT_DIR"
docker build -t "$IMAGE_NAME" tests/fixtures/ssh-server >/dev/null
docker run --detach --rm --name "$CONTAINER_NAME" -p 127.0.0.1::22 "$IMAGE_NAME" >/dev/null
PORT=$(docker port "$CONTAINER_NAME" 22/tcp | sed 's/.*://')

attempt=0
until docker exec "$CONTAINER_NAME" sh -c 'test -f /etc/ssh/ssh_host_ed25519_key.pub'; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 30 ] || {
        printf '%s\n' 'SSH fixture did not become ready.' >&2
        exit 1
    }
    sleep 1
done

export TERMIRUST_MOBILE_CONTROLLER_SSH_PORT="$PORT"
export TERMIRUST_MOBILE_CONTROLLER_SSH_HOST_KEY
TERMIRUST_MOBILE_CONTROLLER_SSH_HOST_KEY=$(docker exec "$CONTAINER_NAME" cat /etc/ssh/ssh_host_ed25519_key.pub)
export TERMIRUST_MOBILE_CONTROLLER_SSH_PRIVATE_KEY
TERMIRUST_MOBILE_CONTROLLER_SSH_PRIVATE_KEY=$(cat tests/fixtures/ssh-server/id_ed25519)
export TERMIRUST_MOBILE_CONTROLLER_SSH_PASSWORD='termirust-pass'

ANDROID_HOME=${ANDROID_HOME:-"$HOME/Library/Android/sdk"}
export ANDROID_HOME
(cd mobile/android && ./gradlew \
    :app:testDebugUnitTest \
    --tests com.termirust.mobile.controller.AndroidSSHControllerTransportLiveTest \
    --no-daemon)

(cd mobile/ios && xcodegen generate >/dev/null && xcodebuild \
    -project TermiRustMobile.xcodeproj \
    -scheme TermiRustMobile \
    -destination "platform=iOS Simulator,id=$SIMULATOR_ID" \
    -only-testing:TermiRustMobileTests/AppleSSHControllerTransportLiveTests \
    test CODE_SIGNING_ALLOWED=NO -quiet)

printf '%s\n' 'Native Android and iOS SSH Controller transports passed.'
