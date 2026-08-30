#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DECISION="docs/decisions/weak-local-transports.md"
test -f "$DECISION"
grep -Fq "Status: Telnet excluded; serial deferred; no product capability is exposed" "$DECISION"
grep -Fq "https://www.rfc-editor.org/rfc/rfc854" "$DECISION"
grep -Fq "https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/termios.h.html" "$DECISION"

if rg -n -i '\btelnet\b|ConnectProtocol::Serial|icon!\("serial"\)|assets/icons/serial' \
    src crates Cargo.toml Cargo.lock assets \
    >/tmp/termirust-weak-local-surface.txt 2>/dev/null; then
    cat /tmp/termirust-weak-local-surface.txt >&2
    rm -f /tmp/termirust-weak-local-surface.txt
    echo "Unreviewed Telnet or serial product surface detected" >&2
    exit 1
fi
rm -f /tmp/termirust-weak-local-surface.txt

cargo test -p termirust-domain ssh_access --locked -- --test-threads=1
cargo test -p termirust e2e_choose_protocol --locked -- --test-threads=1
cargo fmt --all -- --check
git diff --check

printf 'Weak/local transport decision verification passed\n'
