#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "real app AX smoke only runs on macOS" >&2
  exit 1
fi

STATE_DIR="$HOME/Library/Application Support/tshell"
STATE_FILE="$STATE_DIR/state.json"
KNOWN_HOSTS_FILE="$STATE_DIR/known_hosts.json"
STATE_BACKUP=""
KNOWN_HOSTS_BACKUP=""
TMP_KEY=""

cleanup() {
  set +e
  pkill -x tshell >/dev/null 2>&1 || true
  if [[ -n "${CONTAINER_NAME:-}" ]]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TMP_KEY" && -f "$TMP_KEY" ]]; then
    rm -f "$TMP_KEY"
  fi
  if [[ -n "$STATE_BACKUP" && -f "$STATE_BACKUP" ]]; then
    mv "$STATE_BACKUP" "$STATE_FILE"
  else
    rm -f "$STATE_FILE"
  fi
  if [[ -n "$KNOWN_HOSTS_BACKUP" && -f "$KNOWN_HOSTS_BACKUP" ]]; then
    mv "$KNOWN_HOSTS_BACKUP" "$KNOWN_HOSTS_FILE"
  else
    rm -f "$KNOWN_HOSTS_FILE"
  fi
}
trap cleanup EXIT

mkdir -p "$STATE_DIR"
if [[ -f "$STATE_FILE" ]]; then
  STATE_BACKUP="$(mktemp)"
  cp "$STATE_FILE" "$STATE_BACKUP"
fi
if [[ -f "$KNOWN_HOSTS_FILE" ]]; then
  KNOWN_HOSTS_BACKUP="$(mktemp)"
  cp "$KNOWN_HOSTS_FILE" "$KNOWN_HOSTS_BACKUP"
fi

docker build -t tshell-e2e-sshd:local tests/fixtures/ssh-server >/dev/null
CONTAINER_NAME="tshell-real-ax-$RANDOM-$RANDOM"
docker run --detach --rm --name "$CONTAINER_NAME" -p 127.0.0.1::22 tshell-e2e-sshd:local >/dev/null
PORT="$(docker inspect -f '{{(index (index .NetworkSettings.Ports "22/tcp") 0).HostPort}}' "$CONTAINER_NAME")"
STARTUP_MARKER="/tmp/tshell-real-app-startup"

PROFILE_ID="real-app-docker-e2e"
TMP_KEY="$(mktemp)"
cp tests/fixtures/ssh-server/id_ed25519 "$TMP_KEY"
chmod 600 "$TMP_KEY"
python3 - <<PY
import json
from pathlib import Path

state = {
    "settings": {
        "theme_preset": "ocean",
        "terminal_font_size": 14,
        "onboarding_dismissed": True,
        "restore_workspaces_on_launch": True,
        "session_log_limit": 200,
        "default_local_shell": {
            "program": "/bin/zsh",
            "args": [],
            "cwd": "${ROOT_DIR}",
        },
        "default_ssh_startup_directory": None,
        "copy_on_select": False,
        "terminal_font_family": None,
        "confirm_multiline_paste": True,
        "auto_reconnect_attempts": 3,
        "auto_reconnect_delay_secs": 5,
        "ssh_keepalive_secs": 30,
        "sync_folder_path": None,
        "sync_last_pushed_at": None,
        "sync_last_pulled_at": None,
    },
    "vaults": [{
        "id": "vault-personal",
        "label": "Personal",
        "description": "Local vault for private hosts, snippets, and identities.",
        "kind": "personal",
        "members": [{
            "id": "member-you",
            "name": "${USER}",
            "email": "local@device",
            "role": "owner",
        }],
    }],
    "host_groups": [],
    "profiles": [{
        "id": "${PROFILE_ID}",
        "label": "docker-e2e",
        "vault_id": "vault-personal",
        "favorite": False,
        "group": "",
        "tags": [],
        "host": "127.0.0.1",
        "port": int("${PORT}"),
        "username": "tshell",
        "auth_mode": "private_key",
        "key_path": "${TMP_KEY}",
        "identity_id": "identity-real-app-docker-e2e",
        "jump_host_id": None,
        "startup_directory": "/tmp",
        "startup_command": "pwd > ${STARTUP_MARKER}",
        "start_in_files": False,
        "terminal_scrollback_rows": 10000,
        "port_forward_rules": [],
        "local_forwards": [],
        "local_forward": None,
        "password_credential_id": None,
        "source": "user",
        "description": "",
        "color_tag": None,
        "environment": [],
    }],
    "identities": [{
        "id": "identity-real-app-docker-e2e",
        "label": "docker-e2e-key",
        "vault_id": "vault-personal",
        "key_path": "${TMP_KEY}",
        "kind": "OpenSSH",
        "source": "user",
    }],
    "snippets": [],
    "command_history": [],
    "scoped_command_history": [],
    "selected_profile_id": None,
    "session_logs": [],
    "restored_workspaces": [{
        "title": "docker-e2e",
        "layout": {"Leaf": 0},
        "active_pane_index": 0,
        "panes": [{
            "title": "docker-e2e",
            "kind": "ssh",
            "host": "127.0.0.1",
            "port": int("${PORT}"),
            "username": "tshell",
            "auth": {"private_key": {"key_path": "${TMP_KEY}"}},
            "jump_host": None,
            "startup_directory": "/tmp",
            "startup_command": "pwd > ${STARTUP_MARKER}",
            "start_in_files": False,
            "terminal_scrollback_rows": 10000,
            "port_forward_rules": [],
            "local_forwards": [],
            "local_forward": None,
            "local_shell": None
        }]
    }],
    "active_workspace_index": 0,
    "window_bounds": {
        "x": 16.0,
        "y": 33.0,
        "width": 1480.0,
        "height": 949.0,
        "display_id": 1,
    },
}

