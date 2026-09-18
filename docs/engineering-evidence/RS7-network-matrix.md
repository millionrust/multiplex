# RS7 The network matrix, as far as software can take it

Date: 2026-09-18

## The question

Section 7 of the plan asks for a matrix: (RTT, loss, cap) ∈ four profiles × {typing, scrolling,
video} × {Stage A, Stage B}, with four pass criteria. Two of those criteria are properties of the
protocol and the codec rather than of any particular network:

- no stall longer than 1 s
- a converged screen within 3 s of a 10 s outage

A real network adds nothing to measuring those but noise and irreproducibility. The other two —
input-to-glass P95, and how a real iPhone and Android phone behave — need the device and stay
device work (3.6, 5.5).

This covers the first two, so the device runs start from a known-good baseline instead of
discovering a protocol bug through a phone.

Run it with `cargo run -p termirust-screen-host --release --example network_matrix`;
`cargo test -p termirust-screen-host --test matrix` holds the criteria. To see *why* a cell settled
where it did, `cargo run -p termirust-screen-host --release --example ladder_diag -- 3 video` traces
the estimate and the rung through one cell.

## The answer

All 24 cells pass both criteria. The host here is a real `ScreenHost`, so the rate estimator and the
degradation ladder are in the loop and the `Rung` column is what the session had actually given up
by the end of the run.

| Profile | Workload | Stage | kbps | Rung | Longest stall ms | Caught up ms | Exact ms |
|---|---|---|---:|---|---:|---:|---:|
| 20 ms, 0%, uncapped | typing | A | 18 | Full | 133 | 33 | 99 |
| 20 ms, 0%, uncapped | typing | B | 19 | Full | 133 | 33 | 99 |
| 20 ms, 0%, uncapped | scrolling | A | 28 | Full | 66 | 33 | 99 |
| 20 ms, 0%, uncapped | scrolling | B | 30 | Full | 66 | 33 | 99 |
| 20 ms, 0%, uncapped | video | A | 713 | Full | 166 | 99 | 633 |
| 20 ms, 0%, uncapped | video | B | 906 | Full | 166 | 99 | 633 |
| 100 ms, 1%, 5 Mbps | typing | A | 18 | Full | 166 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | typing | B | 19 | Full | 166 | 33 | 233 |
| 100 ms, 1%, 5 Mbps | scrolling | A | 28 | Full | 99 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | scrolling | B | 25 | NoRefinement | 99 | 233 | 233 |
| 100 ms, 1%, 5 Mbps | video | A | 675 | Full | 166 | 199 | 833 |
| 100 ms, 1%, 5 Mbps | video | B | 2,582 | Full | 133 | 266 | 1,133 |
| 300 ms, 5%, 1 Mbps | typing | A | 17 | NoRefinement | 266 | 33 | 433 |
| 300 ms, 5%, 1 Mbps | typing | B | 15 | SlowerFrames | 299 | 33 | 566 |
| 300 ms, 5%, 1 Mbps | scrolling | A | 25 | ViewportOnly | 199 | 433 | 433 |
| 300 ms, 5%, 1 Mbps | scrolling | B | 22 | ViewportOnly | 299 | 33 | 566 |
| 300 ms, 5%, 1 Mbps | video | A | 512 | SlowerFrames | 366 | 33 | 3,266 |
| 300 ms, 5%, 1 Mbps | video | B | 418 | NoRefinement | 499 | 33 | 4,199 |
| 500 ms, 10%, 200 kbps | typing | A | 14 | SlowerFrames | 366 | 33 | 799 |
| 500 ms, 10%, 200 kbps | typing | B | 13 | LowerQuality | 499 | 33 | 1,066 |
| 500 ms, 10%, 200 kbps | scrolling | A | 23 | SlowerFrames | 299 | 33 | 799 |
| 500 ms, 10%, 200 kbps | scrolling | B | 20 | LowerQuality | 533 | 1,066 | 1,066 |
| 500 ms, 10%, 200 kbps | video | A | 251 | NoRefinement | 733 | 1,766 | 6,433 |
| 500 ms, 10%, 200 kbps | video | B | 233 | ViewportOnly | 999 | 1,999 | 7,599 |

### Three things this says that are worth acting on

**The worst stall is 999 ms against a 1,000 ms bound.** Stage B video on the worst profile passes by
one frame. That is a pass and it should not be read as comfort: any change that adds a frame of
latency to the motion path fails this cell, and a real network is less tidy than this one. It is the
number to watch in the device runs.

**Stage B was not always cheaper, and now declines the regions it would make worse.** On the
uncapped profile it cost 3,073 kbps against the tile path's 713 for the same region: with bandwidth
to spare the ladder has no reason to degrade, so the encoder runs at its default bitrate, and for a
640 × 384 region of smooth content the tile path is simply cheaper. [RS5](RS5-motion-path.md)
measured Stage B 2.3× cheaper on a 896 × 512 window of harsher content. Both are true — the motion
path wins when the tile path is expensive and loses when it is not — so the owner chose
(2026-09-18) to decide it by measurement.

