#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-/Users/jacob/Projects/terminal_app/terminal_swift}"
IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-}"
SSH_IMAGE="${TERMIRUST_MOBILE_TEST_SSH_IMAGE:-termirust-e2e-sshd:local}"
CONFIG_PATH="$IOS_DIR/TermiRustMobileTests/.termirust-mobile-live-ssh.properties"

if [[ ! -d "$IOS_DIR/TermiRustMobile.xcodeproj" ]]; then
  echo "iOS project not found at $IOS_DIR/TermiRustMobile.xcodeproj" >&2
  exit 1
fi

if [[ -z "$IOS_DESTINATION" ]]; then
  simulator_id="$(
    xcrun simctl list devices available 2>/dev/null \
      | awk -F '[()]' '/iPhone/ { print $2; exit }'
  )"
  if [[ -z "$simulator_id" ]]; then
    echo "No available iPhone simulator. Set TERMIRUST_IOS_DESTINATION for an eligible device." >&2
    exit 1
  fi
  IOS_DESTINATION="platform=iOS Simulator,id=$simulator_id"
fi

run_ios_smoke() {
  cd "$IOS_DIR"
  xcodebuild test -quiet \
    -project TermiRustMobile.xcodeproj \
    -scheme TermiRustMobile \
    -destination "$IOS_DESTINATION" \
    -only-testing:TermiRustMobileTests/DirectSSHIntegrationTests/testDirectSSHAttachesToPersistentTmuxSessionAndSurvivesReconnect
}

write_smoke_config() {
  mkdir -p "$(dirname "$CONFIG_PATH")"
  {
    printf 'TERMIRUST_MOBILE_TEST_SSH_HOST=%s\n' "$TERMIRUST_MOBILE_TEST_SSH_HOST"
    printf 'TERMIRUST_MOBILE_TEST_SSH_PORT=%s\n' "$TERMIRUST_MOBILE_TEST_SSH_PORT"
    printf 'TERMIRUST_MOBILE_TEST_SSH_USER=%s\n' "$TERMIRUST_MOBILE_TEST_SSH_USER"
    printf 'TERMIRUST_MOBILE_TEST_SSH_KEY_BASE64=%s\n' "$(printf '%s' "$TERMIRUST_MOBILE_TEST_SSH_KEY" | base64 | tr -d '\n')"
    printf 'TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY_BASE64=%s\n' "$(printf '%s' "$TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY" | base64 | tr -d '\n')"
  } > "$CONFIG_PATH"
}

cleanup_config() {
  rm -f "$CONFIG_PATH"
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
  trap cleanup_config EXIT
  write_smoke_config
  run_ios_smoke
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
  cleanup_config
  if [[ -n "${CONTAINER_NAME:-}" ]]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

cd "$ROOT_DIR"
if [[ "${TERMIRUST_MOBILE_REBUILD_SSH_IMAGE:-0}" == "1" ]] || ! docker image inspect "$SSH_IMAGE" >/dev/null 2>&1; then
  docker build -t "$SSH_IMAGE" tests/fixtures/ssh-server >/dev/null
fi

CONTAINER_NAME="termirust-mobile-ios-ssh-$RANDOM-$RANDOM"
docker run --detach --rm --name "$CONTAINER_NAME" -p 127.0.0.1::22 "$SSH_IMAGE" >/dev/null
PORT="$(docker inspect -f '{{(index (index .NetworkSettings.Ports "22/tcp") 0).HostPort}}' "$CONTAINER_NAME")"

KNOWN_HOST_KEY=""
for _ in {1..30}; do
  KNOWN_HOST_KEY="$(
    { ssh-keyscan -t ed25519 -p "$PORT" 127.0.0.1 2>/dev/null || true; } \
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

export TERMIRUST_MOBILE_TEST_SSH_HOST="127.0.0.1"
export TERMIRUST_MOBILE_TEST_SSH_PORT="$PORT"
export TERMIRUST_MOBILE_TEST_SSH_USER="termirust"
export TERMIRUST_MOBILE_TEST_SSH_KEY="$(cat "$ROOT_DIR/tests/fixtures/ssh-server/id_ed25519")"
export TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY="$KNOWN_HOST_KEY"
write_smoke_config
run_ios_smoke
