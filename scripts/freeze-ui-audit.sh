#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -eq 3 && "$2" == "--check-hash" ]]; then
  cargo run -q -p termirust-ui-contract --bin ui-audit -- freeze \
    --inventory "$1" --hash-file "$3"
elif [[ $# -eq 3 && "$2" == "--write-hash" ]]; then
  cargo run -q -p termirust-ui-contract --bin ui-audit -- freeze \
    --inventory "$1" --hash-file "$3" --write
else
  echo "Usage: $0 tests/ui/audit-cases.toml --check-hash|--write-hash HASH_FILE" >&2
  exit 2
fi
