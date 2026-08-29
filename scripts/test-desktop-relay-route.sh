#!/bin/sh
set -eu

usage() {
  printf '%s\n' 'usage: test-desktop-relay-route.sh --local-only [--fault-matrix]' >&2
  exit 64
}

local_only=false
fault_matrix=false
for argument in "$@"; do
  case "$argument" in
    --local-only) local_only=true ;;
    --fault-matrix) fault_matrix=true ;;
    *) usage ;;
  esac
done

if [ "$local_only" != true ]; then
  printf '%s\n' 'Refusing non-local relay verification; this script never deploys a relay.' >&2
  exit 64
fi

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo test -p termirust-relay-client --all-targets --locked
cargo test -p termirust-relay-client --test host_controller_e2e --locked
cargo test -p termirust-relay-client --test reconnect_reconciliation --locked

if [ "$fault_matrix" = true ]; then
  cargo test -p termirust-relay-client --test hostile_relay_tls_limits --locked
  cargo test -p termirust-relay-server --test hostile_forwarding_limits --locked
  cargo test -p termirust-relay-server --test admission_revocation --locked
fi

printf '%s\n' 'desktop relay route verification passed'
