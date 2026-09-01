#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -ne 6 || "${1:-}" != "--surface" || "${2:-}" != "shell-overlays-palette" || "${3:-}" != "--locales" || "${4:-}" != "en-US,en-XA,ar-XB" || "${5:-}" != "--themes" || "${6:-}" != "all" ]]; then
  echo "Usage: $0 --surface shell-overlays-palette --locales en-US,en-XA,ar-XB --themes all" >&2
  exit 2
fi

paths="src/ui/shell.rs,src/ui/app/chrome.rs,src/ui/app/overlay.rs,src/ui/app/palette.rs"

cargo run -q -p termirust-ui-contract --bin generate-tokens -- --check
cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- --paths "$paths" --zero-legacy
cargo run -q -p termirust-ui-contract --bin verify-localization -- --locales en-US,en-XA,ar-XB --paths "$paths" --zero-legacy
cargo test -q -p termirust-ui-contract shell_surface

echo "verified shell, overlay, and palette tokens, copy, semantics, scale, locale, and theme contracts"
