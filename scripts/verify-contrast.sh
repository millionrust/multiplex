#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -ne 2 || "$2" != "--all-states" ]]; then
  echo "Usage: $0 design/tokens.toml --all-states" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin ui-audit -- contrast --tokens "$1"
