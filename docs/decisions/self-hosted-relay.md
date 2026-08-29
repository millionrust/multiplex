# Self-Hosted Ciphertext Relay Feasibility

- Status: accepted research decision
- Decision: **Conditional Go**
- Date: 2026-08-29
- Review again by: 2026-11-29 or before activating a product route
- Scope: local synthetic spike only; not a production protocol, deployment, service, or security audit

## Decision

Proceed to Goal 22.2.1 only: an authenticated, bounded, loopback relay server core that preserves Controller-v1 frames unchanged. Prefer a single-tenant self-hosted Rust binary with an OCI image as the package target. Keep LAN/VPN and SSH as independent direct routes and make relay use explicit and optional.

This is a **Conditional Go**, not approval to ship or operate a relay. The conditions are:

1. G22.2.1 must replace the in-process transport with bounded real sockets/tasks and repeat the 1/10/100/1,000-pair tests with cancellation, slow readers, crash/restart, TLS/WebSocket framing, and file-descriptor evidence.
2. The admission and outer transport design requires an independent security review before beta. Ed25519 is used only for relay admission; Controller-v1 remains the sole content trust root.
3. D06 must approve the exact desktop/mobile relay routes and capabilities before any product client uses the core.
4. D05 plus operator, budget, legal/privacy, abuse, incident and on-call approval is mandatory before any public endpoint exists.

No public relay is authorized. No account, provider resource, purchase, production data, public listener, novel content encryption, or public deployment was created for this decision.

## Alternatives

| Route | Advantages | Limits/costs | Decision |
|---|---|---|---|
| No relay: LAN/VPN/SSH | Least metadata, no relay operator, existing direct behavior remains free | Does not traverse every NAT/firewall; user manages reachability | Keep as default independent routes |
| Single-tenant self-hosted Rust/OCI | User controls operator/region/retention; protocol can stay small; no TermiRust license fee | VM/network/domain/TLS/monitoring/backup/security/on-call are external costs | Preferred conditional target |
| Optional managed WebSocket/edge | Can outsource edge connections and scaling | Provider account/metadata/cost/limits; backend still required; AWS 128 KiB message cap fragments 1 MiB envelopes | Research only; never a protocol dependency |

## Protocol

`RelayEnvelopeV1` is a binary wrapper around an already sealed G20.1 Controller frame:

| Field | Bytes | Rule |
|---|---:|---|
| Magic | 4 | `TRR1` |
| Version | 2 | exactly `1` |
| Direction | 1 | Host-to-Controller or Controller-to-Host |
| Reserved | 1 | zero |
| Relay route ID | 32 | opaque CSPRNG 256-bit value; never derived from a Host, Device, Session, Project or user ID |
| Connection-local sequence | 8 | exact monotonic sequence per direction |
| Ciphertext length | 4 | 1 through 1,048,576 bytes |
| Ciphertext | bounded | complete sealed Controller frame; never parsed by relay |

The relay never receives an inner capability, command, terminal title, project/session identifier, plaintext error or offline TTL. TLS is mandatory defense in depth for any later network route, but Controller-v1 Noise/AEAD remains the confidentiality, integrity, endpoint-authentication, capability and revocation boundary even when the relay or TLS terminator is malicious.

Admission uses separate relay-scoped Ed25519 credentials provisioned through an already authenticated direct route: one Host registration credential and one device route credential. The relay retains the public verifying keys, route ID, revocation epoch and quotas. A single-use challenge binds route, role, verifier, epoch, serial, expiry and nonce. Credentials are not G20 long-term keys; private seeds are zeroized and never logged. This spike uses deterministic secrets only in committed synthetic tests and OS CSPRNG generation for non-fixture credentials. Goal 22.2.1 freezes this verifier binding and exact wire bytes in `docs/decisions/relay-protocol-v1.md`.

Default limits are 1,024 routes, 4,096 pending challenges, a 30-tick challenge lifetime, one Host plus one Controller per route, 1 MiB ciphertext, 2 MiB/32 queued frames per direction, and 64 MiB total queued ciphertext. These are spike limits, not production capacity promises.

State is `Unregistered -> Registered -> HostWaiting | ControllerWaiting -> PairedForwarding -> Closed/Revoked`. Forwarding is available only when both endpoints are live. The relay does not acknowledge an inner mutation. Peer loss clears in-flight ciphertext; reconnect relies on Host replay semantics rather than a relay message queue.

