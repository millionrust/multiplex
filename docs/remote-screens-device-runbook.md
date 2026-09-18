# Remote Screens: the device runs

Four things in the plan cannot be measured without a phone: spike **0.4**, step **3.6**, step
**5.5**, and release gate **2**. Everything else in M0 and M5 is done and green.

This exists because "run the matrix on a device" is not an instruction. Each run below says what to
set up, what to type, what to write down, and what counts as a pass — so the answer comes back
comparable to the software runs rather than as an impression.

**Read first:** the software half is already measured. `cargo run -p termirust-screen-host --release
--example network_matrix` runs all 24 cells against a real `ScreenHost` and
[RS7](engineering-evidence/RS7-network-matrix.md) has the table. Two of the four criteria — no stall
over 1 s, converged within 3 s of a 10 s outage — are properties of the protocol and the codec, and
a real network adds nothing to them but noise. They pass on every cell. **The device runs exist for
the other two**: input-to-glass P95, and how real phones behave. If a device run fails a criterion
RS7 already passes, suspect the measurement before the protocol.

## Definitions, so two runs mean the same thing

Settled by the owner on 2026-09-18 and repeated here because they are the whole difference between
a number and an opinion:

- **Converged** — the viewer is showing the current screen, with picture regions at their lossy
  first pass. Not every tile refined to exact pixels. The two differ by seconds on a slow link, and
  exactness is bounded by the link: 200 kbps cannot make a 1280 × 800 screen exact any faster.
- **Stall** — the screen stopped moving. Not that it is imprecise. A playing video is never exact,
  because refinement only runs once a region goes idle.
- **Input-to-glass** — the wall time from the key going down on the phone to the resulting pixel
  appearing on the phone's screen. Measured with a camera, not a log; a log cannot see the display
  pipeline, and the display pipeline is most of the tail.

---

## Before the phones: what a simulator can and cannot settle

A good deal of this does not need hardware, and waiting for a phone to learn things a simulator
would have told you is waste. But a simulator settles some criteria and actively misleads on
others, so the split matters more than the convenience.

**Shape the link with `scripts/bench/condition-link.sh`**, which drives dummynet through `dnctl` and
`pfctl` — both already on macOS, no install. It takes the four profiles by number.

```bash
lsof -nP -iTCP -sTCP:LISTEN | grep -i termirust      # find the port
sudo scripts/bench/condition-link.sh up 3 --port 51820
# ... run the session ...
sudo scripts/bench/condition-link.sh down
```

It shapes **one port**, deliberately. Network Link Conditioner shapes every interface, and here the
screen *host* is on the same Mac as the viewer being measured — throttle the machine and you
degrade both ends, so the picture is worse for reasons the matrix is not asking about. Scoping to
the session's port leaves the captured screen, the build, and everything else at full speed. The
script keeps its rules in their own pf anchor, refuses to stack one profile on another, and
installs a dead-man's switch that reverts after an hour if the terminal dies.

For Android, the emulator has its own shaper and does not need any of this:

```bash
emulator -avd Pixel_8_API_34 -netdelay 300 -netspeed 1m    # at launch
adb emu network delay 300                                   # or live
adb emu network speed full                                  # restore
```

### What this settles

- **No stall longer than 1 s**, and **converged within 3 s of a 10 s outage.** Both are properties
  of the protocol and the codec. RS7 already holds them across all 24 cells in the harness; a
  simulator run confirms the real client code holds them too, over a real socket.
- **No keyframe on the video path after a single lost datagram.** This is a wire observable — watch
  what the host sends after dropping one. It needs a decoder that reports loss, not a decoder with
  representative performance, so a simulator answers it.
- **The degradation ladder reaching sane rungs**, and the client not falling over on a bad link.

### What it does not settle, and must not be reported as if it did

- **Input-to-glass P95.** The simulator's display pipeline is the Mac's, not a phone's. No ProMotion,
  no device compositor, no real GPU scheduling. This is the criterion the device runs exist for and
  the one a simulator number would quietly corrupt.
- **The hardware decode path.** The Simulator runs on host macOS media frameworks rather than
  emulating iOS hardware, returns `nil` where a device returns a value for some capability queries,
  and there are reported cases of HEVC working in the Simulator and failing on real hardware. The
  entire premise of M4 is hardware long-term references — that is the last thing to trust a
  simulator about. Same for MediaCodec on the Android emulator, which is generally software.
- **0.4, at all.** There is no carrier NAT in a simulator, so there is nothing to punch through and
  nothing to fall back to a relay for.
- **Thermal and battery behaviour**, which is what a half-hour of video on a phone actually tests.

**So:** run the simulator pass first and record it as a simulator pass. It should catch anything
gross before a phone is involved, which is the point. It closes no gate on its own — gate 2 says
"on a real iPhone over cellular and on Wi-Fi with the conditioner", and it means it.

---

## 0.4 — iroh over cellular, with a self-hosted relay

**The question:** can a phone on a carrier network reach a Mac behind another NAT, and if it needs
a relay, is the relay affordable? Everything about Stage B's transport rests on this, and loopback
cannot answer any part of it — there is no NAT to punch and no path to migrate between.

