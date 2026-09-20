#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture="$root/tests/fixtures/controller-lan"

if [ "${1:-}" = "--fixture" ]; then
    [ "$#" -eq 2 ] || {
        echo "usage: $0 [--fixture PATH]" >&2
        exit 2
    }
    case "$2" in
        /*) fixture=$2 ;;
        *) fixture="$root/$2" ;;
    esac
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--fixture PATH]" >&2
    exit 2
fi

[ -f "$fixture/README.md" ] || {
    echo "controller LAN fixture is missing: $fixture" >&2
    exit 1
}

cd "$root"
cargo test -p multiplex-controller-listener --all-targets
cargo test -p multiplex ui::app::remote_devices::network_tests
cargo clippy -p multiplex-controller-listener --all-targets -- -D warnings
cargo run -q -p multiplex-ui-contract --bin generate-messages -- --check

echo "controller LAN verification passed"
