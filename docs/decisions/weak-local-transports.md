# Weak And Local Transport Decision

Status: Telnet excluded; serial deferred; no product capability is exposed

Reviewed: 2026-08-30

## Decision

TermiRust does not expose Telnet or serial as connection types in the current product.

Plain Telnet is excluded from the product roadmap. Its default RFC 854 transport is a TCP byte
stream with option negotiation and no confidentiality, integrity, peer authentication, or safe
credential channel. Implementing it beside SSH would encourage credential use on a downgradeable
cleartext connection and conflict with TermiRust's verified infrastructure-access contract.

Direct serial terminals remain a potentially useful but deferred local-device capability. Serial
is not a Host protocol: it addresses an OS device, has baud/data/parity/stop/flow settings,
exclusive-open and hotplug behavior, and platform permission requirements. It needs a separate
saved device model and runtime instead of being fitted into SSH profiles.

The existing decorative Telnet cards, editor section, menu command, state, and port input are
removed. The unused serial icon is also removed so neither capability appears implemented.

## Telnet Security Result

RFC 854 defines a Network Virtual Terminal carried over TCP with in-band `IAC` command and option
negotiation. It does not define secure server identity or confidentiality. RFC 2946 states that
the default is `WONT ENCRYPT`/`DONT ENCRYPT`, warns that passwords otherwise travel in cleartext,
and explains that encryption alone cannot resist an active downgrade or provide data integrity.
The companion authentication negotiation also has downgrade windows unless strict combinations
are enforced.

TermiRust will not implement plain Telnet, auto-downgrade SSH, store Telnet passwords, reuse the
SSH keychain/vault/known-host UI, or label Telnet as a verified Connection. A future encrypted
legacy-device requirement must arrive as a separately specified protocol with a modern,
non-downgradeable authenticated channel; it will not reactivate plain Telnet.

## Serial Architecture Result

POSIX exposes serial and terminal behavior through device files and `termios`, including baud,
character size, parity, stop bits, flow control, canonical/raw processing, blocking behavior,
and control characters. macOS device discovery belongs behind IOKit/DriverKit behavior rather
than a hard-coded `/dev` scan. Windows requires its own COM-device APIs and ACL/error handling.
Android USB host access is device- and permission-scoped. iOS External Accessory access is tied to
supported manufacturer protocols/MFi relationships and is not equivalent to arbitrary desktop
USB serial.

Reusing `HostProfile` would incorrectly add username, network port, SSH authentication, known-host
trust, jump hosts, forwarding, SFTP, and remote persistence to a local device. Reusing the local
shell PTY would also be wrong because a serial endpoint is already a byte device and has distinct
disconnect, break, modem-line, and hotplug semantics.

## Finite Prerequisites For A Serial Milestone

1. Add a versioned `LocalDeviceConnection` model with stable device identity plus explicit baud,
   data bits, parity, stop bits, flow control, newline, encoding, and bounded read settings.
2. Build per-platform discovery/open adapters with no privileged helper, explicit permission
   errors, symlink/device-type validation, exclusive ownership where supported, hotplug events,
   cancellation, bounded buffers, and deterministic close/reopen behavior.
3. Add a transport-neutral terminal runtime adapter that emits the same bounded input/output/
   resize-disconnected contract without claiming SSH-only features.
4. Design a separate Devices flow with live availability, port identity, settings review, and
   clear local-only labels. Device paths and raw bytes stay out of diagnostics and portable host
   exports.
5. Prove real hardware on every advertised platform: unplug/replug, busy device, denied
   permission, malformed bytes, sustained output/backpressure, flow control, resize irrelevance,
   app shutdown, restore, and safe cancellation. D01 evidence is mandatory per platform.

Until those prerequisites are funded, users can still run trusted local serial tools inside a
normal local terminal, but TermiRust does not claim ownership or lifecycle management for them.

## Primary Sources

- [RFC 854, Telnet Protocol Specification](https://www.rfc-editor.org/rfc/rfc854) - TCP NVT and
  option-negotiation baseline.
- [RFC 2941, Telnet Authentication Option](https://www.rfc-editor.org/rfc/rfc2941) - negotiated
  authentication and active-attack considerations.
- [RFC 2946, Telnet Data Encryption Option](https://www.rfc-editor.org/rfc/rfc2946) - cleartext
  default, downgrade, confidentiality, and integrity limitations.
- [POSIX.1-2024 `termios.h`](https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/termios.h.html)
  and [General Terminal Interface](https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/V1_chap11.html)
  - portable local terminal-device contract.
- [Apple IOKit](https://developer.apple.com/documentation/iokit) - macOS device discovery and
  serial-device integration boundary.
- [Apple External Accessory](https://developer.apple.com/documentation/externalaccessory) - iOS
  accessory and manufacturer-protocol constraints.
- [Android USB host overview](https://developer.android.com/develop/connectivity/usb/host) -
  Android device discovery, permission, interface, and endpoint lifecycle.

## Verification

Run `./scripts/verify-weak-local-transport-decision.sh`. The gate verifies the accepted decision,
primary-source inventory, absence of Telnet/serial product surfaces, access-policy tests, and the
rendered SSH connection-settings flow.
