#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture="$root/tests/fixtures/ssh-controller"

if [ "${1:-}" = "--fixture" ]; then
    [ "$#" -eq 2 ] || {
        printf '%s\n' 'usage: test-controller-ssh.sh [--fixture PATH]' >&2
        exit 2
    }
    fixture=$2
elif [ "$#" -ne 0 ]; then
    printf '%s\n' 'usage: test-controller-ssh.sh [--fixture PATH]' >&2
    exit 2
fi

[ -d "$fixture" ] || {
    printf '%s\n' 'SSH Controller fixture directory is missing' >&2
    exit 1
}
[ -f "$fixture/hostile_ssh_config" ] || {
    printf '%s\n' 'hostile SSH configuration fixture is missing' >&2
    exit 1
}

cd "$root"
cargo test -p termirust-cli --test controller_ssh_json
cargo test -p termirust-cli --test ssh_argv_security
cargo test -p termirust-client --test ssh_controller_reconnect
cargo test -p termirust-controller-listener --test pairing_route
cargo test -p termirust-session-host --test remote_controller_bridge

if rg -n --hidden --glob '*.json' --glob '*.log' \
    'BEGIN (OPENSSH|RSA|EC|DSA) PRIVATE KEY|ABCD-1234|private\.example|operator@' \
    "$fixture" >/dev/null; then
    printf '%s\n' 'SSH Controller fixture contains a secret or private route canary' >&2
    exit 1
fi

printf '%s\n' 'SSH Controller fixture verification passed'
