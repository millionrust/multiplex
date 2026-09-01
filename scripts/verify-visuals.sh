#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--inventory" || -z "${2:-}" || "${3:-}" != "--themes" || "${4:-}" != "all" || "${5:-}" != "--scales" || "${6:-}" != "100,200,400" || $# -ne 6 ]]; then
  echo "Usage: $0 --inventory tests/ui/audit-cases.toml --themes all --scales 100,200,400" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin ui-audit -- visuals \
  --inventory "$2" --themes "$4" --scales "$6"
