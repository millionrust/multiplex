#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
manifest="$root/tools/browser-engine-spike/Cargo.toml"
fixtures="$root/tests/fixtures/browser-hostile"
fixture_only=false
runs=""
output=""

usage() {
  cat <<'USAGE'
Usage: ./scripts/run-browser-spike.sh --fixture-only --runs <10-100> --output <report.json>

Runs only committed synthetic pages on a loopback listener. It never downloads,
launches, or connects to a browser and never visits a public URL.
USAGE
}

while (($#)); do
  case "$1" in
    --fixture-only)
      fixture_only=true
      shift
      ;;
    --runs)
      runs="${2:-}"
      shift 2
      ;;
    --output)
      output="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$fixture_only" != true || ! "$runs" =~ ^[0-9]+$ ]] || ((runs < 10 || runs > 100)) || [[ -z "$output" ]]; then
  usage >&2
  exit 2
fi

if [[ "$output" != /* ]]; then
  output="$root/$output"
fi
mkdir -p "$(dirname "$output")"

timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
if [[ -f "$output" ]]; then
  history="$(dirname "$output")/history"
  mkdir -p "$history"
  cp -p "$output" "$history/report-${timestamp//:/-}.json"
fi

TERMIRUST_SPIKE_TIMESTAMP="$timestamp" \
TERMIRUST_SPIKE_RUSTC="$(rustc --version)" \
CARGO_TARGET_DIR="$root/target/browser-spike/build" \
  cargo run \
    --quiet \
    --manifest-path "$manifest" \
    --locked \
    -- \
    --fixture-only \
    --fixtures "$fixtures" \
    --runs "$runs" \
    --output "$output"

rmdir "$(dirname "$output")/scratch" 2>/dev/null || true
