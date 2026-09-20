#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

cargo test -p multiplex-host-protocol codec::tests::codec_fuzz_smoke -- --exact
