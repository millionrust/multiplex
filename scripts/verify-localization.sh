#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--locales" || "${2:-}" != "en-US,en-XA,ar-XB" || "${3:-}" != "--no-new-baseline" || $# -ne 3 ]]; then
  echo "Usage: $0 --locales en-US,en-XA,ar-XB --no-new-baseline" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
cargo run -q -p termirust-ui-contract --bin verify-localization -- "$@"
