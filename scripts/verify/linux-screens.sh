#!/usr/bin/env bash
# Builds and tests the Linux halves of Remote Screens, which cannot be built on macOS at all:
# the capture backend needs libpipewire, the input backend needs Linux headers to link, and the
# desktop crate cannot be cross-compiled from a Mac because ring's build script wants a C
# toolchain for the target.
#
#   scripts/verify/linux-screens.sh [cargo subcommand] [args...]
#   scripts/verify/linux-screens.sh --desktop [cargo subcommand] [args...]
#
# Defaults to `test --all-targets`; `clippy --all-targets` is the other one worth running.
#
# `--desktop` adds the desktop crate, which is where the Linux screen-sharing flow lives -- the
# portal grant and the fan-out that feeds every watcher from one stream. It costs a much larger
# image and a five-minute build, so it is not the default, but it is the only way to type-check
# that code anywhere.
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
DESKTOP=0
if [[ ${1:-} == --desktop ]]; then
    DESKTOP=1
    IMAGE=termirust-linux-desktop
    shift
fi
SUBCOMMAND=${1:-test}
shift || true
ARGS=${*:---all-targets}

CRATES=(
    multiplex-screen-capture
    multiplex-screen-codec
    multiplex-screen-input
    multiplex-screen-protocol
    multiplex-screen-session
    multiplex-screen-video
)

if ! docker info >/dev/null 2>&1; then
    echo "no Docker daemon answered; skipping the Linux screens check"
    exit 0
fi

# The desktop crate needs gpui's system libraries as well; the list matches ci.yml.
DESKTOP_PACKAGES=""
if (( DESKTOP )); then
    DESKTOP_PACKAGES="libxkbcommon-dev libxkbcommon-x11-dev libssl-dev libfontconfig1-dev \
libdbus-1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev tmux"
fi

docker build -q -t "$IMAGE" - >/dev/null <<DOCKERFILE
FROM rust:1.97-trixie
RUN apt-get update -qq \
 && apt-get install -y -qq libpipewire-0.3-dev clang pkg-config $DESKTOP_PACKAGES \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy
DOCKERFILE

if (( DESKTOP )); then
    # The desktop crate pulls in most of the workspace, so the repository's own manifest is used
    # rather than a synthesised one. apps/ is excluded because it is the mobile sources: 300 MB
    # that no Rust build here reads.
    tar -C "$ROOT" --exclude=target --exclude=.git --exclude=dist --exclude=tools --exclude=apps \
        -cf - . \
      | docker run --rm -i -w /w \
          -v termirust-linux-registry:/usr/local/cargo/registry \
          -v termirust-linux-desktop-target:/target \
          -e CARGO_TARGET_DIR=/target \
          "$IMAGE" bash -c "
            set -e
            mkdir -p /w && cd /w && tar -xf -
            cargo $SUBCOMMAND -p multiplex $ARGS
          "
    exit
fi

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
