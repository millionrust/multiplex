#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

for gate in \
  verify-ssh-access-policy.sh \
  verify-openssh-certificate-auth.sh \
  verify-ssh-agent-access.sh \
  verify-ssh-key-lifecycle.sh \
  verify-mosh-decision.sh \
  verify-weak-local-transport-decision.sh \
  verify-proxy-routing.sh \
  verify-sftp-transfer-manager.sh \
  verify-connection-diagnostics.sh
do
  printf '\n==> %s\n' "$gate"
  "./scripts/$gate"
done

echo 'E07 infrastructure-access acceptance gates passed'
