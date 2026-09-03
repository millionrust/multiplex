# Self-Hosted Relay

TermiRust's optional relay lets a mobile Controller reach a TermiRust Host when a direct
private-network or Controller-over-SSH route is unavailable. The relay is an outbound,
ciphertext-only transport: Controller-v1 authentication, capabilities, replay protection,
and writer leases remain end to end between the mobile app and Host.

Use a private-network route when possible. Operate the relay yourself only when you need it;
TermiRust does not require a public relay service or an account.

## Requirements

- a machine that can run `termirust-relay` continuously;
- a DNS name and trusted TLS certificate for use outside one machine;
- the TermiRust desktop/Host binary on the computer whose Sessions you want to control; and
- the TermiRust iOS/iPadOS or Android app.

The relay process intentionally binds only to a loopback address. For LAN or internet access,
put a TLS reverse proxy such as Caddy in front of `127.0.0.1:7878`.

## Build The Tools

```bash
cargo build --release --bin termirust --bin termirust-relay
```

Keep the state and package directory private:

```bash
install -d -m 700 "$HOME/.termirust-relay" "$HOME/.termirust-relay/packages"
```

## Configure TLS

A minimal Caddy configuration for `relay.example.com` is:

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:7878
}
```

Start Caddy and verify that `https://relay.example.com` presents the certificate expected by
your phones. Obtain its leaf-certificate SPKI pin:

```bash
openssl s_client -connect relay.example.com:443 -servername relay.example.com </dev/null 2>/dev/null \
  | openssl x509 -pubkey -noout \
  | openssl pkey -pubin -outform der \
  | openssl dgst -sha256 -binary \
  | openssl base64 -A
```

Prefix the printed value with `sha256/`. Re-provision affected routes before intentionally
changing the certificate key. A pin mismatch fails closed.

## Provision One Route

Stop the relay before changing its route state, then run:

```bash
target/release/termirust-relay provision \
  --state "$HOME/.termirust-relay/relay.json" \
  --endpoint "wss://relay.example.com/relay/v1" \
  --spki-pin "sha256/BASE64_SPKI_PIN" \
  --output-dir "$HOME/.termirust-relay/packages"
```

This creates two different secret files:

- `host-route.json` is only for the TermiRust Host computer.
- `controller-route.json` is only for one mobile Controller.

Do not email, log, commit, or place either package in a shared folder. Transfer each directly
to its intended device, import it once, and delete the source file.

## Install And Run The Host Route

On the computer whose Sessions will be controlled:

```bash
target/release/termirust relay-host install \
  --package "$HOME/.termirust-relay/packages/host-route.json"
target/release/termirust relay-host status
target/release/termirust relay-host run
```

The imported admission credential is stored in the operating-system credential store. The
persisted route metadata contains no credential. Keep `relay-host run` active using your normal
per-user service manager and stop it with `Ctrl-C` during foreground testing.

## Import The Mobile Route

1. Open **Devices** in the TermiRust mobile app.
2. Open the paired Host and choose **Self-hosted relay**.
3. Copy the complete contents of `controller-route.json` on the phone.
4. Tap **Paste route package**, review the endpoint and pin, then save.
5. Explicitly select **Self-hosted relay** for that Host.

The app never silently falls back to this route or transfers credentials from another route.
If the Host or relay is temporarily unavailable, the route reports the failure and retries only
through the selected bounded reconnect policy. Input is not replayed after an uncertain write.

## Start The Relay

Behind Caddy:

```bash
target/release/termirust-relay run \
  --state "$HOME/.termirust-relay/relay.json" \
  --bind 127.0.0.1:7878
```

For a loopback-only test, direct TLS is also supported:

```bash
target/release/termirust-relay run \
  --state "$HOME/.termirust-relay/relay.json" \
  --bind 127.0.0.1:7878 \
  --cert server-chain.pem \
  --key server-key.pem
```

The relay accepts only the fixed `/relay/v1` WebSocket route, exact TermiRust Origin and
subprotocol, valid role-specific admission proofs, current route epoch, ordered envelopes, and
bounded frames/queues. It does not persist forwarded frames or have the keys needed to decrypt
Controller traffic.

## Revoke Or Remove A Route

Read the route ID before deleting the controller package:

```bash
ROUTE_ID=$(jq -r .relay_route_id "$HOME/.termirust-relay/packages/controller-route.json")
```

Stop the relay, then revoke access immediately:

```bash
target/release/termirust-relay revoke \
  --state "$HOME/.termirust-relay/relay.json" \
  --route-id "$ROUTE_ID"
```

Remove the Host-side configuration and credential:

```bash
target/release/termirust relay-host remove
```

Finally remove relay metadata:

```bash
target/release/termirust-relay remove \
  --state "$HOME/.termirust-relay/relay.json" \
  --route-id "$ROUTE_ID"
```

Removing a relay route does not delete Projects, Sessions, terminal history, or artifacts.
Remove the mobile route from the Host's Device settings and securely delete any remaining route
packages. Filesystem snapshots or backups may retain deleted package bytes.

## Verification

Repository maintainers can run the disposable loopback gate:

```bash
./scripts/test-mobile-controller-relay-transport.sh
cargo test -p termirust-relay-server -p termirust-relay-client --all-targets
./scripts/verify-relay-v1-vectors.sh
```

The mobile gate creates a temporary CA, TLS relay, route pair, Rust echo Host, and cloned iOS
simulator. It proves native Swift admission, pinning, encrypted-envelope forwarding, and a fresh
connection after disconnect, then removes every owned fixture.
