#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

test -f docs/decisions/proxy-routing.md
test -f crates/termirust-desktop/src/proxy.rs

rg -q 'enum OutboundProxy' crates/termirust-desktop/src/models.rs
rg -q 'connect_first_hop' crates/termirust-desktop/src/ssh.rs crates/termirust-desktop/src/sftp.rs crates/termirust-desktop/src/proxy.rs
rg -q 'MAX_HTTP_HEADER_BYTES' crates/termirust-desktop/src/proxy.rs
rg -q 'PROXY_TIMEOUT' crates/termirust-desktop/src/proxy.rs
rg -q 'ForwardTaskGuard' crates/termirust-desktop/src/ssh.rs
rg -q 'editor-outbound-proxy' crates/termirust-desktop/src/ui/app/mod.rs

if rg -n 'ProxyCommand|proxy_command|proxy_password|Proxy-Authorization' \
  crates/termirust-desktop/src/proxy.rs crates/termirust-desktop/src/ssh.rs crates/termirust-desktop/src/sftp.rs crates/termirust-desktop/src/models.rs crates/termirust-desktop/src/ui/app/mod.rs; then
  echo 'unsupported executable or credential-bearing proxy behavior found' >&2
  exit 1
fi

echo 'proxy routing boundary verified'
