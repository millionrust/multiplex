# RS1 Remote Screens Codec Evidence

Date: 2026-09-15

## Outcome

`crates/termirust-screen-codec` implements milestone M1 of
[remote-screens-implementation-plan.md](../remote-screens-implementation-plan.md): the
platform-free tile codec, with no capture, network, or UI code.

- A surface is split into 64×64 tiles. Only tiles whose xxh3 hash changed are encoded, and
  operating-system damage limits which tiles are read.
- Each changed tile is skipped, sent as a solid colour, referenced from the viewer's cache,
  covered by a vertical move, sent losslessly (text and UI), or sent as a lossy first pass
  (pictures) that is refined to exact pixels once idle.
- Batches use a bounded, hand-written big-endian format (`TSB1`) pinned by a golden test.
- The viewer's cache and the host's shadow evict in the same order; misses are repaired.
- A viewer that reconnects receives only tiles, moves, and cache entries after its last
  acknowledged batch.
- A fast-changing area of at least 12 tiles and 3×3 tiles in extent becomes the motion region,
  sent lossy at no more than 8 updates a second on the tile path, and resent when it ends.

The crate adds one dependency, `xxhash-rust 0.8.18` (BSL-1.0, `xxh3` only). It reuses the
already-locked `miniz_oxide 0.8.9` and `proptest 1.11.0`. The controller-security ADR records the
lockfile review and repinned checksums.

## Automated evidence

```text
cargo test -p termirust-screen-codec
PASS: 68 tests (unit, round trip, resume, motion, refinement, workloads), 0 failed

cargo clippy -p termirust-screen-codec --all-targets -- -D warnings
PASS

./scripts/verify/controller-security-vectors.sh --check
PASS: 3 golden-vector tests passed; ADR and lockfile checksums verified
```

Property tests drive random text sessions (scrolls, typing, flat edits, with and without damage
hints) and random sessions with outages and resume; every delivered frame must be pixel-exact.

## Fuzzing

Targets live in `crates/termirust-screen-codec/fuzz` (own workspace). Built with the nightly
toolchain and libFuzzer coverage flags, each ran for 60 seconds without a crash:

| Target | Runs | Coverage | Corpus |
|---|---:|---:|---:|
| `batches` (decode, apply, re-encode) | 4,915,350 | 490 | 361 inputs |
| `tile_payloads` (lossless and lossy decoders) | 1,971,691 | 256 | 421 inputs |

## Workload report

`cargo run -p termirust-screen-codec --release --example workload_report` on a 1512 × 982
surface at 30 frames a second, synthetic content:

| Workload | Seconds | Batches sent | Total KB | KB/s | Largest batch KB | Target (plan 4.8) |
|---|---:|---:|---:|---:|---:|---|
| Idle desktop | 9.9 | 0 | 0.0 | 0.0 | 0.0 | < 1 kbps |
| Typing in an editor | 9.9 | 75 | 18.7 | 1.9 | 0.5 | 10–40 KB/s |
| Scrolling a code file | 3.0 | 30 | 91.5 | 30.8 | 3.9 | 30–120 KB/s |
| Switching windows | 9.9 | 5 | 44.6 | 4.5 | 33.5 | ≤ 300 KB burst, then ~0 |
| Video region (tile path) | 5.0 | 45 | 1,018.9 | 205.8 | 24.2 | ≤ 600 kbps |

The workload test holds idle, typing, scrolling, and window switching to these targets. The
report found and fixed one defect: scrolled text was promoted to a motion region (207 KB/s
before the fix, 31 KB/s after).

## Open items

- **Video on the tile path is above target** (1.6 Mbps against 600 kbps) on this noise-heavy
  synthetic video. Stage A tuning (lower detail inside the region) and the Stage B motion
  stream address it; the release gate does not include it.
- **Content is synthetic.** Real captures and dirty-rect statistics come from the
  ScreenCaptureKit spike (step 0.2).
- **A lost scroll resends the scrolled area** after resume, because the host keeps no copy of
  the viewer's older frame.
- **Frames must be opaque.** Alpha is not transmitted; capture backends deliver opaque frames.
