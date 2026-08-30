# CLI Session removal

Goal 14.1 adds reviewed Session removal to the one-shot local CLI without creating a second deletion
implementation. `SessionRepository::remove_session`, reached through the shared management facade,
remains the only commit authority.

## Two-step review

`session remove <id>` is preview-only. It verifies the typed Session and optional per-Session revision,
requires exited plus archived state, and returns the selected safe Session summary, aggregate owned-data
manifest, confirmation kind, repository revision, and a canonical `tr-remove-v1` preview token. The token
contains only that repository revision and aggregate byte/file counts. It is bounded and non-secret.

Commit requires the same command with the exact token, `--yes`, and `--confirmation-stdin`. Partial
commit flags are usage errors. Confirmation is one UTF-8 line, at most 256 Unicode scalars, read only
from stdin. It never enters argv, environment, JSON, human output, `Debug`, errors, or the token.

## Commit boundary

The service recomputes the current preview and compares the canonical token before invoking the shared
management command. That facade scans again immediately before the store commit. Revision, lifecycle,
archive state, manifest, symlink, unsafe-root, and quarantine conflicts fail closed. Metadata-only
Sessions require literal `REMOVE`; Sessions with transcripts or artifacts require the exact current
title.

The CLI does not prompt, stop a process, retry an ambiguous mutation, purge quarantine, offer undo, or
claim permanent secure deletion. JSON mode follows the identical two-step contract and never prompts.
