#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="$(
  cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT_DIR/Cargo.toml" \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"
HOST_BINARY="$TARGET_DIR/debug/termirust-session-host"
APP_BUNDLE="$TARGET_DIR/release/bundle/osx/TermiRust.app"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/termirust"
FIXTURE_ROOT=""
CONTAINER_NAME=""
APP_PID=""
APP_STARTED=0

status_line() {
  printf '%s: %s\n' "$1" "$2"
}

processes_for_binary() {
  local binary="$1"
  ps -axo pid=,command= \
    | awk -v binary="$binary" '$2 == binary { print $1 }' \
    | sort -n
}

stop_app() {
  if [[ -z "$APP_PID" ]] || ! kill -0 "$APP_PID" 2>/dev/null; then
    APP_PID=""
    return
  fi
  kill -TERM -- "-$APP_PID" 2>/dev/null || kill -TERM "$APP_PID" 2>/dev/null || true
  for _ in {1..50}; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
      wait "$APP_PID" 2>/dev/null || true
      APP_PID=""
      return
    fi
    sleep 0.1
  done
  kill -KILL -- "-$APP_PID" 2>/dev/null || kill -KILL "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  APP_PID=""
}

cleanup() {
  set +e
  stop_app
  if [[ -n "$CONTAINER_NAME" ]]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    CONTAINER_NAME=""
  fi
  if [[ -n "$FIXTURE_ROOT" && "$FIXTURE_ROOT" == "${TMPDIR:-/tmp}"/termirust-n02.* ]]; then
    rm -rf "$FIXTURE_ROOT"
    FIXTURE_ROOT=""
  fi
}

interrupted() {
  status_line "FAIL" "N02 golden run interrupted; owned resources are being removed"
  cleanup
  exit 130
}

trap interrupted HUP INT TERM
trap cleanup EXIT

if [[ "$(uname -s)" != "Darwin" ]]; then
  status_line "FAIL" "N02 bundled desktop golden run currently requires macOS"
  exit 1
fi
for tool in cargo cargo-bundle docker python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    status_line "FAIL" "required tool is missing: $tool"
    exit 127
  fi
done
if ! docker info >/dev/null 2>&1; then
  status_line "FAIL" "Docker is unavailable; start Docker Desktop and wait for docker info"
  exit 1
fi

FIXTURE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termirust-n02.XXXXXX")"
CONFIG_DIR="$FIXTURE_ROOT/config"
SSH_KEY="$FIXTURE_ROOT/id_ed25519"
LOCAL_MARKER="$FIXTURE_ROOT/local-ready"
APP_STDOUT="$FIXTURE_ROOT/app.stdout"
APP_STDERR="$FIXTURE_ROOT/app.stderr"
mkdir -p "$CONFIG_DIR"
cp "$ROOT_DIR/tests/fixtures/ssh-server/id_ed25519" "$SSH_KEY"
chmod 600 "$SSH_KEY"
BASE_HOST_PROCESSES="$(processes_for_binary "$HOST_BINARY")"
BASE_APP_PROCESSES="$(processes_for_binary "$APP_BINARY")"

status_line "RUN" "disposable Docker SSH fixture"
docker build -t termirust-e2e-sshd:local "$ROOT_DIR/tests/fixtures/ssh-server" >/dev/null
CONTAINER_NAME="termirust-n02-$RANDOM-$RANDOM"
docker run --detach --rm --name "$CONTAINER_NAME" \
  -p 127.0.0.1::22 termirust-e2e-sshd:local >/dev/null
SSH_PORT="$(
  docker inspect -f '{{(index (index .NetworkSettings.Ports "22/tcp") 0).HostPort}}' \
    "$CONTAINER_NAME"
)"
status_line "PASS" "disposable Docker SSH fixture"

status_line "RUN" "separate Host and Controller replay/writer/revocation proof"
cd "$ROOT_DIR"
cargo build -p termirust-session-host >/dev/null
TERMIRUST_N02_HOST_BIN="$HOST_BINARY" \
TERMIRUST_N02_SSH_PORT="$SSH_PORT" \
TERMIRUST_N02_SSH_KEY="$SSH_KEY" \
  cargo test -p termirust-controller-listener \
    --test desktop_host_golden \
    bundled_desktop_host_controller_golden_run -- --exact --nocapture
status_line "PASS" "separate Host and Controller replay/writer/revocation proof"

status_line "RUN" "real unsigned TermiRust application bundle"
cargo bundle --release >/dev/null
if [[ ! -x "$APP_BINARY" ]]; then
  status_line "FAIL" "bundled TermiRust executable was not produced"
  exit 1
fi
status_line "PASS" "real unsigned TermiRust application bundle"

