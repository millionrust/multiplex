#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--platform" || "${2:-}" != "macos" || "${3:-}" != "--locale" || "${4:-}" != "en-US,en-XA,ar-XB" || $# -ne 4 ]]; then
  echo "Usage: $0 --platform macos --locale en-US,en-XA,ar-XB" >&2
  exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "verify-accessibility-harness: macOS is required for the AppKit adapter" >&2
  exit 1
fi

cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
cargo test -q -p termirust-ui-contract accessibility_lab --locked
cargo test -q -p termirust-ui-contract semantics --locked
cargo test -q -p termirust-accessibility-macos --locked
cargo run -q -p termirust-ui-contract --bin accessibility_snapshot --locked \
  | diff -u tests/fixtures/accessibility/semantic-tree.snapshot -
cargo check -q -p termirust --locked

if rg -n "TERMIRUST_AX_SECRET_CANARY_7bd50a" tests/fixtures/accessibility/semantic-tree.snapshot >/dev/null; then
  echo "verify-accessibility-harness: secret canary leaked into semantic snapshot" >&2
  exit 1
fi

echo "verify-accessibility-harness: automated semantic, focus, localization, AppKit adapter, and GPUI compile checks passed"
echo "Manual VoiceOver evidence remains required; see docs/accessibility-harness.md"
