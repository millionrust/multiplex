#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 || ! $1 =~ ^[1-9][0-9]*$ ]]; then
  echo "Usage: $0 <positive-count> <exact-test-name>" >&2
  exit 2
fi

count=$1
test_name=$2
if (( count > 100 )); then
  echo "Repeat count must be 100 or less." >&2
  exit 2
fi

for ((run = 1; run <= count; run++)); do
  printf '[repeat-test] %d/%d %s\n' "$run" "$count" "$test_name"
  cargo test -p termirust "$test_name" -- --exact --nocapture
done
