# Remove Projects

Status: accepted

Reviewed: 2026-09-30

## Why

A Project was a named record standing between a session and the folder it ran in. Up to 0.0.4 a
person had to add one before starting anything; 0.0.5 made that implicit, deriving a Project from
the folder a session started in. Either way the Project carried nothing the folder did not: its
only fact was a canonical root, and every screen that showed one was showing a folder under a
second name. It also cost a navigation destination, a store file, a derived index, a search
category, a CLI command, an MCP tool and capability, and a field on the controller wire.

So Projects are gone rather than hidden. A session records its folder; groups, which were the only
real organization under a Project, now stand on their own.

## What replaces each part

| Was | Now |
| --- | --- |
| `HostedSession.project_id` | `HostedSession.folder: CanonicalPath` |
| `projects.json` (Projects, groups, worktrees) | `library.json` (groups, worktrees) |
| Group names unique within a Project | Unique across the library |
| `WorkingDirectoryRule::ProjectRoot` | `SessionFolder`, read from `project_root` too |
| Projects page, Cmd+2 | Removed; Cmd+1 to Cmd+6 are Activity, Connections, Sessions, Files, Devices, Settings |
| New Session from a Project row | New Session asks for the folder |
| `project-session-v1.json` derived index | Not built; the palette index remains |
| Search: Project documents, `project:` filter, current-Project boost | Removed |
| CLI `project list`, `--project` | Removed; `session launch --folder <path>` |
| MCP `projects.read`, `termirust_list_projects` | Removed; `projects.read` in a configuration is accepted and grants nothing |
| MCP launch approval by Project ID | By folder (`--folders`), compared on canonical identity |
| `ControllerSessionSummary.project` | Removed; the phones still accept it from an older desktop |

## Stores written before this

`legacy_projects.rs` carries a store forward the first time it is opened for writing, under the
metadata lock:

1. `library.json` is written from the groups and worktrees in `projects.json`, with every Project
   reference taken out.
2. `sessions.json` and its last-good copy give each session its Project's `canonical_root` as its
   folder. A session naming a Project that no longer existed was already unreachable and is
   dropped.
3. `projects.json` and its last-good copy are renamed to `projects.migrated.json` and
   `projects.last-good.migrated.json`, not deleted.

Read-only opens (the TUI, the listener's fleet view) do the same upgrade in memory and write
nothing. A recovery journal naming the `projects` file kind reads as the library. The desktop's own
`state.json` keeps reading `project_directory` on a saved workspace, and an app-attached session
record without a folder takes it from the store.

## Compatibility given up

- CLI JSON v1: `project_id` is gone from every session object and `project list` no longer exists.
  The goldens were regenerated rather than kept.
- A phone running 0.0.5 checks the exact set of keys in each session summary, and a desktop from
  this change no longer sends `project`, so such a phone lists no sessions until it is updated. The
  phones and the desktop ship together in every release.
- The frozen UI audit inventory (`tests/ui/audit-cases.toml`) still names a `projects` screen. It
  is hash-frozen evidence of the surface as it was audited and is left as recorded.