**Setup**

1. Deploy a relay by [self-hosted-iroh-relay.md](self-hosted-iroh-relay.md). Allow one hour; the
   guide is written and unrun, so expect its iroh 1.2 config keys to need checking against the
   version you install. **Correct the guide as you go** — that is part of this run.
2. Mac on home Wi-Fi, behind its normal router. Do not put it on a public IP; that removes the NAT
   this spike is about.
3. Phone on **cellular only**. Wi-Fi off, not merely "not connected" — iOS will quietly prefer a
   remembered network.

**Run**

```bash
# On the Mac, with the transport feature on:
cargo test -p termirust-screen-transport --features iroh --test quic_route -- --nocapture
```

That is the loopback baseline; it must pass before the phone is worth involving.

Then, from the phone, open a screen session and record:

| What | How | Why it matters |
|---|---|---|
| Did it connect at all? | yes/no | The go/no-go in one bit |
| Time to first picture | stopwatch | Versus the Stage A route over the same link |
| Direct or relayed? | Devices screen reports it | Hole punching working is the difference between paying for one handshake and paying for every pixel |
| If relayed, does it *become* direct? | watch for 30 s | A session that stays relayed for life is the expensive case |
| Bytes through the relay | relay process counters | The input to the relay-economics question, still open in section 9 |
| Walk between rooms, then out of the building | keep watching | Migration should be invisible: QUIC keeps the connection across a path change, so a visible break is a finding |

**Pass:** it connects, reaches a picture, and either punches through within a few seconds or stays
usable on the relay. **No-go:** it cannot connect, or relayed sessions are unusable — in which case
Stage B's transport stays iroh-behind-a-feature and the plan falls back to Stage A, which is what
the `iroh` feature being off by default is for. Nothing has to be unwound.

**Write it up as** `docs/engineering-evidence/RS8-iroh-cellular.md`, and set 0.4 in the todo.

---

## 3.6 — Stage A matrix on a real iPhone and a real Android phone

**The question:** does Stage A behave on real phones the way it behaves in the harness?

**Setup** — Network Link Conditioner (Additional Tools for Xcode) on the Mac for the Wi-Fi runs;
the phone on the same network. Profiles, matching section 7:

| Profile | RTT | Loss | Cap |
|---|---|---|---|
| 1 | 20 ms | 0% | uncapped |
| 2 | 100 ms | 1% | 5 Mbps |
| 3 | 300 ms | 5% | 1 Mbps |
| 4 | 500 ms | 10% | 200 kbps |

**Run** each of typing, scrolling and video on each profile, on each phone. Record the four
criteria. **Input-to-glass P95 is the one that needs care**: film the phone at 240 fps with a second
device, tap a key, and count frames to the pixel changing. Twenty samples per cell is enough for a
P95 you can defend; five is not.

**Pass:** no stall over 1 s, converged within 3 s of a 10 s outage, **and Stage A input-to-glass P95
under RTT + 150 ms on the first three profiles**. The fourth profile has no P95 bound — at
200 kbps the link is the limit and the number would only measure the conditioner.

---

## 5.5 — The full matrix, both stages

As 3.6, plus Stage B on each cell, plus two Stage B criteria:

- **input-to-glass P95 under RTT + 40 ms** on the first three profiles. Much tighter than Stage A,
  because that is the whole argument for the motion path.
- **no keyframe on the video path after a single lost datagram.** Drop one datagram deliberately
  and watch the wire: the answer must be a reference refresh, not a keyframe. This is what the
  long-term references exist for, and RS4 measured the difference at 135× and 163×. A keyframe here
  means the LTR path is not working and the bytes are being spent for nothing.

**Then tune the steps.** The ladder's rungs and its two thresholds (`CROWDED` 0.95, `ROOMY` 0.60,
`DWELL_MS` 1500) were set against the harness. If a device run says a rung is reached too eagerly or
too late, that is the evidence for moving them — and [RS6](engineering-evidence/RS6-ladder.md) is
the warning that four of those rungs were once wired to nothing and read correctly the whole time.
Change one constant at a time and re-run the software matrix after each, because it is cheap and it
will catch a change that helps the device and breaks everything else.

---

## Gate 2 — the release gate

Gate 2 is not a separate run: it is 3.6 (for Stage A at M3) or 5.5 (for Stage B at M5), met **on a
real iPhone over cellular and on Wi-Fi with the conditioner**, for the stage being released. The
cellular half is what 0.4 also needs, so do them in one sitting.

The gate is per stage. Stage A can pass and ship while Stage B is still open — that is the point of
staging them, and Stage A is the configuration that works on every route today.

---

## What to do if a run fails

Record it and stop, rather than tuning until it passes. Every number in RS1 through RS7 that was
worth having came from a measurement that disagreed with the code, and three of them found bugs that
review had read straight past: four ladder rungs wired to nothing, a promotion decision taken a
second too early, and a rate controller that never ran at all on Stage A. A device run that fails is
the cheapest bug report this project will get.
