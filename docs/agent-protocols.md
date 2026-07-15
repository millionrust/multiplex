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

## Gemini CLI

Structured jobs launch the official headless form with `-p` and
`--output-format stream-json`. TermiRust does not install hooks or pass `--yolo`.

Claude and Gemini jobs are one prompt per child process. Their adapter supports
queued prompts and cancellation through owned process handles; long-lived TUI
use remains the separate interactive terminal backend.
