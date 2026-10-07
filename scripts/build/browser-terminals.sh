#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
python3 "$root/scripts/build/browser-terminal-tokens.py"
cd "$root/apps/web-terminal"
if [[ ! -d node_modules ]]; then bun install --frozen-lockfile; fi
bun run typecheck
bun test
bun run build
mkdir -p "$root/crates/multiplex-cli/assets/web-terminal"
rsync -a --delete .output/public/ "$root/crates/multiplex-cli/assets/web-terminal/"
echo "Browser assets embedded. Rebuild Multiplex with Cargo; no JavaScript server is required."
