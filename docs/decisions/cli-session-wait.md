# ADR: bounded one-shot CLI Session wait

## Decision

The local CLI exposes `session wait <id>` for one explicit lifecycle or activity state. Exactly one
of `--state` or `--activity` is required. `--timeout-ms` defaults to 30 seconds and accepts 1 through
300,000 milliseconds.

The command reads the authoritative `SessionRepository` at most once every 50 milliseconds after
the initial observation. It uses an injected monotonic waiter, checks cancellation before every
observation, and checks it during production sleep in slices no longer than 10 milliseconds. A
matching first snapshot returns immediately. Timeout, cancellation, Session disappearance, and store
failure retain the CLI's stable error and exit-code classes.

## Contract

Success returns the existing bounded safe Session view plus a closed condition object:

```json
{
  "condition": { "kind": "lifecycle", "state": "exited" },
  "session": { "id": "...", "state": "exited", "activity": "done" }
}
```

The activity form uses `{ "kind": "activity", "state": "done" }`. The command is read-only and
does not expose terminal output, transcript content, filesystem paths, runtime endpoints,
credentials, or provider records. JSON schema version 1 remains additive.

## Consequences

Scripts can replace unbounded shell polling and arbitrary sleeps with one cancellable command.
Only an observed exact state matches; skipped transient states do not produce invented success.
Repository reads retain their existing bounded lock behavior, and the wait never mutates or retries
another command.

Terminal attach/input/resize, transcript export, expression predicates, multi-Session watch, remote
wait, and execute-on-match behavior remain separate reviewed goals.