REMOTE_MARKER="/tmp/termirust-n02-app-ready"
python3 - \
  "$CONFIG_DIR/state.json" "$SSH_PORT" "$SSH_KEY" "$ROOT_DIR" \
  "$LOCAL_MARKER" "$REMOTE_MARKER" <<'PY'
import json
import shlex
import sys
from pathlib import Path

state_path, port, key, cwd, local_marker, remote_marker = sys.argv[1:]
local_command = (
    f"printf local-ready > {shlex.quote(local_marker)}; "
    "trap 'exit 0' INT TERM; while :; do sleep 1; done"
)
state = {
    "settings": {
        "restore_workspaces_on_launch": True,
        "onboarding_dismissed": True,
        "default_local_shell": {"program": "/bin/sh", "args": [], "cwd": cwd},
    },
    "restored_workspaces": [
        {
            "title": "N02 Local PTY",
            "active_pane_index": 0,
            "panes": [{
                "title": "N02 Local PTY",
                "kind": "local_shell",
                "local_shell": {
                    "program": "/bin/sh",
                    "args": ["-c", local_command],
                    "cwd": cwd,
                },
            }],
        },
        {
            "title": "N02 Docker SSH",
            "active_pane_index": 0,
            "panes": [{
                "title": "N02 Docker SSH",
                "kind": "ssh",
                "host": "127.0.0.1",
                "port": int(port),
                "username": "termirust",
                "auth": {"private_key": {"key_path": key}},
                "startup_directory": "/tmp",
                "startup_command": f"printf app-ready > {remote_marker}",
                "start_in_files": False,
                "persistent_session": False,
                "terminal_scrollback_rows": 10000,
            }],
        },
    ],
    "active_workspace_index": 1,
}
Path(state_path).write_text(json.dumps(state), encoding="utf-8")
PY

status_line "RUN" "bundled desktop local PTY and SSH restore"
TERMIRUST_CONFIG_DIR="$CONFIG_DIR" \
  /usr/bin/python3 -c \
    'import os, sys; os.setsid(); os.execv(sys.argv[1], sys.argv[1:])' \
    "$APP_BINARY" >"$APP_STDOUT" 2>"$APP_STDERR" &
APP_PID=$!
APP_STARTED=1
for _ in {1..100}; do
  if [[ -f "$LOCAL_MARKER" ]] \
    && docker exec "$CONTAINER_NAME" test -f "$REMOTE_MARKER" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    status_line "FAIL" "bundled desktop exited before restoring its sessions"
    exit 1
  fi
  sleep 0.2
done
if [[ "$(cat "$LOCAL_MARKER" 2>/dev/null || true)" != "local-ready" ]]; then
  status_line "FAIL" "bundled desktop did not open the restored local PTY"
  exit 1
fi
if ! docker exec "$CONTAINER_NAME" sh -lc \
  "test \"\$(cat '$REMOTE_MARKER')\" = app-ready" >/dev/null 2>&1; then
  status_line "FAIL" "bundled desktop did not complete the restored SSH startup"
  exit 1
fi
if ! docker logs "$CONTAINER_NAME" 2>&1 \
  | grep -q 'Accepted publickey for termirust'; then
  status_line "FAIL" "Docker fixture did not observe bundled-app SSH authentication"
  exit 1
fi
if [[ ! -f "$CONFIG_DIR/known_hosts.json" ]]; then
  status_line "FAIL" "bundled desktop did not persist isolated host trust"
  exit 1
fi
status_line "PASS" "bundled desktop local PTY and SSH restore"

stop_app
if [[ "$APP_STARTED" -ne 1 ]]; then
  status_line "FAIL" "bundled application launch was not accounted"
  exit 1
fi
if [[ "$(processes_for_binary "$HOST_BINARY")" != "$BASE_HOST_PROCESSES" ]]; then
  status_line "FAIL" "a golden-run Session Host process survived cleanup"
  exit 1
fi
if [[ "$(processes_for_binary "$APP_BINARY")" != "$BASE_APP_PROCESSES" ]]; then
  status_line "FAIL" "a golden-run bundled-app process survived cleanup"
  exit 1
fi

docker rm -f "$CONTAINER_NAME" >/dev/null
REMOVED_CONTAINER="$CONTAINER_NAME"
CONTAINER_NAME=""
if docker inspect "$REMOVED_CONTAINER" >/dev/null 2>&1; then
  status_line "FAIL" "golden-run Docker container survived cleanup"
  exit 1
fi
rm -rf "$FIXTURE_ROOT"
FIXTURE_ROOT=""
trap - EXIT
status_line "PASS" "owned process, container, credential, and state cleanup"
status_line "PASS" "N02 real bundled desktop and Host golden run"
