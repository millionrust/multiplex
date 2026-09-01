#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

bytes=""
profile="release"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bytes) bytes="${2:-}"; shift 2 ;;
    --profile) profile="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! "$bytes" =~ ^[1-9][0-9]*$ || "$profile" != "release" ]]; then
  echo "Usage: $0 --bytes 104857600 --profile release" >&2
  exit 2
fi

cargo build -q --release -p termirust-ui-contract --bin bench-terminal-accessibility
./target/release/bench-terminal-accessibility --bytes "$bytes"
