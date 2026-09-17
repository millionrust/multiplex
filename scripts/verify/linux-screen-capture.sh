#!/usr/bin/env bash
# Builds and tests the Linux screen-capture backend, which needs libpipewire and so cannot be
# built on macOS at all.
#
#   scripts/verify/linux-screen-capture.sh [cargo subcommand] [args...]
#
# Defaults to `test --all-targets`; `clippy --all-targets` is the other one worth running.
#
# Two things about how this reaches Linux are worth knowing before changing it.
#
# The sources are streamed in as a tar rather than bind-mounted. DOCKER_HOST is often a daemon on
# another machine, and a bind mount would name paths on *that* machine, so a mount silently
# resolves to nothing. The tar always arrives.
#
# Only the two crates that matter are sent, with a workspace manifest written inside the
# container. Sending the whole repository would drag in every other crate's dependencies for no
# benefit, and this only has to prove that the capture backend compiles and its tests pass.
#
# Skips itself when no daemon answers, the way the other Docker-backed checks here do.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE=termirust-linux-check
SUBCOMMAND=${1:-test}
shift || true
ARGS=${*:---all-targets}

if ! docker info >/dev/null 2>&1; then
    echo "no Docker daemon answered; skipping the Linux capture check"
    exit 0
fi

docker build -q -t "$IMAGE" - >/dev/null <<'DOCKERFILE'
FROM rust:1.97-trixie
RUN apt-get update -qq \
 && apt-get install -y -qq libpipewire-0.3-dev clang pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy
DOCKERFILE

tar -C "$ROOT" -cf - \
    crates/termirust-screen-capture \
    crates/termirust-screen-codec \
  | docker run --rm -i -w /w \
      -v termirust-linux-registry:/usr/local/cargo/registry \
      -v termirust-linux-target:/target \
      -e CARGO_TARGET_DIR=/target \
      "$IMAGE" bash -c "
        set -e
        mkdir -p /w && cd /w && tar -xf -
        cat > Cargo.toml <<'EOF'
[workspace]
members = [\"crates/termirust-screen-capture\", \"crates/termirust-screen-codec\"]
resolver = \"3\"

[workspace.package]
rust-version = \"1.88\"
EOF
        cargo $SUBCOMMAND -p termirust-screen-capture $ARGS
      "
