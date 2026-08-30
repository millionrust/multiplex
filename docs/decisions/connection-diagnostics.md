# Connection Diagnostics Contract

Status: implemented for saved desktop SSH connections.

## Purpose

The Connections library can check whether a saved host is usable before opening a terminal. The
same action works for one selected host or a bounded selection. It is an explicit point-in-time
probe, not monitoring and not an automatic repair mechanism.

## Strict Trust Boundary

Diagnostics use the saved direct, SOCKS5, HTTP CONNECT, and jump-host route plus saved
authentication. Every hop requires an already pinned, exactly matching host key. An unknown key is
reported with instructions to connect normally and review it; diagnostics never add or replace a
known-host entry. Normal interactive connections retain the existing trust-on-first-use behavior.

The request is sanitized before transport setup:

- startup directory and command are removed
- persistent tmux mode and session name are removed
- environment entries are removed
- local, dynamic, and remote forwarding rules are removed
- SSH-agent forwarding is disabled

The probe authenticates, opens and drops one SSH session channel without requesting a PTY, shell,
or exec command, then opens the SFTP subsystem and canonicalizes `.`. It does not read remote file
content or mutate the remote host.

## Lifecycle And Limits

- Four fixed workers may run diagnostics concurrently.
- The process-wide queue holds at most 64 pending operations.
- One batch contains at most 64 unique saved profiles.
- A profile cannot have duplicate queued/running operations.
- Route/authentication is bounded to 30 seconds, channel and SFTP probes to 10 seconds each, and the
  whole operation to 45 seconds.
- Queued and active work can be cancelled. Retry is always explicit and starts a new operation.

Events identify an opaque operation and report queued, stage start/pass, completed, failed, or
cancelled. UI results are in-memory only and remain visible until cleared. They contain profile
labels, address, route class, stage, elapsed time, an allowlisted failure category, and an
actionable recovery sentence.

## Failure Categories

Unknown host key, host-key mismatch, credential denial, route unavailable, timeout, session-channel
denial, SFTP unavailability, cancellation, and internal failure remain distinct. Raw transport
errors are used only for local classification and are not projected into the results panel.

## Privacy And Non-Goals

Passwords, private/public key bodies, keychain values, agent messages, terminal bytes, commands,
environment values, remote file content, proxy endpoints, and raw server errors are excluded from
diagnostic results. This feature does not perform discovery, background monitoring, automatic
retry, automatic trust, mass terminal launch, remote repair, or cross-device synchronization.

## Verification

Run `./scripts/verify-connection-diagnostics.sh`. The live fixture proves strict non-mutating trust,
direct/proxy/jump routing, credential denial, key mismatch, session/SFTP checks, timeout,
cancellation, SFTP loss and recovery, and absence of startup/tmux/forwarding side effects. The
rendered GPUI test drives the actual batch-toolbar action and verifies no workspace opens.
