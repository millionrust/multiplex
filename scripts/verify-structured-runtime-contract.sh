#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust structured_runtime_contract --locked -- --test-threads=1
cargo test -p termirust cancellation_remains_cancelled_after_the_child_exits --locked -- --test-threads=1
cargo clippy -p termirust --bin termirust --tests --locked
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-G11.5-01 OK: Codex, Claude Code, Gemini CLI use one bounded synthetic process contract'
printf '%s\n' 'AT-G11.5-02 OK: shared success semantics and truthful capabilities match'
printf '%s\n' 'AT-G11.5-03 OK: failure and cancellation settle exactly once'
printf '%s\n' 'AT-G11.5-04 OK: malformed/oversize recovery, argv safety, cleanup, and redaction pass'
