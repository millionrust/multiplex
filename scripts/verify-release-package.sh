#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
  printf '%s\n' 'usage: verify-release-package.sh DIRECTORY' >&2
  exit 2
fi

directory=$1
test -d "$directory"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) suffix=.exe ;;
  *) suffix= ;;
esac

for name in termirust termirust-cli termirust-session-host termirust-mcp termirust-mcp-authorize termirust-relay; do
  path="$directory/$name$suffix"
  if [ ! -f "$path" ]; then
    printf 'missing required release executable: %s\n' "$path" >&2
    exit 1
  fi
  if [ ! -s "$path" ]; then
    printf 'empty required release executable: %s\n' "$path" >&2
    exit 1
  fi
done

printf 'PASS: release package contains the desktop application and all required sidecars\n'
