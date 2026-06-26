#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-/Users/jacob/Projects/terminal_app/terminal_kotlin}"

if [[ ! -x "$ANDROID_DIR/gradlew" ]]; then
  echo "Android project Gradle wrapper not found at $ANDROID_DIR/gradlew" >&2
  exit 1
fi

run_android_smoke() {
  cd "$ANDROID_DIR"
  ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}" \
  ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}" \
    ./gradlew testDebugUnitTest --tests com.termirust.mobile.DirectSshIntegrationTest --no-daemon
}

if [[ -n "${TERMIRUST_MOBILE_TEST_SSH_HOST:-}" ]]; then
  required=(
    TERMIRUST_MOBILE_TEST_SSH_PORT
    TERMIRUST_MOBILE_TEST_SSH_USER
    TERMIRUST_MOBILE_TEST_SSH_KEY
    TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY
  )
  for name in "${required[@]}"; do
    if [[ -z "${!name:-}" ]]; then
      echo "When TERMIRUST_MOBILE_TEST_SSH_HOST is set, $name is also required." >&2
      exit 1
    fi
  done
  run_android_smoke
  exit 0
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required unless TERMIRUST_MOBILE_TEST_SSH_* env vars point at a reachable SSH host." >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "Docker daemon is not running. Start Docker, or set TERMIRUST_MOBILE_TEST_SSH_* env vars for a reachable SSH host." >&2
  exit 1
fi

cleanup() {
  set +e
  if [[ -n "${CONTAINER_NAME:-}" ]]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

cd "$ROOT_DIR"
docker build -t termirust-e2e-sshd:local tests/fixtures/ssh-server >/dev/null

CONTAINER_NAME="termirust-mobile-android-ssh-$RANDOM-$RANDOM"
docker run --detach --rm --name "$CONTAINER_NAME" -p 127.0.0.1::22 termirust-e2e-sshd:local >/dev/null
PORT="$(docker inspect -f '{{(index (index .NetworkSettings.Ports "22/tcp") 0).HostPort}}' "$CONTAINER_NAME")"

KNOWN_HOST_KEY=""
for _ in {1..30}; do
  KNOWN_HOST_KEY="$(
    ssh-keyscan -p "$PORT" 127.0.0.1 2>/dev/null \
      | awk 'NF >= 3 && $2 ~ /^ssh-/ { print $2 " " $3; exit }'
  )"
  if [[ -n "$KNOWN_HOST_KEY" ]]; then
    break
  fi
  sleep 1
done

if [[ -z "$KNOWN_HOST_KEY" ]]; then
  echo "Unable to read Docker SSH host key." >&2
  docker logs "$CONTAINER_NAME" >&2 || true
  exit 1
fi

TERMIRUST_MOBILE_TEST_SSH_HOST="127.0.0.1" \
TERMIRUST_MOBILE_TEST_SSH_PORT="$PORT" \
TERMIRUST_MOBILE_TEST_SSH_USER="termirust" \
TERMIRUST_MOBILE_TEST_SSH_KEY="$(cat "$ROOT_DIR/tests/fixtures/ssh-server/id_ed25519")" \
TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY="$KNOWN_HOST_KEY" \
  run_android_smoke
