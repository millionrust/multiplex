# Capability-scoped read-only MCP

Date: 2026-09-02

## Decision

TermiRust exposes N11 inspection through a separate local `termirust-mcp` stdio process. The
server implements the MCP `2025-11-25` initialization and tools contracts directly over bounded
newline-delimited JSON-RPC. It advertises only tools allowed by an exact environment capability
set. It does not provide HTTP, Resources, Prompts, Sampling, experimental Tasks, or mutation.

Project, connection-preset, Session, and runtime projections reuse `termirust-cli`'s local command
service, which already reads the versioned TermiRust repositories and Host-projected Session
state. Artifact inspection reuses `ArtifactRepository` but returns metadata only. Semantic
transcript inspection accepts only the fixed Session-owned `records.jsonl` contract, defaults
permanently to User and Assistant categories for N11, normalizes content through the shared
redactor, and never reads the terminal journal.

## Security Boundary

Metadata capabilities are enabled by default when the user configures this local process in an
MCP host. Artifact names and transcript bodies are disabled unless explicitly added. Unauthorized
tools are indistinguishable from unknown tools. Tool arguments deny unknown fields, typed IDs are
validated before path derivation, Session paths are contained, symlinks fail closed, all lists are
paginated, and request/result/concurrency/rate/cursor stores are bounded.

Cancellation is available while a tool worker is running; a cancelled call drops its response.
Server logs go only to stderr, and their stable messages contain no paths, titles, transcript
content, credentials, or artifact names. Stdout contains only MCP messages.

## Deferred

N12 owns any launch, wait, cancel-Session, attach, input, resize, resume, artifact import, or other
mutation. An HTTP transport would require a separate authenticated design with Origin validation
and is not implied by this decision. Raw terminal-content export is not an N11 capability.
