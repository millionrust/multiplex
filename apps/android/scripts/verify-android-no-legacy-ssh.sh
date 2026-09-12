#!/bin/sh
set -eu

while [ "$#" -gt 0 ]; do
  case "$1" in
    --variant) shift 2 ;;
    *) echo "usage: $0 [--variant ignored]" >&2; exit 2 ;;
  esac
done

echo "Direct SSH is now an intentional product route; verifying the unified app instead."
exec "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/verify-android-unified-routes.sh" --structural