Both paths are now measured over the same second and the cheaper one keeps the region. That is
possible because they overlap: the tile path keeps covering a promoted region until the choice is
made, which costs about a second of sending the rectangle twice and is the only way to compare
like with like. Comparing the tile cost against the encoder's *configured bitrate* instead was
tried first and declined almost everything, because an encoder rarely spends its ceiling.

The uncapped cell now settles at 906 kbps rather than 3,073. **It does not fire everywhere yet**:
the 5 Mbps cell still runs video at 2,582 kbps against a tile path that would cost 675, so the
measurement is not yet catching every case it should. That is the next thing to look at here.

**Stage A had no rate control at all, and now it does.** Every Stage A cell used to stay at `Full`,
including 200 kbps where the session offered 251 kbps into the link and simply queued: bandwidth
reports are a Stage B feature (`FeatureSet::BANDWIDTH_REPORTS`), so a Stage A viewer never times a
burst, the host never has an estimate, and `Ladder::consider` returned early every time. The design
said as much — 5.1 added the bit "so a peer that cannot time bursts is never asked to" — but the
consequence had not been written down.

The owner chose a fallback (2026-09-18). When there is no estimate the ladder now steers on
*suppressed demand*: frames that had something to send and were refused because too many batches
were outstanding, against frames that got through. Stage A degrades on the constrained profiles and
stays at `Full` where there is room.

Getting that signal right took two attempts, and the first was wrong in an instructive way.
Counting a **full unacknowledged window** looked obvious and is not: a window fills on any
high-latency link simply because acknowledgements take a round trip to come back, which is
pipelining working rather than congestion. That version degraded a typing session using 18 kbps of
a 5 Mbps link. Counting refusals against deliveries, at a ratio of three to one, separates the two.

What the signal still cannot do is tell a small window from a slow link. A fixed
`max_unacked_batches` of 4 caps any session at four batches per round trip whatever the link
carries, so at 300 ms some of the refusals are the window talking. The ladder's rungs give up
bytes, which is the right answer to a slow link and only sometimes to a small window — **a window
that scales with the measured round trip is the better fix, and is not built.**

Stage B does adapt, and does it in the right direction: `SlowerFrames` at 5 Mbps, `NoRefinement` at
1 Mbps, `ViewportOnly` at 200 kbps, with the measured estimate settling around 9.7 KB/s against the
profile's 25 KB/s cap — the gap being what retransmission costs at 10% loss.

### A wrong finding, corrected

An earlier version of this note said the ladder stayed at `Full` on the slowest profile and
speculated that the host was "already throttled by unacknowledged batches before demand could crowd
the estimate". That was wrong, and it was wrong because of this harness rather than the product.
`ladder_diag` showed `estimate=None` for entire runs: the link handed each message to the viewer at
a single instant, so there was no arrival spread to time, and the estimator — which works by timing
how far apart a burst's bytes land — had nothing to report. `bandwidth.rs` makes exactly this point
about its own fake link, and this harness had not taken it.

Delivering in 1,400-byte chunks at the times they would really land fixed it, and the ladder
promptly descended two rungs on the profile it had supposedly ignored. The lesson is the session's
recurring one: a control that measures nothing looks identical to a control that decides to do
nothing, and only an instrument that shows the input can tell them apart.

## Three definitions the criteria needed, and why

**A stall is the screen not moving, not the screen being imprecise.** The first version measured
"the viewer's pixels are not exactly the host's", and reported every video run as one five-second
stall on a perfect link. A picture region is sent as a lossy first pass and only becomes exact once
it goes idle and refinement catches up, so a playing video is *never* exact. That measurement was
describing the codec working as designed and calling it a failure.

**"Converged" was ambiguous in the plan; the owner settled it on 2026-09-18 as the first reading
below, and section 7 now says so.** Both are still reported, because the second is worth knowing. "Caught up" is the viewer
showing the current screen, allowing the lossy first pass — which is what a person means. "Exact" is
every picture tile refined to exact pixels. They differ by seconds on a slow link: on the worst
profile a video screen catches up in 1.1 s and becomes exact at 3.8 s. The 3 s bound is asserted
against the first reading, which is the one that counts. Under the strict reading the worst profile
would fail at 3.8 s — not a defect, but a fact about the link: refinement is bounded by it, and
200 kbps cannot make a 1280 × 800 screen exact faster than that.

**Loss is modelled only where it can happen.** Stage A's tile path rides an ordered, reliable
channel, so a lost packet is retransmitted and the session sees delay and reduced goodput, never a
missing batch. Dropping tile batches to "simulate 5% loss" would model a transport nobody ships and
make the codec look broken for the wrong reason — one lost batch leaves the screen wrong forever,
which is exactly why `Class` exists. Only the video class loses anything here; reliable classes pay
for loss in retransmission time.

The outage is a real drop and resume — the host closes into the resume store and the viewer
reconnects — not a severed wire with the session left running. Modelling it the second way was the
first version's other mistake, and it made convergence never happen at all.

## What this does not establish

- **Whether the link model is fair to a real transport.** Serialisation, one-way delay and a
  retransmission penalty per lost segment is a reasonable first order, but it has no congestion
  control, no queue limit and no reordering. A real path is worse in ways this cannot predict.
- Input-to-glass P95 and real phones: device work, unchanged.
- Synthetic content, as everywhere else in this plan.
