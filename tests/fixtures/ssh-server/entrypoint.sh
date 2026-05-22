#!/bin/sh
set -eu

mkdir -p /run/sshd
exec /usr/sbin/sshd -D -e
