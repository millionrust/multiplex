# Mosh Lifecycle Decision

Status: deferred; no product capability is exposed

Reviewed: 2026-08-30

## Decision

TermiRust does not ship a Mosh transport in the current architecture. This is a product and
security decision, not a claim that Mosh is unsafe or unsuitable as a standalone tool. The
official Mosh implementation is mature and purpose-built for roaming interactive terminals, but
adapting it honestly requires a separate native runtime, packaging, lifecycle, and terminal-state
boundary that the current SSH-channel abstraction does not provide.

No Mosh selector, saved setting, availability badge, fallback, or placeholder is added. Existing
SSH, tmux persistence, Controller/relay access, SFTP, remote execution, jump hosts, and forwarding
continue to mean exactly what they mean today.

## Protocol And Lifecycle Findings

The official `mosh` wrapper first authenticates over SSH and starts `mosh-server`. The server
chooses a UDP port and a 128-bit session key, reports both through the SSH bootstrap output, and
then the SSH connection closes. The long-lived terminal is a separate encrypted State
Synchronization Protocol session over UDP, normally using ports 60000-61000. The server exits if
no client reaches a newly started session within 60 seconds. A connected server may otherwise
wait indefinitely for that same client to return unless an administrator configures a network
timeout.

Mosh synchronizes terminal screen state rather than carrying an SSH byte stream. Roaming works
while the same `mosh-client` process and secret remain alive. Closing TermiRust would terminate
that local process and cannot be presented as reopen persistence. Persisting the UDP session key
to recreate a client would introduce a new bearer-secret store and an unsupported recovery
contract; TermiRust will not do that.

Mosh requires direct client-to-server UDP reachability. An SSH jump chain can bootstrap a server
but does not carry the later UDP traffic. SSH local/remote/dynamic forwarding, agent forwarding,
SFTP channels, remote exec channels, SSH keepalives, and host-channel reconnect are not Mosh
features. There is no silent SSH fallback when UDP is blocked because that would change the
selected transport and its lifecycle without consent.

## Integration Options Considered

### Invoke the `mosh` wrapper

Rejected. It delegates bootstrap to an external `ssh` executable and therefore cannot guarantee
TermiRust's selected saved credential, certificate pairing, local-agent limits, one-shot agent
forwarding policy, jump-chain implementation, TOFU/pinned host-key decision, timeout, or redacted
diagnostics. Passing secrets or complex policy through shell arguments/configuration is outside
the accepted SSH access contract.

### Bootstrap with `russh`, then invoke `mosh-client`

Architecturally possible, but not shippable yet. It would require a strict parser for bounded
`MOSH CONNECT` output, a zeroizing in-memory session key, a controlled child environment, direct
UDP endpoint selection, a PTY adapter for the external terminal client, cancellation and orphan
cleanup, and explicit incompatibility checks for every SSH-only feature. The local binary is not
present on the reviewed macOS machine and is not part of TermiRust packaging.

### Embed or rewrite Mosh

Rejected. The official implementation is a security-sensitive C++/Protocol Buffers application,
not a stable embeddable Rust library. A custom SSP/Mosh implementation would create a new
cryptographic and terminal emulator and is explicitly outside this milestone.

## Platform And Packaging Matrix

| Target | Current result | Reason |
| --- | --- | --- |
| macOS arm64 desktop | unavailable | No bundled/signed universal client, lifecycle adapter, or release acceptance; local `mosh` and `mosh-client` were absent during review. |
| Linux x86_64 desktop | unavailable | Distribution packages exist, but TermiRust does not package or verify the native client and its C++/Protobuf/ncurses dependencies. |
| Windows desktop | unavailable | No first-class TermiRust native adapter or packaging proof; Cygwin/WSL is not equivalent to a native supported transport. |
| iOS | unavailable | The native app cannot depend on spawning a desktop executable; a reviewed native library/port and background-network lifecycle would be required. |
| Android | unavailable | The native app needs a reviewed native library/port and mobile lifecycle evidence, not a desktop process assumption. |

No platform is therefore advertised as Mosh-capable. D01 platform evidence is required before a
future row can change any entry to available.

## Finite Prerequisites For Reconsideration

1. Select and pin an auditable upstream Mosh client implementation that can be embedded or
   distributed for each claimed target without invoking external OpenSSH.
2. Define a transport-neutral session runtime so screen-state protocols do not masquerade as
   SSH byte streams, including truthful resize, terminal history, output sequencing, hosted
   session, restore, and app-shutdown semantics.
3. Specify an in-memory-only bootstrap secret boundary, bounded parser, child/native ABI,
   cancellation, crash cleanup, server-orphan handling, and redacted diagnostics.
4. Add a capability validator that rejects jump-only endpoints and every SSH-only feature, and
   requires an explicit UDP port/range plus a preflight that distinguishes blocked UDP from a
   missing server binary.
5. Produce signed packaging and live native acceptance for each supported platform, including
   loss, reordering, Wi-Fi/cellular roaming, suspend/resume, resize, Unicode, hostile bootstrap
   output, timeout, cancellation, shutdown, and upgrade compatibility.

Until all five are funded as a dedicated transport milestone, the supported recommendation for
durable remote work remains verified SSH plus tmux, with Controller/relay routes for approved
mobile access.

## Primary Sources

- [Mosh project README](https://github.com/mobile-shell/mosh) - official bootstrap, binaries,
  UDP, roaming, and session-key environment requirements.
- [Mosh architecture and FAQ](https://mosh.org/) - SSP screen-state synchronization, UDP
  heartbeats, roaming, authenticated encryption, and operational behavior.
- [Mosh paper](https://mosh.org/mosh-paper.pdf) - protocol and terminal-state design.
- [`mosh-server(1)`](https://github.com/mobile-shell/mosh/blob/master/man/mosh-server.1) - port
  range, 60-second initial timeout, disconnect timeout, shell, and process lifecycle.
- [Official wrapper source](https://github.com/mobile-shell/mosh/blob/master/scripts/mosh.pl) -
  bounded startup marker shape, SSH invocation, endpoint selection, and client launch.
- [Mosh 1.4.0 release](https://github.com/mobile-shell/mosh/releases/tag/mosh-1.4.0) - current
  upstream feature and packaging baseline reviewed for this decision.

## Verification

Run `./scripts/verify-mosh-decision.sh`. The gate confirms this accepted decision, its primary
source inventory, the absence of a shipped Mosh surface/runtime, formatting, and the existing
access-policy contract tests.
