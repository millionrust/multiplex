#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

printf '%s\n' "==> Browser capability unit and contract tests"
cargo fmt --all -- --check
cargo test -p termirust-browser -p termirust-mcp --locked

browser="${TERMIRUST_BROWSER_EXECUTABLE:-}"
if [[ -z "$browser" ]]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "/usr/bin/google-chrome" \
    "/usr/bin/chromium" \
    "/usr/bin/chromium-browser"; do
    if [[ -x "$candidate" ]]; then
      browser="$candidate"
      break
    fi
  done
fi

if [[ -n "$browser" && -x "$browser" ]]; then
  printf '%s\n' "==> Live isolated-browser containment and cancellation"
  TERMIRUST_BROWSER_EXECUTABLE="$browser" \
    cargo test -p termirust-browser --locked -- --ignored --nocapture
  printf '%s\n' "PASS: live isolated browser executed"
else
  printf '%s\n' "SKIPPED(browser): install Chrome/Chromium or set TERMIRUST_BROWSER_EXECUTABLE"
fi

printf '%s\n' "==> Browser strict Clippy and static security policy"
cargo clippy -p termirust-browser -p termirust-mcp --all-targets --locked -- -D warnings

if rg -n -- '--no-sandbox|--disable-web-security|--ignore-certificate-errors' \
  crates/termirust-browser crates/termirust-mcp; then
  printf '%s\n' "Browser sandbox or certificate verification must not be disabled." >&2
  exit 1
fi

for marker in \
  'env_clear()' \
  'SetDownloadBehaviorBehavior::Deny' \
  'MAX_DOWNLOAD_BYTES' \
  'browser_origins' \
  'process_group(0)'; do
  if ! rg -Fq "$marker" crates/termirust-browser crates/termirust-mcp; then
    printf 'Missing browser containment marker: %s\n' "$marker" >&2
    exit 1
  fi
done

git diff --check
printf '%s\n' "PASS: isolated browser capability"
