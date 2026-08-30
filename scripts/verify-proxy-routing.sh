#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

test -f docs/decisions/proxy-routing.md
test -f src/proxy.rs

rg -q 'enum OutboundProxy' src/models.rs
rg -q 'connect_first_hop' src/ssh.rs src/sftp.rs src/proxy.rs
rg -q 'MAX_HTTP_HEADER_BYTES' src/proxy.rs
rg -q 'PROXY_TIMEOUT' src/proxy.rs
rg -q 'ForwardTaskGuard' src/ssh.rs
rg -q 'editor-outbound-proxy' src/ui/app/mod.rs

if rg -n 'ProxyCommand|proxy_command|proxy_password|Proxy-Authorization' \
  src/proxy.rs src/ssh.rs src/sftp.rs src/models.rs src/ui/app/mod.rs; then
  echo 'unsupported executable or credential-bearing proxy behavior found' >&2
  exit 1
fi

echo 'proxy routing boundary verified'