Pinned spike dependencies are `termirust-controller-security` from this repository, `ed25519-dalek = 2.2.0`, `zeroize = 1.8.2`, and the exact standalone `tools/relay-spike/Cargo.lock`. No custom content-encryption primitive was added.

## Data flow

1. An already authenticated direct route provisions a random route ID and separate Host/device relay admission credentials.
2. Operator configuration loads only route ID, public admission keys, revocation epoch and quotas.
3. Host and Controller independently open outbound connections and answer role-bound single-use admission challenges.
4. The relay matches exactly one Host and one Controller for the route.
5. Each endpoint sends `RelayEnvelopeV1` containing a sealed Controller-v1 frame. The relay checks only outer version, route, role/direction, sequence, length and queue/rate limits.
6. The peer performs the authoritative inner Controller authentication, sequence, capability and revocation checks.
7. Disconnect, slow-reader close, revoke or crash clears all ciphertext. Restart may reload public admission metadata only.

Trust boundaries are endpoint process, outer TLS/WebSocket and intermediaries, relay process, operator configuration, infrastructure/provider logs, and the peer endpoint. The relay is explicitly outside the inner Controller trust boundary.

## Threat model

The machine-readable STRIDE-style matrix is `tests/fixtures/relay/threat-model.json`. It covers a malicious relay/operator, unauthenticated Internet client, credential theft, tampering, replay/downgrade, route enumeration, abusive admitted endpoint, traffic analysis, diagnostic disclosure, flood, slow reader, duplicate endpoint, capability escalation, crash/restart, clock skew, compromised image/TLS/deployment secret, and compelled operator.

The local tests prove:

- a real sealed Controller-v1 payload is not present in relay-visible bytes;
- unchanged ciphertext opens only at the paired endpoint and a one-bit relay forgery fails AEAD authentication;
- wrong, replayed and expired admission proofs fail closed;
- duplicate endpoint, version mismatch, sequence gap and revoked route fail closed;
- queue caps return explicit backpressure and disconnect/restart retain zero ciphertext;
- debug/stats contain aggregate codes/counts only, not route IDs, credentials or content.

The relay cannot prevent an operator from dropping, delaying or correlating ciphertext. Availability and metadata privacy remain residual risks.

## Residual metadata

“Ciphertext-only” does not hide **IP address, timing, and size**. A relay/provider can observe source/destination IP, route occupancy, connection start/end, direction, ciphertext length, volume, coarse failure class, infrastructure region and provider account/billing metadata. TLS does not make this anonymous. A public operator may also be compelled to retain or disclose those records.

Application logs must omit route IDs, admission credentials, payloads, per-frame sizes, per-route timing, Host/Device/Session/Project/user identifiers and IP addresses by default. Aggregate rejection, cap, reconnect and health counters are sufficient for the prototype. Any operated service needs an approved retention/deletion table and provider-log review under D05.

## Abuse and incident checklist

- [ ] D05 names the legal operator, regions, processors, budget and production owner.
- [ ] D06 names every allowed client route and capability.
- [ ] Independent security review covers admission, TLS/WebSocket, reverse proxy and Controller framing.
- [ ] Per-source and per-route admission/connection/byte/rate quotas are load-tested.
- [ ] DDoS/WAF behavior, abuse contact, suspension and appeal process are documented.
- [ ] TLS, image signing, update, secret rotation and compromised-key procedures are exercised.
- [ ] Monitoring avoids content/route/IP leakage and has approved retention.
- [ ] Crash, backup, restore, rollback, region outage and revocation drills pass.
- [ ] Incident severity, on-call, notification, evidence preservation and postmortem ownership are assigned.
- [ ] Privacy terms cover IP/timing/size metadata, subpoenas, processors and deletion.
- [ ] Capacity and spend alerts have hard ceilings; no silent overage or degraded security mode exists.

Every unchecked item is a release blocker for an operated public endpoint.

## Traffic evidence

The reproducible command runs 10 release-build samples for every combination of 1/10/100/1,000 pairs and idle/interactive/burst workloads:

```sh
./scripts/run-relay-spike.sh --local-only --pairs 1,10,100,1000 --runs 10 --output target/relay-spike
```

Detailed machine, workload, p50/p95/p99 admission/forward latency, throughput, RSS/CPU, queue/drop, logical endpoint, ingress/egress, storage and log-volume evidence is in `docs/benchmarks/relay-spike-2026-08-29.md`. At 1,000 pairs, admission p99 was 163–169 ms across workloads, interactive forwarding p50/p95/p99 was 235/256/256 µs, burst forwarding was 1,559/1,627/1,627 µs, peak RSS was 4,456,448 bytes and drops/persistent bytes/per-route log bytes were zero.

