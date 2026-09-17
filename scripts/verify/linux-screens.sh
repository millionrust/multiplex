#!/usr/bin/env bash
# Builds and tests the Linux halves of Remote Screens, which cannot be built on macOS at all:
# the capture backend needs libpipewire, and the input backend needs Linux headers to link.
#
#   scripts/verify/linux-screens.sh [cargo subcommand] [args...]
#
# Defaults to `test --all-targets`; `clippy --all-targets` is the other one worth running.
#
# Three things about how this reaches Linux are worth knowing before changing it.
#
# The sources are streamed in as a tar rather than bind-mounted. DOCKER_HOST is often a daemon on
# another machine, and a bind mount would name paths on *that* machine, so a mount silently
# resolves to nothing rather than failing. The tar always arrives.
#
# Only the screen crates are sent, with a workspace manifest written inside the container. Sending
# the whole repository would drag in every other crate's dependencies for no benefit; this only
# has to prove these crates compile and their tests pass on Linux.
#
# What this can and cannot establish is worth being exact about. It runs the parts that are pure
# logic -- keymaps, stride unpacking, the format offered to PipeWire -- which is real coverage
# that a cross-compile check alone does not give. It cannot establish anything that needs a
# desktop: no compositor answers the portal, and /dev/uinput is not there.
#
# Skips itself when no daemon answers, the way the other Docker-backed checks here do.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE=termirust-linux-check
SUBCOMMAND=${1:-test}
shift || true
ARGS=${*:---all-targets}

CRATES=(
    termirust-screen-capture
    termirust-screen-codec
    termirust-screen-input
    termirust-screen-protocol
    termirust-screen-session
    termirust-screen-video
)

if ! docker info >/dev/null 2>&1; then
    echo "no Docker daemon answered; skipping the Linux screens check"
    exit 0
fi

docker build -q -t "$IMAGE" - >/dev/null <<'DOCKERFILE'
FROM rust:1.97-trixie
RUN apt-get update -qq \
 && apt-get install -y -qq libpipewire-0.3-dev clang pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy
DOCKERFILE

members=$(printf '"crates/%s",' "${CRATES[@]}")
selected=$(printf -- '-p %s ' "${CRATES[@]}")
paths=$(printf 'crates/%s ' "${CRATES[@]}")

# shellcheck disable=SC2086
tar -C "$ROOT" -cf - $paths \
  | docker run --rm -i -w /w \
      -v termirust-linux-registry:/usr/local/cargo/registry \
      -v termirust-linux-target:/target \
      -e CARGO_TARGET_DIR=/target \
      "$IMAGE" bash -c "
        set -e
        mkdir -p /w && cd /w && tar -xf -
        cat > Cargo.toml <<EOF
[workspace]
members = [${members%,}]
resolver = \"3\"

[workspace.package]
rust-version = \"1.88\"
EOF
        cargo $SUBCOMMAND $selected $ARGS
      "
