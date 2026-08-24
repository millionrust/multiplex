#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--all-ui" || "${2:-}" != "--no-new-baseline" || $# -ne 2 ]]; then
  echo "Usage: $0 --all-ui --no-new-baseline" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin generate-tokens -- --check
cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- "$@"
