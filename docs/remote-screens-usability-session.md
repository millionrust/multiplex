# Stage A usability session (release gate 2a)

> **Gate 2a:** Stage A is judged usable on its own — a 30-minute session of editing and reading over
> the 300 ms / 5 % / 1 Mbps profile with no complaint other than speed.

This is the gate that decides whether Stage A can ship while Stage B is still open, and it is the
only gate that cannot be turned into a number. Everything else in the matrix measures whether the
picture arrives; this asks whether a person can work in it.

That phrase — **"no complaint other than speed"** — is the whole test, and it needs saying plainly
before you start: the session is *expected* to feel slow. 300 ms round trip and 1 Mbps is a bad
link, and a slow screen on a bad link is the design working. The gate fails on anything else: text
you cannot read, a cursor you cannot place, a screen that stops and stays stopped, input that
arrives out of order or twice, a session that drops, colours that are wrong, or a moment where you
cannot tell whether the app or the link is broken.

## Setup

```bash
# 1. Find the listening port with the app running.
lsof -nP -iTCP -sTCP:LISTEN | grep -i termirust

# 2. Shape only that port -- not the whole Mac, which would also throttle the host.
sudo scripts/bench/condition-link.sh up 3 --port <port> --ttl 2700

# 3. Work for thirty minutes. Then:
sudo scripts/bench/condition-link.sh down
```

Use a phone you would actually use, on Wi-Fi. Stage A rides the Controller channel, so no relay and
no iroh feature are involved.

**Do real work.** A scripted tour will not find anything: the failures worth catching are the ones
that appear when you stop thinking about the software. Read a file you actually need to read. Fix
something. Run a build and watch it scroll.

## What to do, roughly

Thirty minutes, unhurried, spread across:

- **Reading** — open a long file, scroll it, find something. Scrolling is the workload the tile
  path handles worst and the one most likely to feel wrong.
- **Editing** — type into a file for several minutes continuously. Typing is the workload the
  targets are built around, and the one where latency is felt rather than seen.
- **Watching output** — start a build or a log tail and let it run. This is where the ladder should
  visibly give things up, and where a stall would show.
- **Switching** — move between windows and workspaces. Window switching is the third workload in
  the 4.8 targets and the one that produces the largest single frame.
- **Leaving and coming back** — lock the phone, walk away for a minute, return. Resume is a path
  people hit constantly and tests rarely cover.

## The observation sheet

Fill this in *during* the session, not after. Anything you can remember at the end was severe; the
gate turns on the things you would otherwise forget.

| | |
|---|---|
| Date / tester | |
| Phone and OS | |
| Host (machine, macOS version) | |
| Profile confirmed active | `condition-link.sh status` output |
| Minutes actually worked | |

**Count these.** A tally is worth more than an adjective.

| Observation | Count | Notes |
|---|---:|---|
| Screen stopped for longer than you would tolerate | | how long, doing what |
| Text you could not read at the first pass | | |
| A tap or key that did nothing | | |
| A tap or key that landed in the wrong place | | |
| Input that arrived twice | | |
| Session dropped or had to be reopened | | |
| Wrong colours, torn frame, stale region | | |
| You could not tell whether the app or the link was at fault | | **this one matters most** |
| Anything that made you stop working | | |

**Then the judgement, in your own words:**

- Could you have done this work for real, only slower? yes / no
- What was the worst moment, and what were you doing?
- Did anything make you distrust what you were seeing?
- Anything you would not have noticed if you had not been looking for it?

## The verdict

**Pass** — you worked for thirty minutes, the counts above are zero or explained, and every
complaint reduces to "it was slow".

**Fail** — anything else. Record it and stop; do not re-run hoping for a better session. A gate that
is re-rolled until it passes is not a gate.

Write the outcome into `docs/engineering-evidence/` as `RS9-stage-a-usability.md`, including the
filled sheet even on a pass — especially the last question, because "things you only noticed because
you were looking" is where the next round of work comes from.

## A note on who should run it

Ideally not the person who wrote it. Someone who knows the app but not this branch will notice
things the author has trained themselves to ignore — that is the same reason gate 4 asks for an
independent reviewer. If it has to be the author, write the sheet first and fill it honestly.
