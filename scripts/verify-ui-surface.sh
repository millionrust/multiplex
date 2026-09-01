#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

surface=""
states="all"
locales=""
themes=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --surface) surface="${2:-}"; shift 2 ;;
    --states) states="${2:-}"; shift 2 ;;
    --locales) locales="${2:-}"; shift 2 ;;
    --themes) themes="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ "$states" != "all" || "$locales" != "en-US,en-XA,ar-XB" || "$themes" != "all" ]]; then
  echo "Usage: $0 --surface shell-overlays-palette|projects-groups-sessions [--states all] --locales en-US,en-XA,ar-XB --themes all" >&2
  exit 2
fi

case "$surface" in
  shell-overlays-palette)
    paths="src/ui/shell.rs,src/ui/app/chrome.rs,src/ui/app/overlay.rs,src/ui/app/palette.rs"
    test_filter="shell_surface"
    description="shell, overlay, and palette"
    ;;
  projects-groups-sessions)
    paths="src/ui/app/projects.rs,src/ui/app/session_sidebar.rs,src/ui/app/session_library.rs"
    test_filter="product_session_surface"
    description="Projects, groups, and Sessions"
    ;;
  *)
    echo "unknown UI surface: $surface" >&2
    exit 2
    ;;
esac

cargo run -q -p termirust-ui-contract --bin generate-tokens -- --check
cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- --paths "$paths" --zero-legacy
cargo run -q -p termirust-ui-contract --bin verify-localization -- --locales en-US,en-XA,ar-XB --paths "$paths" --zero-legacy
cargo test -q -p termirust-ui-contract "$test_filter"

echo "verified $description tokens, copy, semantics, states, scale, locale, and theme contracts"
