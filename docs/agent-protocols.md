# Structured Agent Protocol Notes

Structured adapters are version-sensitive and must be checked against official
provider documentation before release.

## Codex

The v1 adapter was implemented and schema-checked against `codex-cli 0.144.4`
on 2026-07-15. It launches `codex app-server`, performs the documented
`initialize` / `initialized` handshake, starts threads and turns, correlates
request IDs, handles streaming notifications and approvals, and interrupts an
active turn on cancellation.

To regenerate reference schemas for the installed Codex version:

```bash
codex app-server generate-json-schema --out /tmp/termirust-codex-schema
```

Generated schemas are review inputs and are not committed as build artifacts.
The deterministic fake app-server tests remain the compatibility gate. A real
provider smoke test is opt-in because it requires local authentication and may
consume paid usage.

## Claude Code

Structured jobs launch the official headless form with `-p`,
`--output-format stream-json`, and `--verbose`. stdout JSON events are normalized;
stderr remains diagnostic. TermiRust does not install hooks or enable bypass
permissions.

The headless CLI does not expose an interactive approval-response channel to
this Rust process. Its documented automation modes pre-allow tools or deny an
unapproved tool. Callback-based `canUseTool` approval is provided by the
official TypeScript and Python Agent SDKs, not by a Rust SDK. TermiRust therefore
reports `approvals: false` for this one-shot adapter instead of pretending that
an emitted tool event can be approved. Revisit this boundary if Anthropic ships
a stable language-neutral protocol or a Rust SDK.

Reference checked 2026-07-15:
https://code.claude.com/docs/en/headless and
https://code.claude.com/docs/en/agent-sdk/permissions

## Gemini CLI

Structured jobs launch the official headless form with `-p` and
`--output-format stream-json`. TermiRust does not install hooks or pass `--yolo`.

Gemini's policy engine documents that `ask_user` is treated as `deny` in
non-interactive mode. The JSONL stream exposes tool use and results, but it does
not provide a supported approval-response request channel for a headless client.
TermiRust therefore reports `approvals: false` for this adapter and relies on
read-only or provider policy configuration.

Reference checked 2026-07-15:
https://geminicli.com/docs/cli/headless/ and
https://geminicli.com/docs/reference/policy-engine/

Claude and Gemini jobs are one prompt per child process. Their adapter supports
queued prompts and cancellation through owned process handles; long-lived TUI
use remains the separate interactive terminal backend.

## Remote transport

The same adapters can run on a saved SSH host. Codex keeps its bidirectional
JSON-RPC stdin/stdout transport over a non-PTY exec channel; Claude and Gemini
stream documented JSONL over stdout. stderr remains diagnostic and remote exit
status or signal becomes a normalized terminal state. A centrally generated,
strictly quoted shell prelude checks the executable, version, and working
directory before `exec`. TermiRust never uploads a helper and never wraps a
structured job in tmux.
