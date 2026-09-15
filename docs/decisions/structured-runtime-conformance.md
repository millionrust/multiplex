# Structured runtime adapter conformance

Goal 11.5 applies one TermiRust-owned synthetic lifecycle contract to the existing Codex, Claude
Code, and Gemini CLI structured adapters. This is engineering conformance, not a claim that any
provider version, account, resume route, transcript format, or network service is supported for
release. D02 continues to control those claims.

## Boundary

The test harness launches private temporary executables through the same local process APIs used by
the Canvas. Codex exercises its bidirectional app-server handshake; Claude and Gemini exercise their
headless JSON-line paths. The fixture contains only synthetic records and fixed opaque identifiers.
It never invokes a real provider executable, reads a provider home, uses a user project, or opens a
network connection.

The committed schema-v1 fixture is limited to 64 KiB. Each scenario accepts at most 256 normalized
events before a five-second deadline. The oversize case is just over the adapters' one-MiB line
limit; both it and a malformed record must produce bounded diagnostics before the following valid
records complete normally. Temporary fixture processes record their own PID and every scenario
proves that PID is gone before its private directory is removed.

## Shared semantics

All three routes must report one synthetic Session identity, running state, assistant message, tool
start, successful tool finish, completion, and exactly one succeeded state. Provider failure must
produce the fixed synthetic failure and exactly one failed state without later exit-status
replacement. Cancellation must terminate the owned process and produce exactly one cancelled state,
with no success, completion, or failure.

Cancellation reporting is owned by the worker that observes process settlement. The public
`cancel()` methods request termination but do not emit a speculative terminal event. This removes
the previous headless double emission while preserving cancellation during pre-spawn and active
process races for both local and remote headless workers.

## Capability truth

Interactive PTY, structured events, cancellation, context handoff, and remote execution remain
declared for Codex, Claude Code, and Gemini CLI. Only Codex declares interactive structured approval
handling. Claude and Gemini approval remains absent; the conformance harness does not emulate it.
No descriptor gains resume, transcript, installer, or version support from this test.

Prompts and custom arguments are passed as data. Every scenario includes a shell-expression canary,
asserts that it creates no file, and rejects any normalized/debug event containing the canary.
