#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

test -f docs/decisions/proxy-routing.md
test -f crates/multiplex-desktop/src/proxy.rs

rg -q 'enum OutboundProxy' crates/multiplex-desktop/src/models.rs
rg -q 'connect_first_hop' crates/multiplex-desktop/src/ssh.rs crates/multiplex-desktop/src/sftp.rs crates/multiplex-desktop/src/proxy.rs
rg -q 'MAX_HTTP_HEADER_BYTES' crates/multiplex-desktop/src/proxy.rs
rg -q 'PROXY_TIMEOUT' crates/multiplex-desktop/src/proxy.rs
rg -q 'ForwardTaskGuard' crates/multiplex-desktop/src/ssh.rs
rg -q 'editor-outbound-proxy' crates/multiplex-desktop/src/ui/app/mod.rs

if rg -n 'ProxyCommand|proxy_command|proxy_password|Proxy-Authorization' \
  crates/multiplex-desktop/src/proxy.rs crates/multiplex-desktop/src/ssh.rs crates/multiplex-desktop/src/sftp.rs crates/multiplex-desktop/src/models.rs crates/multiplex-desktop/src/ui/app/mod.rs; then
  echo 'unsupported executable or credential-bearing proxy behavior found' >&2
  exit 1
fi

echo 'proxy routing boundary verified'
