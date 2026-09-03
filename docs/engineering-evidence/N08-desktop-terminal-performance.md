# N08 Desktop Terminal Performance Evidence

Date: 2026-09-02

## Outcome

Desktop terminal output now has a separately invalidated GPUI terminal-grid entity, bounded
output-event coalescing, and a revision-aware snapshot cache that reuses unchanged rows.
Ordinary output in the active pane no longer notifies the `TermiRustApp` root solely to repaint
terminal cells. Root invalidation remains intentional for lifecycle changes, inactive-workspace
unread state, terminal search, and accessibility announcements.

## Implementation

- `src/terminal.rs` caches the current viewport snapshot by terminal revision and theme. It
  compares rows exactly and shares unchanged rows through `Arc<TerminalRow>`.
- `src/ui/app/terminal_grid.rs` owns terminal-cell rendering as a GPUI entity with independent
  invalidation.
- `src/ui/app/mod.rs` coalesces adjacent output for one session up to 256 KiB, updates terminal
  entities after output/resize/search/selection/scroll changes, and retains root updates only for
  broader application state.
- `src/ui/app/workspace.rs` embeds the terminal-grid entity instead of rebuilding terminal rows in
  the workspace render function.
- `scripts/bench-desktop-terminal.sh` runs the fixed component and rendered-entity profiles.

## Fixed Fixtures And Results

Environment: macOS 26.5.2, Apple silicon, Rust/Cargo 1.97.1, optimized Cargo test profile.

Command:

```sh
./scripts/bench-desktop-terminal.sh
```

Component profile:

| Measurement | Result | Regression threshold |
|---|---:|---:|
| 120x40 terminal create plus initial snapshot p50 / p95 / p99 | 333 / 365 / 384 us | p99 < 20 ms |
| Single-byte parse plus snapshot p50 / p95 / p99 | 262 / 292 / 304 us | p99 < 10 ms |
| 2,162,688-byte sustained parse plus snapshot throughput | 62.13 MiB/s | >= 10 MiB/s |
| Sustained fixture parser batches | 33 | fixed by fixture |
| Sustained fixture rows scanned / rebuilt | 1,320 / 72 | viewport bounded |

Rendered GPUI entity profile:

| Measurement | Result | Regression threshold |
|---|---:|---:|
| Test window, app state, pane, and first settled render | 25 ms | < 5 s |
| Single-byte output event to settled GPUI test render p50 / p95 / p99 | 1,520 / 1,636 / 1,698 us | p99 < 100 ms |
| 4 KiB output event to settled GPUI test render p50 / p95 / p99 | 3,054 / 4,332 / 4,603 us | p99 < 100 ms |
| Rendered sustained-output throughput | 1.25 MiB/s | >= 0.5 MiB/s |
| Process CPU during synchronous rendered-output fixture | 99.8% of one core | recorded, not a portable threshold |
| Peak RSS / peak growth during rendered fixture | 144.9 / 64.7 MiB | peak < 2 GiB; growth < 128 MiB |
| Child-grid renders across 64 input and 64 output samples | 128 | >= 64 |

The CPU result is expected for a deliberately saturated, single-threaded synchronous test loop;
it is not an idle-CPU measurement. GPUI `run_until_parked` measures output event through settled
test rendering, not physical-display scanout or user keystroke transport across SSH.

## Bounded Work Contracts

- A second unchanged snapshot is a zero-scan cache hit.
- A one-line mutation rebuilds one row; a newline plus cursor movement rebuilds at most two rows.
- A terminal containing more than 1,000 scrollback rows snapshots only its 8-row by 40-column
  viewport in the bounded fixture.
- Adjacent same-session output is coalesced without crossing lifecycle events, sessions, or the
  256 KiB cap.
- A GPUI characterization test proves active terminal output repaints the child grid without an
  app-root notification when accessibility/search/unread state is unchanged.

## Verification

Passed:

```text
cargo test active_terminal_output_invalidates_only_the_terminal_grid --bin termirust -- --test-threads=1
cargo test terminal_output_drain --bin termirust -- --test-threads=1
cargo test terminal::tests:: --bin termirust -- --test-threads=1
cargo test e2e_canvas_local_shell_paste_confirmation_and_search --bin termirust -- --test-threads=1
cargo test e2e_workspace_search_navigates_between_results --bin termirust -- --test-threads=1
cargo test e2e_copy_on_select_copies_selection_to_clipboard --bin termirust -- --test-threads=1
cargo test e2e_pane_context_menu_click_copy_paste_clear_and_close --bin termirust -- --test-threads=1
cargo test e2e_canvas_terminal_paging_shortcuts_adjust_scrollback --bin termirust -- --test-threads=1
cargo test e2e_canvas_terminal_clipboard_shortcuts_copy_and_cancel_multiline_paste --bin termirust -- --test-threads=1
python3 scripts/clippy-changed.py
cargo test -q
```

The final full workspace run passed 663 root tests with 4 intentional ignores, followed by all
1, 3, 1, and 4-test integration binaries. Docker-backed SSH/tmux fixtures executed in that run.

## Remaining Qualification Scope

N08 provides repeatable local regression measurements and bounded-work contracts. N15 still owns
idle CPU, physical-display frame pacing, multi-hour high-output soak, memory-pressure behavior,
remote-network latency, and cross-platform measurements on Windows and Linux hardware.
