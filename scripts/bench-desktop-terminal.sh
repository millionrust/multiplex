#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export CARGO_INCREMENTAL=0
cargo test --bin termirust \
  desktop_terminal_ \
  -- --include-ignored --nocapture --test-threads=1
