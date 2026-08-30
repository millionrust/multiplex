# Proxy Routing Boundary

TermiRust supports three explicit outbound routes for saved SSH connections:

- Direct, which remains the backward-compatible default.
- Unauthenticated SOCKS5 CONNECT, with target domain names resolved by the proxy.
- Unauthenticated HTTP/1.0 or HTTP/1.1 CONNECT.

The route applies only to the first network hop. For a jump chain, the outermost jump host owns
that route; later hops and the final target travel through verified SSH `direct-tcpip` channels.
Interactive terminals, SFTP, remote execution, and public-key deployment all use the same route
because they share the SSH/SFTP establishment paths.

The proxy establishes a byte stream only. SSH host-key verification and target authentication
still happen end to end after the proxy handshake. A successful proxy response is never treated
as proof of the target identity.

## Security Boundary

- Proxy configuration is typed host, port, and protocol data. TermiRust does not execute
  `ProxyCommand`, PAC/WPAD, URLs, environment expansion, scripts, or arbitrary executables.
- Proxy authentication is deliberately unsupported. SOCKS username/password and HTTP Basic over
  a plaintext proxy connection would expose a second credential class and risk accidental reuse
  of SSH credentials. Authentication challenges produce a specific error and no credential is
  sent.
- The complete handshake has a 15-second deadline. HTTP response headers are capped at 16 KiB and
  read through the final delimiter without consuming SSH banner bytes. SOCKS framing uses exact,
  bounded reads.
- Errors distinguish unreachable, timeout, malformed response, authentication-required, and
  target-denied states without including SSH passwords or proxy request payloads.
- Local and dynamic forward listeners remain loopback by default and are owned by the SSH session;
  disconnect, cancellation, startup failure, and reconnect replacement release them.

Authenticated proxy support requires a separate design with encrypted transport and an isolated
credential-store namespace. It must never reuse target credentials.
