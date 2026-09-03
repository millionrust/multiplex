# N09 GPUI Decomposition Evidence

Date: 2026-09-02

## Outcome

Two high-frequency/cohesive responsibilities no longer live in the main GPUI coordinators:

- `terminal_grid.rs` owns independently invalidated terminal-cell presentation.
- `canvas_agent_runtime.rs` owns structured-provider handles, bounded transcript/context state,
  transcript auto-follow, and transcript selection extraction.

This was an incremental move with unchanged persisted models, provider protocols, Canvas actions,
and rendering behavior. No rewrite or new dependency was introduced.

## Enforced Dependency Direction

`scripts/verify-gpui-boundaries.py` reads locked Cargo metadata with all features enabled and walks
the complete dependency closure of:

- `termirust-domain`
- `termirust-store`
- `termirust-protocol`
- `termirust-host-protocol`
- `termirust-relay-protocol`

The gate fails with the exact dependency path if `gpui` or `gpui-component` becomes reachable.
It is invoked by `scripts/auto-test.sh` and both `focused` and `workspace` modes of
`scripts/verify-rust.sh`, so local baseline and CI verification enforce the same boundary.

Observed result:

```text
GPUI dependency boundaries passed: termirust-domain, termirust-host-protocol,
termirust-protocol, termirust-relay-protocol, termirust-store
```

## Size And Ownership

Before this extraction, `src/ui/app/canvas.rs` was 11,814 lines and mixed provider-handle dispatch
and bounded transcript ownership with Canvas interaction/rendering. The moved runtime is now a
separate sibling module. N08 also removed terminal-row rendering from `workspace.rs` and made it a
dedicated entity, reducing output-driven coupling to `TermiRustApp`.

Large-module reduction remains incremental by design. `TermiRustApp` continues to coordinate
application-wide state; Canvas continues to coordinate geometry, interaction, orchestration, and
rendering. Later decomposition must preserve these characterized boundaries rather than start a
parallel architecture.

## Verification

Passed:

```text
python3 scripts/verify-gpui-boundaries.py
cargo check -p termirust --all-targets --all-features --locked
python3 scripts/clippy-changed.py
cargo test ui::app::canvas::tests:: --bin termirust -- --test-threads=1
```

The Canvas filter passed all 29 geometry, persistence, transcript, orchestration, grouping,
capacity, and remote-bootstrap characterization tests.

The final complete Rust suite after extraction also passed: 663 root tests passed with four
explicitly ignored performance/soak tests, followed by all integration suites (1, 3, 1, and 4
tests respectively) with no failures.

## Explicit Non-Goals

- No domain or protocol type moved into a UI module.
- No persistence schema changed.
- No provider process or cancellation behavior changed.
- No attempt was made to split every large file in one change.
