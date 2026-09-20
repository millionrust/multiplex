# Self-Hosted iroh Relay

Remote Screens Stage B carries pictures over QUIC. When two devices can reach each other directly
— on the same network, or through the hole punching iroh does for you — nothing here is needed and
no relay sees a byte. This is for the rest of the time: a phone on a cellular network behind a
carrier NAT that refuses to be punched, reaching a computer behind another one.

You do not need this to use Remote Screens. Stage A rides the routes you already have, and Stage B
falls back to them. Run a relay when direct connections are failing often enough to be worth a
machine.

## What a relay can and cannot see

Worth being exact, because "relay" reads as "man in the middle" and here it is not.

An iroh relay forwards encrypted QUIC packets between two endpoints that have already authenticated
to each other. It cannot read a tile, a video frame, or a keystroke: the QUIC connection is
end-to-end encrypted between the two devices, and the screen session inside it is separately
authenticated by the Controller-v1 ticket. A relay operator who keeps every packet has ciphertext.

What it does see is metadata, and that is not nothing:

- the public keys of the two endpoints talking, which are stable identities;
- when they talk, for how long, and roughly how much;
- the IP addresses they connect from, which locates them.

That is the same shape of exposure as the Controller relay in
[self-hosted-relay.md](self-hosted-relay.md), and the same reason to run your own rather than use a
public one. **n0's public relays are the default for people who do not self-host**, and they are
best effort: they rate-limit, and sustained screen streaming through them is not something this
project promises.

## Requirements

- a machine with a public IP that can run `iroh-relay` continuously;
- a DNS name for it;
- a trusted TLS certificate, or the ability to get one from Let's Encrypt;
- UDP 3478 and TCP/UDP 443 reachable from the internet.

It can be the same machine as `termirust relay-host`. They are different protocols on different
ports and neither knows about the other, but they fail for the same reasons and are worth watching
together — one alert, one on-call rota, one set of certificates to renew.

## Install

`iroh-relay` is published by n0 as part of the iroh project.

```bash
cargo install iroh-relay --locked
install -d -m 700 /etc/iroh-relay
```

## Configure

`/etc/iroh-relay/config.toml`:

```toml
# The hostname devices will dial. It must match the certificate.
[http_bind_addr]
addr = "0.0.0.0:80"

[https_bind_addr]
addr = "0.0.0.0:443"

# STUN is how two endpoints learn their own public address so they can try to reach each other
# directly. Leaving it on is what lets most sessions stop using the relay after a second or two.
[stun]
enabled = true
bind_addr = "0.0.0.0:3478"

[tls]
hostname = "screens.example.com"
cert_mode = "LetsEncrypt"
cert_dir = "/etc/iroh-relay/certs"
contact = "you@example.com"

# Who may use it. Without this anyone who learns the hostname can relay through your machine at
# your bandwidth expense; the relay cannot read their traffic, but it is still your bill and your
# IP address on the far end of it.
[access]
default = "deny"
allowlist = [
    # Endpoint public keys, one per device. `termirust screens endpoint-id` prints a device's.
    # "k51qzi5uqu5dk...",
]
```

Run it under whatever supervises long-running services on that machine. A systemd unit:

```ini
[Unit]
Description=iroh relay for Multiplex Remote Screens
After=network-online.target

[Service]
ExecStart=/usr/local/bin/iroh-relay --config-path /etc/iroh-relay/config.toml
Restart=on-failure
User=iroh-relay
# The relay handles untrusted input from the internet. Give it as little as it needs.
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/etc/iroh-relay/certs

[Install]
WantedBy=multi-user.target
```

## Point Multiplex at it

On the computer being watched and on each phone, in **Settings → Remote Devices → Screens**, set
the relay URL to `https://screens.example.com`. Devices that cannot reach it fall back to a direct
connection, and then to Stage A over the Controller channel — a slower picture rather than none.

## Verify

```bash
# The relay answers and presents the certificate you expect.
curl -sSf https://screens.example.com/relay/probe

# STUN is answering, which is what makes most sessions stop needing the relay.
nc -u -w2 screens.example.com 3478 </dev/null && echo "stun reachable"
```

Then watch a screen from a phone on cellular and confirm in **Devices** that the session reports a
direct connection within a few seconds of starting. A session that stays on the relay for its whole
life means hole punching is failing, which is worth knowing: it is the difference between paying
for one handshake and paying for every pixel.

## What is not settled

**This guide is written ahead of the device spike that was meant to justify it.** Step 0.4 of the
Remote Screens plan — a phone to a Mac over cellular through a self-hosted relay — has not been
run, and it is the only thing that can say whether this route is good enough to promise. Treat
everything above as a deployment that has been reasoned about rather than one that has been
operated.

Specifically:

- **The config keys are from iroh 1.2 and have not been run.** iroh's relay configuration has
  changed shape between releases; check it against the version you install.
- **No capacity figure.** How many concurrent screen sessions one relay carries depends on how many
  fail to punch through, which 0.4 is supposed to measure. Until then, size it by watching it.
- **Relay economics.** A relayed screen session is expensive in a way a relayed terminal session is
  not — pictures are orders of magnitude more bytes than text. If a large share of sessions end up
  relayed, this is a bandwidth bill rather than a rounding error, and section 9 of the plan flags it
  as an open question for exactly that reason.
- **The ADR conditions still apply.** [self-hosted-relay.md](decisions/self-hosted-relay.md) makes
  operator, budget, legal, abuse, incident and on-call approval mandatory before any public endpoint
  exists. Running one of these for yourself is your business; offering one to other people is not
  covered by that decision.
