# E07 Infrastructure Access Depth

- Status: complete
- Completed: 2026-08-30
- Branch: `test`
- Aggregate verifier: `scripts/verify-infrastructure-access.sh`

## Completed Children

1. E07.1 defines the shared SSH access capability and policy contract with secret-free projection.
2. E07.2 supports OpenSSH user-certificate authentication with hostile certificate fixtures.
3. E07.3 provides constrained SSH-agent authentication and explicit forwarding policy.
4. E07.4 provides bounded key generation and safe, reviewed public-key deployment.
5. E07.5 records and enforces the Mosh lifecycle defer decision instead of claiming unsafe
   continuity.
6. E07.6 excludes Telnet and defers serial transport behind an isolated future architecture.
7. E07.7 completes bounded SOCKS5 and HTTP CONNECT proxy routing plus forwarding acceptance.
8. E07.8 provides the bounded resilient SFTP transfer manager, conflict handling, cancellation,
   resume validation, and checksums.
9. E07.9 provides strict read-only connection and jump-route diagnostics with bounded bulk work
   and consolidated hostile acceptance.

The detailed contracts, commits, tests, and limits for each child are recorded in
`completion-evidence/e07.1-ssh-access-policy-contract.md` through
`completion-evidence/e07.9-connection-diagnostics-bulk-hostile-acceptance.md`.

## Exit-Gate Evidence

- `./scripts/verify-infrastructure-access.sh` passed every E07 verifier in sequence: policy,
  certificate authentication, agent access, key lifecycle, Mosh decision, weak/local transport
  decision, proxy and forwarding, SFTP transfer management, and connection diagnostics.
- The aggregate fixtures cover normal operation, hostile input, timeout or stall, cancellation,
  credential denial, host-key mismatch, bounded resource behavior, and recovery for every capability
  claimed by the reference implementation.
- `cargo test -q -- --test-threads=1` passed with 633 main tests, 3 expected ignored tests, and both
  integration binaries green.
- Formatting, changed-line Clippy, and whitespace checks passed. The worktree was clean before this
  evidence-only closeout, and disk headroom was 18 GiB.

## Scope and Honest Deferrals

The supported reference scope is the macOS desktop implementation plus Linux Docker fixtures used
for protocol acceptance. Native-platform secure-store and release claims remain subject to their
platform gates. Mosh is deliberately deferred because the current Host/session lifecycle cannot
yet preserve its semantics safely. Telnet remains excluded; serial remains deferred until it has a
separate local-device trust and permission model. No public relay is operated, no network discovery
or continuous monitoring was introduced, and weak transports never reuse SSH security labels.
