# Relay Core Loopback Benchmark - 2026-08-29

## Scope

This benchmark starts the actual `termirust-relay-server`, loads restrictive atomic metadata, and opens real loopback TCP/WebSocket Host and Controller connections. Each established pair forwards one 1 KiB opaque binary payload concurrently. Admission is deliberately sequential so the benchmark honors the compiled four-unauthenticated-handshake cap.

- Hardware/OS: Apple silicon `arm64`, macOS Darwin 25.5.0 (`RELEASE_ARM64_T8103`)
- Toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Build profile: workspace dev (`opt-level=1`; dependencies `opt-level=2`)
- Samples: 10 runs at 1, 10, and 100 route pairs
- Transport: real loopback TCP plus RFC 6455 WebSocket; cleartext loopback benchmark path
- TLS: separately covered by the rustls WSS integration test, not included in these timing values
- Raw reproducible output: `target/relay-core/relay-core-report.json`

## Results

Times are p50/p95/p99 in microseconds. Throughput is p50 aggregate ciphertext bytes per second for the concurrent 1 KiB round trip.

| Pairs | Sequential connect µs | Concurrent round trip µs | Throughput B/s | Max RSS | Drops | Real TCP sockets | Max metadata bytes |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1,611 / 1,822 / 1,822 | 1,419 / 1,450 / 1,450 | 724,186 | 4,308,992 | 0 | 2 | 1,618 |
| 10 | 9,796 / 17,015 / 17,015 | 1,402 / 1,727 / 1,727 | 7,420,289 | 7,405,568 | 0 | 20 | 15,524 |
| 100 | 45,567 / 54,085 / 54,085 | 1,073 / 2,078 / 2,078 | 104,810,644 | 39,256,064 | 0 | 200 | 154,516 |

Every run reports zero queue drops, zero persistent ciphertext bytes, and zero per-route log bytes. The metadata bytes contain only route IDs, public admission verifiers, epochs, quotas, and revoked flags.

## Thresholds And Interpretation

- Required p99 sequential admission: below 2,000,000 µs. Worst recorded: 54,085 µs.
- Required p99 concurrent 1 KiB round trip: below 500,000 µs. Worst recorded: 2,078 µs.
- Required queue/persistence/log outcome: zero. Recorded: zero at every point.
- The 100-pair run exercises the compiled simultaneous-forwarding maximum with 200 real TCP sockets.

These results validate the local server core and task/queue model. They do not claim Internet latency, TLS timing, reverse-proxy overhead, public capacity, multi-instance coordination, DDoS resistance, mobile radio behavior, or provider cost. Those remain gated later evidence.

## Reproduce

```sh
./scripts/bench-relay-core.sh --loopback-only --pairs 1,10,100 --runs 10
```

The script rejects noncanonical pair/run settings for completion evidence, validates schema/counts/sockets/zero-content-storage, and enforces the thresholds above.
