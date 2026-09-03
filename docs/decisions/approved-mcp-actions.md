# ADR: Approved MCP actions

- Status: Accepted for local stdio operation
- Decision date: 2026-09-02
- Release constraint: action capabilities are default-off and require a current local approval

## Decision

TermiRust extends its read-only MCP stdio server with a closed set of typed Session and artifact
actions. It does not add HTTP transport, arbitrary shell execution, filesystem paths, raw terminal
output, or automatic mutation retries.

An action is reachable only when its exact startup capability is enabled and the owner-only local
policy allows its action plus exact Project or Session UUID. Policies expire after at most 24 hours,
are replaced atomically, and are revalidated every 25 ms during active work. Removing, expiring, or
replacing a grant cancels active work. The legacy `all` capability alias remains read-only so an MCP
configuration cannot gain mutations after an upgrade.

Mutations carry caller-generated UUID command IDs. A command ID is bound to a canonical operation
fingerprint, including a SHA-256 digest rather than terminal input or artifact content. Same-ID and
same-operation retries replay a bounded persisted result. Same-ID and different-operation calls
fail closed. Host input and cancel preserve the command ID through the existing Host protocol and
writer-lease checks. Launch and semantic resume derive the new Session ID from the command ID, so a
reconnect cannot create a second Session. No mutation is retried automatically.

Resume keeps review and commit separate. Commit requires the exact reviewed source revision. Attach
is metadata-only and read-only. Artifact creation accepts at most 64 KiB UTF-8 and stores it through
the existing inert artifact repository; callers cannot choose a source or destination path.

Before dispatch, an owner-only bounded audit ledger records only timestamp, grant ID, command ID,
action, scope kind, and outcome. Input, artifact content, paths, titles, terminal bytes, and secrets
are excluded. A full or unsafe ledger blocks new actions. Receipts are owner-only, atomically
replaced, and capped at 512 entries.

## Consequences

MCP hosts still provide their own human tool-confirmation UX, but TermiRust does not trust that UI as
its authority boundary. The local approval remains mandatory and can be revoked independently.
Actions are intentionally less convenient than unrestricted shell access because their effects are
bounded, attributable, and recoverable.

Windows owner-only ACL parity remains a release qualification item under N14/N15. The current Unix
implementation enforces `0700` directories, `0600` files, regular-file checks, and symlink rejection.
