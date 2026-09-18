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
  sent lossy at no more than `tile_path_max_hz` updates a second on the tile path, and resent when it ends.

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
| Typing in an editor | 9.9 | 75 | 26.4 | 2.7 | 0.6 | 10–40 KB/s |
| Scrolling a code file | 3.0 | 30 | 91.5 | 30.8 | 3.9 | 30–120 KB/s |
| Switching windows | 9.9 | 5 | 44.6 | 4.5 | 33.5 | ≤ 300 KB burst, then ~0 |
| Video region, moving picture | 5.0 | 30 | 201.4 | 40.7 | 6.9 | ≤ 600 kbps |
| Video region, worst case (noise) | 5.0 | 30 | 674.4 | 136.3 | 23.9 | bound only |

Every row with a target is inside it. The workload test holds idle, typing, scrolling, window
switching and the video region to them.

The report found and fixed one defect in the codec: scrolled text was promoted to a motion region
(207 KB/s before the fix, 31 KB/s after).

It then found two defects in **itself**, which are worth recording because a workload that does not
represent what it claims is the same failure as an untested control, and this file had been
reporting both as facts about the codec.

**Typing was one line of text and nothing else**, and measured 1.9 KB/s against a 10–40 KB/s
target. The test had only an upper bound, so being a tenth of the expected cost looked exactly like
passing. Real editors are never still: there is a caret blinking whether or not anyone types, and a
status bar redrawing on every keystroke, both landing in tiles nowhere near the text. With those in
it measures 2.7 KB/s — still comfortably under, which is now a result rather than an artefact. The
test also asserts the batch count, so a workload that stops representing typing fails instead of
looking fast.

**"Video region" was white noise.** Every pixel independent of its neighbours defeats the lossy
pass, the palette and deflate at once, so it measured 1,090 kbps and this file reported the tile
path as being over target for video. No screen looks like that. Ordinary moving picture — locally
smooth, as anything a camera or codec has touched is — measures **326 kbps, inside the target**.
The noise row is kept as a bound on the worst case and is deliberately held to no target.

## Open items

- **Content is synthetic, and that cuts both ways.** Two of these rows were wrong about the codec
  until the workloads were fixed (above). The remaining risk is the same one in reverse: a
  synthetic editor and a synthetic video are still guesses about what real screens do.
- **Content is synthetic.** Real captures and dirty-rect statistics come from the
  ScreenCaptureKit spike (step 0.2).
- **A lost scroll resends the scrolled area** after resume, because the host keeps no copy of
  the viewer's older frame.
- **Frames must be opaque.** Alpha is not transmitted; capture backends deliver opaque frames.
