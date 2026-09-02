# N02 Real Bundled Desktop And Host Golden Run

**Evidence date:** 2026-09-02
**Status:** Complete

## Scope

N02 proves the real macOS application bundle and separate Rust Session Host
process model against disposable local fixtures. The evidence contains bounded
result metadata only. It does not contain credentials, copied key material,
application state, terminal content, or private environment values.

## Environment

- Host: macOS Darwin 25.5.0, arm64
- Rust: `rustc 1.97.1`, `cargo 1.97.1`
- Docker client/server: 29.5.3, daemon available
- Bundler: `cargo-bundle 0.11.0`
- SSH target: disposable loopback OpenSSH container built from the repository fixture
- Desktop app: real unsigned `TermiRust.app` release bundle
- Free repository volume space after verification: 50 GiB

## Golden Workflow

The single command below builds and runs the complete fixture:

```bash
./scripts/verify-desktop-host-golden-run.sh
```

The separate-process integration starts three real
`termirust-session-host` children:

1. a local PTY shell with deterministic input/output markers;
2. a real OpenSSH client connected to the disposable Docker server; and
3. a fingerprint-verified deterministic fake-agent executable represented as a
   managed-agent Session.

It then opens the real authenticated and encrypted Controller channel, lists all
three typed Sessions, attaches to the managed agent, acquires writer authority,
sends input, and releases authority. After the first Controller closes, direct
Host input creates output beyond the recorded watermark. A second Controller
reattaches from that exact watermark, receives the identical tail, reacquires
writer authority, and sends new input. Every collected sequence is asserted to
increase by exactly one, with no gaps or duplicates. Revoking the paired device
closes the Controller path and rejects stale input.

The same command builds and launches the real release application bundle with
an isolated config. The app restores one local PTY and one Docker-backed SSH
workspace. Fixture-owned markers prove both startup paths completed, the Docker
log proves public-key authentication, and isolated known-host storage proves
host trust was persisted.

## Results

The final golden command passed three consecutive complete executions:

| Assertion group | Result |
|---|---|
| Three separate Host processes become ready | PASS, 3/3 runs |
| Local PTY and Docker SSH process input/output | PASS, 3/3 runs |
| Fake-agent Session classification and runtime fingerprint | PASS, 3/3 runs |
| Controller close/reopen and exact-watermark replay | PASS, 3/3 runs |
| Contiguous output ordering without gaps or duplicates | PASS, 3/3 runs |
| Writer acquire/release/transfer and post-revocation denial | PASS, 3/3 runs |
| Real release app restores local PTY and SSH | PASS, 3/3 runs |
| Owned process, container, credential, and state cleanup | PASS, 3/3 runs |
| `./scripts/verify-product-model.sh --local` | PASS |
| Changed Rust lines are Clippy-clean | PASS |
| Rust, Swift, and Kotlin repository diff hygiene | PASS |

The integration test also has an explicit non-live path. Without
`TERMIRUST_N02_HOST_BIN`, it prints `SKIPPED N02 live golden run` and exits
successfully so ordinary local tests do not imply that live execution occurred.

Adding the Session Host as an integration-test dependency changed `Cargo.lock`.
The controller-security fixture's reviewed lockfile checksum was updated to the
new SHA-256 while its cryptographic vectors and security-ADR checksum remained
unchanged. All three controller-security golden-vector tests then passed, and
the uninterrupted full product-model baseline passed afterward.

## Cleanup And Limits

Each run records the exact pre-run app and Host process sets, terminates only the
owned app process group, stops every Host through the real protocol, and verifies
the post-run sets match. It removes the uniquely named Docker container and the
guarded temporary directory containing the copied fixture key and isolated app
state. Final audits found no surviving N02 process or container.

The pre-existing Swift project-file modification was preserved with SHA-256
`9cbdb068887ddab65f15080e1932f36658c369f8d7c344371d2f369abb5ea25e`;
the Kotlin repository remained clean.

N02 does not claim Android, iOS/iPadOS, Windows, Linux desktop, external-server,
or real provider-agent execution. Swift source/lifecycle verification and the
Android unit/debug-build gate passed, but iOS runtime execution was explicitly
skipped because no eligible runtime is installed. Those remain separate queued
outcomes.
