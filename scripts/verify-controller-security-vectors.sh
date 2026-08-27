#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ "${1:-}" != "--check" || "$#" -ne 1 ]]; then
  echo "usage: $0 --check" >&2
  exit 2
fi

fixture="crates/termirust-controller-security/tests/vectors/controller-v1.json"
adr="docs/decisions/controller-security-v1.md"

expected_adr=$(sed -n 's/.*"adr_sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$fixture")
expected_lock=$(sed -n 's/.*"cargo_lock_sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$fixture")
actual_adr=$(shasum -a 256 "$adr" | awk '{print $1}')
actual_lock=$(shasum -a 256 Cargo.lock | awk '{print $1}')

[[ "$expected_adr" == "$actual_adr" ]] || {
  echo "controller-security ADR checksum mismatch" >&2
  exit 1
}
[[ "$expected_lock" == "$actual_lock" ]] || {
  echo "controller-security lockfile checksum mismatch" >&2
  exit 1
}

if rg -n 'TcpListener|UdpSocket|std::net|tokio|uniffi' crates/termirust-controller-security/src; then
  echo "controller-security crate contains a forbidden transport or FFI surface" >&2
  exit 1
fi

cargo test -p termirust-controller-security --test golden_vectors --locked
echo "controller-security-v1 vectors and checksums verified"
