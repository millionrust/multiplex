#!/bin/sh
set -eu

if [ "$#" -eq 2 ] && [ "$1" = "controller-bridge" ] && [ "$2" = "--stdio" ]; then
    exec cat
fi

printf '%s\n' 'This fixture only supports: termirust controller-bridge --stdio' >&2
exit 64
