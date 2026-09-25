# Growing the Controller wire

Status: accepted

Reviewed: 2026-09-25

## Why

Two pieces of planned work need a new message on the Controller channel, and neither can ship
without answering the same question first:

- **`HostAddresses`** (phase 5 of `route-selection-plan.md`): the computer tells a paired phone
  the addresses it is bound to now, so a DHCP change never strands a phone that is away from home.
- **Creating a terminal from a phone** (stage 3 of the mobile revamp): `ControllerCommand` has
  `ListSessions`, `Attach`, `Input`, `AcquireWriter`, `ReleaseWriter`, `Resize`, `Approval`,
  `Detach`, `OpenScreen` and `CloseScreen`, and nothing that starts one.

The wire as it stands has no room for either, in **both** directions:

- `ControllerCommandEnvelope` carries `version: u16` and `validate()` rejects anything that is not
  `CONTROLLER_COMMAND_VERSION` (`protocol.rs:49`), so a phone that bumps the version cannot talk
  to any host already shipped.
- Commands and responses are both `#[serde(deny_unknown_fields)]`, and `ControllerResponse` is an
  internally tagged enum, so an unknown `kind` — or a new field on a known one — fails to decode
  rather than being ignored.
- A command the host cannot decode is not refused, it is fatal: `decode_command(&opened.payload)?`
  in `runtime.rs:595` propagates out of `serve_authenticated_stream`, which ends the connection.
  A newer phone asking an older host for something it has never heard of loses the session,
  including the terminal the person was reading.
- `CapabilitySet::from_bits` rejects any bit outside `KNOWN_MASK` (`0x00ff`), so capability bits
  cannot be used to announce a new message either: an older host refuses the whole connection
  rather than the one bit it does not know.

Every one of those is a deliberate fail-closed choice, and each is right for garbage on the wire.
Together they also mean the protocol cannot grow without a flag day.

## What this costs today

Nothing that is published. Every release so far is a prerelease (`v0.0.1`–`v0.0.4`), the Homebrew
cask has never run, and the phone apps are unsigned builds that have to be re-installed by hand.
The installed base is the author's own devices. A flag day is therefore cheap **now** and gets
more expensive with every release, which is the argument for settling it before either message is
written rather than after.

## Options

1. **Tolerant decoders, shipped a release ahead of any new message.** Both ends learn to ignore
   what they do not understand — an unknown response `kind` decodes to a variant the reader skips,
   an unknown command is answered with `Error { code: "unsupported_command" }` instead of closing
   the connection, and capability bits above the mask are masked off rather than refused. Nothing
   new is sent in that release; the release after it may add messages freely.
2. **Negotiate a version in the handshake.** The honest long-term answer, and the most work: the
   handshake gains a protocol-version exchange, both ends speak the lower of the two, and every
   message says which version introduced it. It does not help the builds already shipped, which
   still reject an unexpected envelope version, so it needs option 1 first anyway.
3. **Do nothing and require matching versions.** Say plainly that a phone and a computer must be
   on the same release, and make a mismatch a clear message rather than a dropped connection.
   Cheapest, and defensible while the apps are side-loaded — but it makes every future protocol
   change a synchronized update of three applications, and the phone stores cannot promise that.

## Recommendation

**Option 1 now, option 3's message as its fallback, option 2 when the wire is public.**

Concretely, in one release that adds no new message:

- `ControllerResponse` gains `#[serde(other)] Unknown`, and every reader treats it as "a message
  from a newer computer, ignore it". Responses keep `deny_unknown_fields` on their known
  variants, so a malformed `sessions` is still refused.
- `decode_command` distinguishes "not valid JSON or too large" (still fatal, still
  `MalformedFrame`) from "a command kind this build does not know" (answered with
  `Error { code: "unsupported_command", completion_unknown: false }`, connection kept).
- `CapabilitySet::from_wire` masks bits outside `KNOWN_MASK` instead of failing, and the golden
  vectors record that a device asking for a bit from the future is granted the ones this build
  knows rather than refused. Both phone apps already carry the matching rule
  (`granted and ALL_SUPPORTED_CAPABILITIES.inv() == 0` must widen to a mask).
- The envelope version check becomes "not newer than mine" rather than "equal to mine", so a
  phone one release behind still works.

Only after that release may `HostAddresses` and a create-a-terminal command be added, each gated
on a capability bit so a computer that has not granted the capability never sends or accepts one.

## What is not decided here

Whether creating a terminal from a phone is allowed at all, and under which grant — that belongs
with the command's own record, alongside the host-backend path or the explicit refusal for tmux
and console sources.

## Status note

Approved 2026-09-25. The tolerant decoders land first, in their own commit, and only then the
messages that need them. The window this record relies on — an installed base of one author's
devices — closes at the first published release, so nothing here is worth deferring.
