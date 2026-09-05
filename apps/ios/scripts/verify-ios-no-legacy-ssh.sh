#!/usr/bin/env bash
set -euo pipefail

printf 'Direct SSH is now an intentional product route; verifying the unified app instead.\n'
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/verify-ios-unified-routes.sh" "$@"
