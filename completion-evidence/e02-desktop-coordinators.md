# E02 Desktop coordinator decomposition completion evidence

- Status: complete
- Branch: `test`
- Final child implementation: `7d4ae53` `refactor(canvas): centralize node placement`
- Final child evidence commit: `64b3504` `docs: record canvas placement extraction`

## Result

The GPUI composition root now owns one instance each of five non-GPUI coordinators:

- `SessionCoordinator`: durable launch/attach worker start plus hosted status/stream projection.
- `ConnectionCoordinator`: local/SSH starts, reconnect decisions, SFTP operations/events.
- `ProjectCoordinator`: reviewed project/preset launch resolution and worktree operations.
- `ControllerCoordinator`: device mutation, listener lifecycle, pairing commands/events.
- `CanvasCoordinator`: selection, graph link creation/mutation, viewport geometry, placement.

Each extraction has a source-boundary test that prevents the migrated policy or worker boundary
from returning to GPUI modules. Views still own GPUI entities, rendering, focus, dialogs,
persistence timing, and user feedback. Coordinators use typed domain/model inputs and closed
results rather than mutating unrelated UI fields.

## Canvas Session identity boundary

Canvas nodes are presentation nodes, so notes and groups have only `CanvasNodeId`. Executable
nodes hold presentation-local pane references; durable panes resolve the authoritative
`HostedSessionId` through `SessionPane.app_attached`. Canvas does not own a Host process or
invent a second Session identity. Persisted canvas nodes use pane indexes so workspace restore
can remap presentation panes while the durable Session ID remains in the restorable connection.

## Child evidence

Canonical child reports are `completion-evidence/e02.1-*.md` through
`completion-evidence/e02.18-*.md`. The final Canvas sequence is:

- `f225c0f`: adjacent selection
- `508dea3`: graph-local link creation
- `833169b`: link mutations
- `506c49b`: viewport geometry
- `7d4ae53`: node placement

## Final verification

- `cargo fmt --all -- --check`: PASS
- `python3 scripts/clippy-changed.py`: PASS for every finite child
- `./scripts/verify-rust.sh workspace`: PASS after every finite child
- Final gate: 534 desktop tests passed, 3 ignored; all workspace tests passed
- Dependency advisories, bans, licenses, and sources: PASS
- `git diff --check`: PASS

The three ignored tests are the repository's intentional live-provider/network gates. Existing
allowed dependency-duplicate and future-incompatibility warnings remain unchanged.
