Bundled executables live here for development builds.

Place a platform tmux binary at one of these paths:

- `assets/bin/macos/aarch64/tmux`
- `assets/bin/macos/x86_64/tmux`
- `assets/bin/linux/x86_64/tmux`

Release bundles should copy the same executable into the app resources as
`Resources/bin/...` or `Resources/bin/tmux`. The app checks system `tmux`
first, then the bundled binary, then falls back to the configured shell.
