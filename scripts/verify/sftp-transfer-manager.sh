#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

command -v docker >/dev/null 2>&1 || {
  echo 'Docker is required for the SFTP transfer-manager acceptance gate.' >&2
  exit 1
}
docker info >/dev/null 2>&1 || {
  echo 'Docker is installed but its engine is unavailable.' >&2
  exit 1
}

test -f docs/decisions/sftp-transfer-manager.md

rg -q 'pub struct SftpTransferManager' crates/termirust-desktop/src/sftp.rs
rg -q 'SFTP_TRANSFER_CHUNK_BYTES: usize = 256 \* 1024' crates/termirust-desktop/src/sftp.rs
rg -q 'SFTP_TRANSFER_MAX_ACTIVE: usize = 3' crates/termirust-desktop/src/sftp.rs
rg -q 'SFTP_TRANSFER_MAX_QUEUED: usize = 32' crates/termirust-desktop/src/sftp.rs
rg -q 'SFTP_TRANSFER_MAX_BYTES: u64 = 8 \* 1024 \* 1024 \* 1024' crates/termirust-desktop/src/sftp.rs
rg -q 'workspace-transfer-cancel' crates/termirust-desktop/src/ui/app/workspace.rs
rg -q 'workspace-transfer-replace' crates/termirust-desktop/src/ui/app/workspace.rs
rg -q 'workspace-transfer-resume' crates/termirust-desktop/src/ui/app/workspace.rs

if rg -n 'read_to_end|fs::read\(' crates/termirust-desktop/src/sftp.rs | awk '$1 + 0 >= 500 && $1 + 0 < 1100 { found = 1 } END { exit !found }'; then
  echo 'full-file buffering found in the SFTP transfer implementation' >&2
  exit 1
fi

cargo fmt --all -- --check
cargo test -p termirust 'sftp::tests::' --locked -- --test-threads=1
cargo test -p termirust \
  ui::app::tests::e2e_sftp_upload_and_download_via_dialog_actions \
  --locked -- --exact --test-threads=1

echo 'bounded SFTP transfer manager verified'
