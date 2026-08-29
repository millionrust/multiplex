# Relay Spike Benchmark — 2026-08-29

## Scope

This is a release-build, in-process loopback model of the proposed relay state machine. It measures admission proof verification, envelope validation, bounded queue forwarding, and aggregate memory/resource behavior. It does **not** measure kernel sockets, TLS, WebSocket framing, Internet latency, a reverse proxy, provider runtime, or multi-process coordination. Its throughput is therefore an implementation upper bound, not a deployment claim.

- Hardware/OS: Apple silicon `arm64`, macOS Darwin 25.5.0 (`RELEASE_ARM64_T8103`)
- Toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Profile: release
- Seed: `0x22012026`
- Samples: 10 runs for every pair-count/duty-cycle combination
- Pair counts: 1, 10, 100, 1,000 (two logical endpoints per pair)
- Workloads: idle; interactive (1 KiB each pair plus sampled 64 KiB frames); burst (1 KiB each pair, 64 KiB each pair, sampled 1 MiB frames)
- Raw reproducible output: `target/relay-spike/relay-spike-report.json` from `scripts/run-relay-spike.sh`

## Results

Times are p50/p95/p99 in microseconds. Throughput is p50 ciphertext bytes/second and reflects memory forwarding only.

| Pairs | Duty | Connect µs | Forward µs | Throughput B/s | Max RSS | Drops | Logical endpoints |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | idle | 195 / 275 / 275 | 0 / 0 / 0 | 0 | 1,949,696 | 0 | 2 |
| 1 | interactive | 195 / 289 / 289 | 2 / 13 / 13 | 66,560,000,000 | 2,097,152 | 0 | 2 |
| 1 | burst | 235 / 290 / 290 | 20 / 107 / 107 | 55,756,800,000 | 3,080,192 | 0 | 2 |
| 10 | idle | 2,361 / 2,658 / 2,658 | 0 / 0 / 0 | 0 | 3,112,960 | 0 | 20 |
| 10 | interactive | 1,960 / 2,130 / 2,130 | 2 / 5 / 5 | 37,888,000,000 | 3,112,960 | 0 | 20 |
| 10 | burst | 1,699 / 1,818 / 1,818 | 29 / 31 / 31 | 59,109,517,241 | 3,112,960 | 0 | 20 |
| 100 | idle | 16,090 / 16,352 / 16,352 | 0 / 0 / 0 | 0 | 3,194,880 | 0 | 200 |
| 100 | interactive | 16,015 / 16,113 / 16,113 | 20 / 30 / 30 | 21,504,000,000 | 3,227,648 | 0 | 200 |
| 100 | burst | 16,039 / 16,164 / 16,164 | 150 / 157 / 157 | 51,708,563,758 | 3,244,032 | 0 | 200 |
| 1,000 | idle | 160,771 / 162,523 / 162,523 | 0 / 0 / 0 | 0 | 3,751,936 | 0 | 2,000 |
| 1,000 | interactive | 160,542 / 164,571 / 164,571 | 235 / 256 / 256 | 18,379,487,179 | 4,030,464 | 0 | 2,000 |
| 1,000 | burst | 160,654 / 169,134 / 169,134 | 1,559 / 1,627 / 1,627 | 49,515,269,922 | 4,456,448 | 0 | 2,000 |

## Interpretation

- Admission verification scales approximately linearly and remains below 170 ms p99 for 1,000 pairs in this model.
- Bounded forwarding produces no drops when the peer drains immediately. The hostile slow-reader test separately proves the exact 2 MiB/32-frame cap and explicit backpressure result.
- The relay core reports zero persistent ciphertext and zero per-route log bytes in every scenario.
- Actual sockets, TLS/WebSocket overhead, scheduling, cross-process copies, WAN behavior, and multi-instance admission coordination remain mandatory G22.2.1/G22.2.2 evidence before any product claim.
