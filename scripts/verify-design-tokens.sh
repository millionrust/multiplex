#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -eq 2 && "${1:-}" == "--all-ui" && "${2:-}" == "--no-new-baseline" ]]; then
  :
elif [[ $# -eq 3 && "${1:-}" == "--paths" && -n "${2:-}" && "${3:-}" == "--zero-legacy" ]]; then
  :
elif [[ $# -eq 3 && "${1:-}" == "--surface" && -n "${2:-}" && "${3:-}" == "--zero-legacy" ]]; then
  :
elif [[ $# -eq 4 && "${1:-}" == "--surface" && "${2:-}" == "terminal-chrome" && "${3:-}" == "--zero-legacy-except" && "${4:-}" == "terminal-grid-metrics" ]]; then
  set -- --surface terminal-chrome --zero-legacy-except terminal-grid-metrics
else
  echo "Usage: $0 --all-ui --no-new-baseline" >&2
  echo "   or: $0 --paths src/ui/a.rs,src/ui/b.rs --zero-legacy" >&2
  echo "   or: $0 --surface vault-keys-snippets|settings|agent-canvas --zero-legacy" >&2
  echo "   or: $0 --surface terminal-chrome --zero-legacy-except terminal-grid-metrics" >&2
  exit 2
fi

cargo run -q -p termirust-ui-contract --bin generate-tokens -- --check
cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- "$@"
