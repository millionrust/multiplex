#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--inventory" || -z "${2:-}" || "${3:-}" != "--platform" || "${4:-}" != "macos" || "${5:-}" != "--reader" || "${6:-}" != "voiceover" || $# -ne 6 ]]; then
  echo "Usage: $0 --inventory tests/ui/audit-cases.toml --platform macos --reader voiceover" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin ui-audit -- run \
  --inventory "$2" --platform "$4" --reader "$6"
