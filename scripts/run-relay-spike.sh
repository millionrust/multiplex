#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
manifest="$root/tools/relay-spike/Cargo.toml"
local_only=false
pairs=""
runs=""
output=""

while (($#)); do
  case "$1" in
    --local-only) local_only=true; shift ;;
    --pairs) pairs=${2:?missing pair list}; shift 2 ;;
    --runs) runs=${2:?missing run count}; shift 2 ;;
    --output) output=${2:?missing output directory}; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ $local_only != true ]]; then
  echo "--local-only is required; this spike must not open or deploy a public route." >&2
  exit 2
fi
if [[ $pairs != "1,10,100,1000" ]]; then
  echo "--pairs must be exactly 1,10,100,1000 for comparable evidence." >&2
  exit 2
fi
if [[ ! $runs =~ ^[0-9]+$ ]] || ((runs < 10 || runs > 100)); then
  echo "--runs must be between 10 and 100." >&2
  exit 2
fi
if [[ -z $output ]]; then
  echo "--output is required." >&2
  exit 2
fi

mkdir -p "$output"
CARGO_TARGET_DIR="$root/target/relay-spike-build" \
  cargo run --release --manifest-path "$manifest" --locked -- \
  --local-only --pairs "$pairs" --runs "$runs" --output "$output"
cp "$root/tests/fixtures/relay/cost-model.json" "$output/cost-model.json"
cp "$root/tests/fixtures/relay/threat-model.json" "$output/threat-model.json"