Path("${STATE_FILE}").write_text(json.dumps(state, indent=2))
PY

rm -f "$KNOWN_HOSTS_FILE"

if [[ "${TSHELL_SKIP_RELEASE_BUILD:-0}" != "1" ]]; then
  cargo build --release >/dev/null
fi
APP_BUNDLE="/Volumes/Footages/cargo-target/terminal-c59500f320d9bfcc/release/bundle/osx/TShell.app"
APP_BINARY="/Volumes/Footages/cargo-target/terminal-c59500f320d9bfcc/release/tshell"
if [[ ! -x "$APP_BINARY" ]]; then
  echo "release app binary not found at $APP_BINARY" >&2
  exit 1
fi
if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "bundled app not found at $APP_BUNDLE" >&2
  exit 1
fi
cp "$APP_BINARY" "$APP_BUNDLE/Contents/MacOS/tshell"

pkill -x tshell >/dev/null 2>&1 || true
sleep 1
"$APP_BUNDLE/Contents/MacOS/tshell" >/tmp/tshell-real-app.out 2>/tmp/tshell-real-app.err &
APP_PID=""
for _ in {1..20}; do
  APP_PID="$(pgrep -x tshell | tail -n 1 || true)"
  if [[ -n "$APP_PID" ]]; then
    break
  fi
  sleep 1
done
if [[ -z "$APP_PID" ]]; then
  echo "unable to find launched TShell pid" >&2
  cat /tmp/tshell-real-app.err >&2 || true
  exit 1
fi

for _ in {1..20}; do
  if docker logs "$CONTAINER_NAME" 2>&1 | grep -q "Accepted publickey for tshell"; then
    break
  fi
  sleep 1
done

if ! docker logs "$CONTAINER_NAME" 2>&1 | grep -q "Accepted publickey for tshell"; then
  echo "real app never completed public-key auth against docker ssh server" >&2
  docker logs "$CONTAINER_NAME" >&2
  exit 1
fi

for _ in {1..20}; do
  if docker exec "$CONTAINER_NAME" sh -lc "test -f '$STARTUP_MARKER'"; then
    break
  fi
  sleep 1
done

if ! docker exec "$CONTAINER_NAME" sh -lc "test \"\$(cat '$STARTUP_MARKER')\" = '/tmp'"; then
  echo "bundled app never completed startup directory/command flow" >&2
  docker exec "$CONTAINER_NAME" sh -lc "ls -l '$STARTUP_MARKER' 2>/dev/null || true; cat '$STARTUP_MARKER' 2>/dev/null || true" >&2
  exit 1
fi

for _ in {1..10}; do
  if [[ -f "$KNOWN_HOSTS_FILE" ]]; then
    break
  fi
  sleep 1
done

if [[ ! -f "$KNOWN_HOSTS_FILE" ]]; then
  echo "known_hosts.json was not written" >&2
  exit 1
fi

if ! grep -q "\"127.0.0.1:${PORT}\"" "$KNOWN_HOSTS_FILE"; then
  echo "known_hosts.json does not contain the launched-app SSH endpoint" >&2
  cat "$KNOWN_HOSTS_FILE" >&2
  exit 1
fi

echo "real app AX SSH smoke passed against $APP_BUNDLE on port $PORT"
