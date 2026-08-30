#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

python3 -m json.tool tests/fixtures/ssh-access/access-policy-v1.json >/dev/null
cargo test -p termirust-domain ssh_access --locked
cargo test -p termirust legacy_host_profiles_project_to_safe_ssh_access_policies --locked
cargo fmt --all -- --check
git diff --check

printf 'SSH access policy verification passed\n'
