#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

hours=""
output="target/qualification/session-relay-soak.jsonl"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --hours) hours="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! "$hours" =~ ^[0-9]+$ || "$hours" -lt 48 || "$hours" -gt 168 ]]; then
  echo "Usage: $0 --hours 48..168 [--output PATH]" >&2
  exit 2
fi

mkdir -p "$(dirname "$output")"
: > "$output"
deadline=$(( $(date +%s) + hours * 3600 ))
iteration=0

while (( $(date +%s) < deadline )); do
  iteration=$((iteration + 1))
  started=$(date +%s)
  log=$(mktemp "${TMPDIR:-/tmp}/termirust-soak.XXXXXX")
  status=pass
  if ! cargo test -q -p termirust-session-host --test lifecycle --locked >"$log" 2>&1 \
    || ! cargo test -q -p termirust-relay-server --test admission_revocation --locked >>"$log" 2>&1 \
    || ! cargo test -q -p termirust-relay-server --test hostile_forwarding_limits --locked >>"$log" 2>&1 \
    || ! cargo test -q -p termirust-controller-listener --test authenticated_bridge --locked >>"$log" 2>&1; then
    status=fail
  fi
  finished=$(date +%s)
  printf '{"iteration":%d,"started_unix":%d,"finished_unix":%d,"status":"%s"}\n' \
    "$iteration" "$started" "$finished" "$status" >> "$output"
  if [[ "$status" != pass ]]; then
    tail -n 160 "$log" >&2
    rm -f "$log"
    exit 1
  fi
  rm -f "$log"
  sleep 5
done

printf 'PASS: %s-hour Session/relay endurance cycle (%d iterations)\n' "$hours" "$iteration"
