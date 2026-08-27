# Goal 20.4 Completion Evidence

## Delivered

- Disabled-by-default exact private LAN/VPN interface and port policy with no wildcard fallback.
- Discovery remains Off; no mDNS, firewall mutation, relay, account, or public bind exists.
- Owned listener child with bounded stdin/stdout protocol, five-second readiness, EOF ownership,
  two-second shutdown, interface-loss monitoring, and read-only firewall observation.
- Noise-authenticated reconnect and confirmation-gated pairing with short-lived offers, SAS,
  atomic trust persistence, revocation/epoch/capability/generation/deadline/writer checks.
- Typed Host bridge for fleet, attach, output, input, resize, approval response, and detach.
  Unsupported Host approval mutation returns a typed `approval_unavailable` result and never
  guesses completion or replays a write.
- Remote Devices route selection, generated/fixed port, Add Controller, text offer, SAS
  confirm/reject, trusted-device controls, explicit recovery states, pseudo/RTL localization,
  and recording-friendly route masking.

## Verification

Passed on macOS arm64 on 2026-08-27:

```text
./scripts/verify-controller-lan.sh --fixture tests/fixtures/controller-lan
controller LAN verification passed

cargo test -p termirust-controller-listener --all-targets
29 passed, 0 failed across unit and integration targets

cargo test -p termirust ui::app::remote_devices::network_tests
7 passed, 0 failed

cargo clippy -p termirust-controller-listener --all-targets -- -D warnings
passed

cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
localization messages are current
```

The owned-worker test uses a real eligible private interface and temporary durable stores. It
verifies typed readiness, exact-route offer generation, and listener shutdown when the parent
control stream closes. Synthetic encrypted pairing tests cover confirmation and rejection;
hostile suites cover malformed/oversize frames, rate buckets, and queue count/byte boundaries.

`./scripts/verify-rust.sh focused` completed formatting and the all-feature app check, then was
stopped during its separate full Clippy rebuild when available disk reached 14 GiB. Rebuildable
Cargo output was cleaned immediately, restoring 17 GiB. The focused listener Clippy gate and
the complete Goal 20.4 verification script both passed before cleanup.

## Commits

- `f6007ae` Add bounded controller LAN security foundation
- `dba9d67` Implement authenticated controller listener runtime
- `f8edfca` Wire owned controller listener into desktop settings
- `090f284` Enforce bounded controller response queues
- `d4f8ffd` Define bounded controller pairing route protocol
- `999b088` Implement confirmation-gated controller pairing exchange
- `af637f8` Add desktop controller pairing broker
- `80c19b4` Harden controller pairing presentation
- `f3a24ec` Verify owned controller LAN boundary
