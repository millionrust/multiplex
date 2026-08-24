#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo test -p termirust-client --test real_host one_thousand_detach_reattach_cycles_keep_one_host -- --exact --ignored --nocapture
cargo test -p termirust-session-host --test faults thirty_two_host_limit_is_exact_and_the_next_start_fails_closed -- --exact --ignored --nocapture
cargo test -p termirust-store journal::tests::journal_encode_scan_throughput_meets_reference_target -- --exact --ignored --nocapture
