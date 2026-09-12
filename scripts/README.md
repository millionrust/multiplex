# Scripts

Automation is grouped by verb. Run everything from the repository root; each
script resolves the root relative to its own location, so the current directory
does not matter for the ones that compute `ROOT_DIR`.

| Folder     | Purpose                                                                     |
| ---------- | --------------------------------------------------------------------------- |
| `verify/`  | Fail-closed gates for contracts, decisions, policies, and releases          |
| `test/`    | Test drivers: `auto.sh`, `repeat.sh`, mobile/live transport and device runs |
| `build/`   | Mobile FFI and binding builds, plus the Python helpers they share           |
| `sync/`    | Copy generated artifacts and fixtures into the mobile projects              |
| `bench/`   | Benchmarks for the desktop terminal, relay core, and accessibility          |
| `run/`     | Spikes, soak, stress, fuzz, and UI-audit runners                            |
| `dev/`     | Developer helpers: changed-line Clippy policy and UI-audit inventory        |

CI entry points are `verify/rust.sh workspace`, `verify/design-tokens.sh`,
`verify/localization.sh`, `verify/release-workflow.sh`, and `test/repeat.sh`.
