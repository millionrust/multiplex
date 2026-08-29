# Goal 11.3 completion evidence

## Result

Goal 11.3 is complete. TermiRust can resume one exact, fixture-backed Codex 0.150.1
conversation through a reviewed replacement Host generation while preserving the source
journal and rejecting every unsupported, ambiguous, stale, live, malformed, escaped, or
resource-exhausting candidate before mutation.

## Delivery

- Repository: `/Users/jacob/Projects/terminal`
- Branch: `test`
- Delivered and pushed commits:
  - `3c14dae` `feat(runtime): validate exact Codex resume contracts`
  - `ef99a2d` `feat(store): persist session resume continuity`
  - `8b5136a` `feat(session): fence resume replacement generations`
  - `9440a42` `feat(session): add reviewed Codex conversation resume`
  - `44783dc` `chore(controller): satisfy strict Clippy checks`
  - `3e0f39e` `test(security): refresh locked dependency checksum`
  - `ca010f2` `test(transcript): scope release contract assertion`
- No delivery commit contains a `Co-authored-by` trailer.
- Final observed free space: 24,794,384 KiB, about 23.65 GiB, above the required
  15 GiB floor.

## Delivered contracts

- The runtime registry exposes Resume only for a managed, exact Codex 0.150.1
  occurrence. Claude, Gemini, generic, observed, ambiguous, live, wrong-version, and
  stale occurrences receive no resume claim.
- `ConversationHandle` is a strict opaque UUID with redacted `Debug`. Handles are
  excluded from ordinary session persistence, logs, search, filenames, and visible IDs.
- The frozen route is literal argv:
  `codex resume --cd <canonical-project> [--sandbox <effective-policy>] <conversation-uuid>`.
  No shell joins or prompt replay are used.
- Candidate discovery and validation are contained to the approved Codex session root
  and bounded by entry, depth, byte, elapsed-time, symlink, exact version, canonical cwd,
  executable-fingerprint, duplicate, and cancellation checks.
- The review sheet presents provider/version, safe project/cwd labels, and effective
  permission policy before launch. All progress, unavailable, failure, and cancellation
  states are localized, including expanded and RTL pseudo-locales.
- Resume creates a new Host instance and occupant generation with expected
  generation/revision fencing. It never replaces the source process in place.
- Durable continuity is committed only after replacement readiness. The atomic,
  private-permission ledger rejects stale revisions, competing successors, forks,
  cycles, unsafe documents, corrupt generations, and lock symlinks; identical commands
  replay idempotently.
- The source journal remains read-only and hash-stable. Inspector links distinguish the
  source and successor generations instead of presenting one continuous journal.
- Cancellation before Ready cleans the replacement Host; cancellation after Ready
  detaches. Failed validation or launch does not stop or mutate the source.

## Acceptance

| Acceptance | Result | Evidence |
|---|---|---|
| `AT-G11.3-01` exact eligibility | PASS | Domain and runtime-manifest tests cover exact version, lifecycle, ownership, generation, handle, and unsupported-provider truth. |
| `AT-G11.3-02` contained validation and safe plan | PASS | Adapter tests cover exact metadata, literal argv, cancellation, malformed, missing, oversize, symlink, duplicate, cwd/version/fingerprint, permission, and resource limits. |
| `AT-G11.3-03` replacement and continuity | PASS | Real separate Host integration proves a new generation, source-journal preservation, Ready-gated continuity, stale fencing, idempotency, chain/fork/cycle behavior, and cancellation semantics. |
| `AT-G11.3-04` failure/privacy/accessibility UX | PASS | Session-library projections, localized review/error states, restart privacy, pseudo/RTL generation, and content-free identifiers pass the canonical gate. |

## Verification

Commands ran against commit `ca010f2b42578b0c71ac0b267b45f2d507da4d79`:

```text
cargo test -p termirust-domain resume
PASS: 4 passed

cargo test -p termirust runtime_resume_contracts
PASS: 1 passed

cargo test -p termirust-session-host --test resume_replacement
PASS: 1 passed

cargo test -p termirust-store continuity
PASS: 6 passed

cargo test -p termirust ui::app::session_library::resume_tests
PASS: 3 passed

cargo test -p termirust agents::resume::tests
PASS: 5 passed

cargo test -p termirust-controller-security --test golden_vectors
PASS: 3 passed

./scripts/verify-localization.sh --locales en-US,en-XA,ar-XB --no-new-baseline
PASS

python3 scripts/clippy-changed.py
PASS: Changed Rust lines are Clippy-clean.

./scripts/verify-rust.sh workspace
PASS: formatting, locked checks, legacy-aware Clippy, changed-line Clippy, all workspace
tests/targets, documentation, localization/tooling, and cargo-deny advisories, bans,
licenses, and sources.
PASS: desktop application 474 passed, 3 authenticated-provider tests intentionally ignored.
PASS: resume replacement, continuity, cancellation, transcript security, Controller,
relay, Docker SSH/tmux, and 1,000-drop Host suites passed.

git diff --check
PASS

git status --short --branch
PASS: clean test branch synchronized with origin/test before this evidence commit.
```

The goal's combined strict command that includes the legacy desktop package still reports
the repository's pre-existing application warning baseline, including Objective-C macro
cfg warnings and legacy dead/style warnings. Those unrelated warnings were not suppressed
or bulk-fixed. Strict `-D warnings` passed for the clean architecture crates touched by
this goal, and every changed desktop Rust line passes `scripts/clippy-changed.py`.

## Failure and privacy evidence

- Missing, malformed, duplicate, oversize, permission-denied, symlinked, wrong-version,
  wrong-cwd, changed-executable, stale-generation, and cancelled candidates fail closed.
- Competing resume attempts cannot create two successors; identical command retries
  return the committed outcome.
- Provider authentication and network failures remain provider output and are never
  retried automatically.
- Raw provider content, credentials, tokens, paths, argv, and opaque handles are absent
  from durable session JSON, continuity metadata, UI labels, and diagnostic `Debug`.
- Authenticated live-provider tests remain intentionally opt-in; the release claim is
  based on the exact installed 0.150.1 command contract plus sanitized fixtures.

## Manual disposition

The safety matrix is deterministic and automated in the canonical gate: eligible resume,
pre/post-Ready cancellation, competing commands, source-hash preservation, policy/root/
fingerprint changes, malformed/permission/resource failures, restart privacy, and
unsupported/observed/live denial. A release-candidate visual pass across supported device
scales and themes remains ordinary release QA, not an unmet resume contract.

