# Authoritative English catalog. Pseudo-locales are generated from this parsed AST.
common-cancel = Cancel
common-close = Close
common-connect = Connect
common-delete = Delete
common-retry = Retry
common-run = Run
common-save = Save
status-ready = Ready
status-connecting = Connecting to { $host }…
session-count = { $count ->
    [zero] No active sessions
    [one] One active session
    [many] { $count } active sessions
   *[other] { $count } active sessions
    }
transfer-summary = { $count ->
    [zero] No files transferred
    [one] One file transferred ({ $bytes })
    [many] { $count } files transferred ({ $bytes })
   *[other] { $count } files transferred ({ $bytes })
    }
last-updated = Last updated { $when }
shortcut-hint = Shortcut: { $key }
path-unavailable = Cannot open { $path }
field-error = Error: { $reason }
development-localization-title = Pseudo-locales
development-localization-hint = Preview text expansion and bidirectional layout stress without changing saved form values.
development-locale-active = Active development locale: { $locale }
projects-nav-label = Projects
projects-title = Projects & Sessions
projects-subtitle = Organize stable folders for local work.
projects-shortcut-description = Open Projects
projects-loading = Loading projects…
projects-ready-status = Project library ready.
projects-add-action = Add a project
projects-empty-title = No projects yet
projects-empty-description = Add a folder to organize your work and future durable sessions.
projects-folder-safety = Nothing will be changed inside the folder.
projects-local-only = Stored only on this device. No account or network is required.
project-review-title = Review project
project-validating = Checking folder access…
project-review-description = Selected folder: { $path }
project-label-field = Project name
project-add-confirm = Add project
project-status-available = Available
project-status-unavailable = Folder unavailable
project-status-permission-denied = Permission denied
project-remove-action = Remove
project-files-stay = Files stay on disk.
project-added-status = Added { $name }.
project-duplicate-status = { $name } is already in Projects and has been selected.
project-removed-status = Removed { $name } from TermiRust. Files stay on disk.
project-undo-action = Undo
project-restored-status = Restored { $name }.
project-undo-expired = The undo period has ended. Files remain on disk.
project-store-recovered = Last-good project metadata is shown read-only. The damaged file was preserved.
project-store-corrupt = Project metadata is damaged and was not overwritten.
project-store-newer = Project metadata was created by a newer version and is read-only.
project-store-unavailable = Project metadata storage is unavailable. Existing files were not changed.
project-error-empty-path = Choose a project folder.
project-error-permission-denied = TermiRust cannot read this folder. Check its permissions or choose another folder.
project-error-unavailable = This folder is unavailable. Reconnect the drive or choose another folder.
project-error-not-directory = Choose a folder, not a file.
project-error-path-too-long = This folder path is too long to store safely.
project-error-invalid-label = Enter a project name between 1 and 256 characters.
project-error-stale = The project library changed. It has been reloaded; try again.
project-error-generic = The project operation could not be completed safely.
presets-nav-label = Presets
presets-title = Launch presets
presets-subtitle = Save executable and argument lists without shell parsing.
presets-ready-status = Preset library ready.
presets-add-action = New preset
presets-scan-action = Detect installed CLIs
presets-scanning = Checking installed CLIs…
presets-scan-cancelled = CLI detection was cancelled. Saved presets were not changed.
presets-scan-partial = Detection finished with some unavailable or timed-out tools. Other results are still usable.
presets-scan-none = No supported CLIs were found. You can still create a preset manually.
presets-detected-title = Detected suggestions
presets-empty-title = No launch presets yet
presets-empty-description = Detect supported CLIs or add an executable and its arguments manually.
preset-form-title-new = New preset
preset-form-title-edit = Edit preset
preset-label-field = Name
preset-executable-field = Executable
preset-arguments-field = Arguments
preset-argument-add = Add argument
preset-argument-remove = Remove argument
preset-working-directory-field = Working directory
preset-working-project-root = Project root
preset-working-home = Home folder
preset-working-subdirectory = Project subfolder
preset-subdirectory-field = Subfolder
preset-permission-field = Permission policy
preset-permission-ask = Ask as needed
preset-permission-read-only = Read only
preset-permission-workspace-write = Workspace write
preset-enabled-field = Enabled
preset-favorite-field = Favorite
preset-risk-confirm-field = I understand this preset contains a permission-bypass option.
preset-risk-warning = Risky permission option. Review every argument before use.
preset-safe-copy = Arguments are passed literally. TermiRust does not use a shell or install anything.
preset-save-action = Save preset
preset-edit-action = Edit
preset-delete-action = Remove
preset-move-up-action = Move up
preset-move-down-action = Move down
preset-accept-action = Accept suggestion
preset-status-supported = Supported
preset-status-unknown = Version unknown
preset-status-unsupported = Unsupported version
preset-status-missing = Executable missing
preset-status-permission = Permission denied
preset-status-timeout = Probe timed out
preset-status-failed = Version check failed
preset-status-risky = Risky
preset-status-disabled = Disabled
preset-store-recovered = Last-good preset metadata is shown read-only. The damaged file was preserved.
preset-store-corrupt = Preset metadata is damaged and was not overwritten.
preset-store-newer = Preset metadata was created by a newer version and is read-only.
preset-store-unavailable = Preset metadata storage is unavailable. No commands were run.
preset-error-invalid = Check the highlighted preset fields and try again.
preset-error-stale = The preset library changed. It has been reloaded; try again.
preset-error-risk-confirm = Confirm the risky permission option before making this preset a favorite.
preset-saved-status = Saved { $name }.
preset-removed-status = Removed { $name }.
preset-accepted-status = Added detected preset { $name }. Nothing was launched.
preset-detected-version = Version: { $version }
preset-argument-count = { $count ->
    [zero] No arguments
    [one] One argument
    [many] { $count } arguments
   *[other] { $count } arguments
    }
runtime-label-codex = Codex
runtime-label-claude = Claude Code
runtime-label-gemini = Gemini CLI
