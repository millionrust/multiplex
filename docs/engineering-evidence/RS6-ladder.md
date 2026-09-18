# RS6 What the degradation ladder is worth

Date: 2026-09-17

## The question

[RS5](RS5-motion-path.md) measured the ladder under a squeeze and had to admit the measurement
proved nothing: on that workload refinement was never a significant cost and the viewport covered
the window, so the ladder descended and the bytes did not move. A benchmark whose numbers cannot
move is not evidence either way. The ladder's premise — that each rung gives up something worth
having — had never been tested.

This is that test. `cargo test -p termirust-screen-host --test rungs` measures a real
`HostSession` and `ViewerSession` rung by rung over a workload built so that every rung has
something to take away, and fails if any rung costs what the one above it costs.

## The answer

| Rung | kbps | Saving against the rung above |
|---|---:|---:|
| `Full` | 1220 | — |
| `NoRefinement` | 1185 | 3% |
| `ViewportOnly` | 595 | 50% |
| `SlowerFrames` | 359 | 40% |
| `LowerQuality` | 347 | 3% |

Top to bottom the ladder takes 1220 kbps down to 347, a **72% saving** — enough to rescue a
session that is over budget, which is the only reason to have it.

Four of the five rungs did nothing at all before this measurement. Every one of them looked
correct in review, and three had been read several times.

## What the workload has to be

Almost every property of the content is there to stop a rung being tested against nothing, and the
first three attempts each failed for a different one of these reasons:

- **Photographic, not flat.** The lossy first pass and the refinement queue only apply to tiles
  that classify as `Picture` — many colours, few hard edges. Flat colour bands have a small palette
  and are sent losslessly, so a workload made of them leaves both the refinement rung and the
  quality rung with nothing to give up, and the test passes while proving nothing.
- **It has to settle.** Refinement only touches tiles idle for `REFINE_IDLE_MS` (250 ms), so a
  screen that changes every frame forever never refines. The cycle moves for ten frames and holds
  for ten.
- **It must not be video.** The changing content is in tile-aligned two-tile-tall strips separated
  by static gaps, so it can never satisfy `min_side_tiles` and be promoted to a motion region. If
  it were, the motion path would carry it and the tile throttle would flatten every rung to
  roughly the same number — the test would pass or fail on the wrong subsystem. What the motion
  path costs is RS5's measurement.
- **Content outside the viewport.** Otherwise the viewport rung is genuinely free and proves
  nothing, which is exactly the hole RS5 left.

## Four bugs, all of the same kind

Each was found by a number refusing to move, and each is a rung wired to nothing.

**A screen that sat still stopped updating for good.** The encoder's sequence counter advances for
every frame it is shown, including the ones that turn out to have changed nothing. The host drops
those empty batches rather than sending them, but counted them as outstanding, so after
`max_unacked_batches` (4) idle frames `unacked` could never come down again and the host stopped
sending anything at all — permanently, for the rest of the session. A desktop goes idle constantly,
so this would have hit every real session; it had not been seen because every workload measured so
far changed every single frame. The host now tracks the last sequence it actually put on the wire.

**`ViewportOnly` saved nothing unless the host reported damage.** Clipping was only applied to
`Pending::Rects`. A capture that cannot say what changed produces `Pending::Everything`, which fell
through to no clipping at all — so the rung was free, the ladder would take it, and exactly as many
bytes went out as before. `Pending::Everything` under this rung now clips to the viewport rect.

**`SlowerFrames` was wired to nothing.** `last_sent_ms` was only ever assigned on the thumbnail
path. For every interactive subscription it stayed `None` for the life of the session, so
`too_soon` was never true and the minimum interval had no effect whatsoever.

**`LowerQuality` never reached the encoder.** `set_limits` walks the subscriptions that exist when
it is called. The usual order is limits first, then the viewer subscribes — so the encoder was
built with the default detail and the rung changed nothing. It also meant a second surface
subscribed to while degraded came up at full quality. New interactive subscriptions now start at
the session's current detail.

## What this does not answer

- Whether the ladder picks the right rung. This measures what each rung is worth once taken;
  `Ladder::consider` deciding when to take it is covered by its own unit tests and by RS5's squeeze.
- Real captured pixels. The content is synthetic, so the byte counts are indicative and the
  relative savings are the point.
- `LowerQuality` is worth only 3% here, and `NoRefinement` 3%. Both are real and both are in the
  right order, but on this content they are close to noise; a workload with more picture area and
  longer idle periods would make them matter more. The ladder's value is overwhelmingly in the
  middle two rungs.