These are in-memory measurements. They demonstrate algorithmic bounds and justify a real loopback core spike; they do not establish WAN, WebSocket, TLS, kernel socket, proxy, multi-instance or provider capacity.

## Cost model

TermiRust software license cost is US$0 under D16. Infrastructure is not free.

The self-host example uses the DigitalOcean Basic 1 GiB plan listed at US$6/month with 1 vCPU, 25 GiB SSD and 1,000 GiB transfer on 2026-08-29. This is a worksheet input, not a sizing recommendation. The measured spike omits kernel/network/proxy overhead, and a reliable deployment may require larger or redundant instances.

The optional managed comparison uses AWS API Gateway’s US East WebSocket example rates on 2026-08-29: US$1.00 per million metered messages and US$0.25 per million connection-minutes. AWS meters in 32 KiB increments and limits a WebSocket message to 128 KiB, so a 1 MiB envelope needs at least eight transport chunks and 32 metered units. With the fixture’s 12 active hours/day and traffic mix, estimated API Gateway-only subtotals are US$0.03192, $0.3192, $3.192 and $31.92/month for 1, 10, 100 and 1,000 pairs. These exclude backend compute/state, data transfer, TLS/domain, monitoring/logs, backups, high availability, DDoS/WAF, support, taxes, security response and on-call.

The exact formulas, assumptions, exclusions, source URLs and access dates are in `tests/fixtures/relay/cost-model.json`. Free tiers are excluded because they are account/time dependent and are not a durable operating model.

## Operations

The later self-host package target is one least-privilege Rust binary plus a pinned signed OCI image conforming to the OCI image specification. Configuration contains public admission material/epochs/quotas and TLS integration references only; private endpoint credentials remain on endpoints. The package needs a `doctor` command for bind/TLS/config/clock/file-descriptor/capacity checks, structured aggregate health, graceful drain, exact config migration, signed update/rollback, backup/restore for admission metadata, and explicit revoke/rotate workflows.

No silent route fallback is allowed. Later UI/CLI must show Disabled, Connecting, Ready, Degraded, Revoked and Failed using text plus shape/icon, disclose observed metadata, and let users select Relay versus LAN/VPN/SSH explicitly. Diagnostics use stable localized codes and never copy server internals, credentials, routes or content.

Invalid/replayed/expired proof, unknown/revoked route, duplicate endpoint, version mismatch, frame/queue/rate cap, slow reader, peer loss, TLS failure, clock/config error and resource exhaustion are explicit failed outcomes. Cancellation closes only owned tasks/sockets. Restart restores no ciphertext and reports no partial forwarding success.

## D05 and D06 boundary

D16 authorizes free, optional, self-hostable engineering. It does not authorize a service.

- D06 is still pending for relay product routes. G22.2.2–G22.2.5 cannot connect desktop, iOS or Android product flows to a relay until the exact route/capability/lost-device policy is approved.
- D05 prohibits accounts, hosted relay and public operation until the client approves operator, regions/processors, privacy/retention, budget/procurement, production credentials, abuse/incident/on-call and legal responsibility.
- Building or testing a loopback core/OCI asset never authorizes deployment. A public endpoint additionally requires explicit deployment authority and all abuse/incident checklist items closed.
- Direct LAN/VPN/SSH behavior stays independent and free; relay is never mandatory and never a silent fallback.

## Sources

Accessed 2026-08-29:

- [RFC 6455, The WebSocket Protocol](https://www.rfc-editor.org/rfc/rfc6455) — framing, version/subprotocol negotiation, origin behavior, masking and connection failure requirements.
- [RFC 8446, TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446) — outer transport security baseline; it does not create an anonymity claim.
- [OWASP WebSocket Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html) — explicit authentication/authorization, size/rate limits, heartbeats, backpressure and sensitive-log avoidance.
- [Open Container Initiative Image Format Specification](https://github.com/opencontainers/image-spec/blob/main/spec.md) — interoperable image/package target.
- [DigitalOcean Droplet pricing](https://www.digitalocean.com/pricing/droplets) — current self-host VM worksheet price and included transfer.
- [Amazon API Gateway pricing](https://aws.amazon.com/api-gateway/pricing/) — current WebSocket message size/metering and US East example rates.
