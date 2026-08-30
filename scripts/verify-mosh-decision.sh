#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DECISION="docs/decisions/mosh-lifecycle.md"
test -f "$DECISION"

grep -Fq "Status: deferred; no product capability is exposed" "$DECISION"
grep -Fq "https://github.com/mobile-shell/mosh" "$DECISION"
grep -Fq "https://mosh.org/mosh-paper.pdf" "$DECISION"
grep -Fq "no silent SSH fallback" "$DECISION"

# A future implementation must replace this decision and gate deliberately. Until then,
# source and manifests cannot accidentally expose an unverified Mosh product surface.
if rg -n -i '\bmosh([-_ ]?(client|server))?\b' \
    src crates Cargo.toml Cargo.lock assets >/tmp/termirust-mosh-surface.txt 2>/dev/null; then
    cat /tmp/termirust-mosh-surface.txt >&2
    rm -f /tmp/termirust-mosh-surface.txt
    echo "Unreviewed Mosh product surface detected" >&2
    exit 1
fi
rm -f /tmp/termirust-mosh-surface.txt

cargo test -p termirust-domain ssh_access --locked -- --test-threads=1
cargo fmt --all -- --check
git diff --check

printf 'Mosh lifecycle defer decision verification passed\n'
