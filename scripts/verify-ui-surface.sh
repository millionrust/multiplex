#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

surface=""
states="all"
locales=""
themes=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --surface) surface="${2:-}"; shift 2 ;;
    --states) states="${2:-}"; shift 2 ;;
    --locales) locales="${2:-}"; shift 2 ;;
    --themes) themes="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ "$states" != "all" || "$locales" != "en-US,en-XA,ar-XB" || "$themes" != "all" ]]; then
  echo "Usage: $0 --surface shell-overlays-palette|projects-groups-sessions|presets-runtimes|worktrees-artifacts|hosts-connections|sftp|vault-keys-snippets|settings|agent-canvas [--states all] --locales en-US,en-XA,ar-XB --themes all" >&2
  exit 2
fi

case "$surface" in
  shell-overlays-palette)
    paths="src/ui/shell.rs,src/ui/app/chrome.rs,src/ui/app/overlay.rs,src/ui/app/palette.rs"
    test_filter="shell_surface"
    description="shell, overlay, and palette"
    ;;
  projects-groups-sessions)
    paths="src/ui/app/projects.rs,src/ui/app/session_sidebar.rs,src/ui/app/session_library.rs"
    test_filter="product_session_surface"
    description="Projects, groups, and Sessions"
    ;;
  presets-runtimes)
    paths="src/ui/app/presets.rs,src/ui/app/runtimes.rs,src/ui/app/session_sidebar.rs"
    test_filter="preset_runtime_surface"
    description="preset and runtime"
    ;;
  worktrees-artifacts)
    paths="src/ui/app/worktree_launch.rs,src/ui/app/artifact_gallery.rs"
    test_filter="worktree_artifact_surface"
    description="worktree and artifact"
    ;;
  hosts-connections)
    paths="src/ui/app/hosts.rs,src/ui/app/connect.rs,src/ui/app/editor.rs"
    test_filter="host_connection_surface"
    description="Hosts and Connections"
    ;;
  sftp)
    paths="src/ui/app/sftp.rs,src/ui/sftp_local.rs"
    test_filter="sftp_surface"
    description="local and remote SFTP"
    ;;
  vault-keys-snippets)
    paths=""
    test_filter="vault_key_snippet_surface"
    description="Vault, key, and Snippet"
    ;;
  settings)
    paths=""
    test_filter="settings_surface"
    description="Settings"
    ;;
  agent-canvas)
    paths="src/ui/app/canvas.rs,src/ui/app/workspace.rs"
    test_filter="agent_canvas_surface"
    description="Agent Canvas"
    ;;
  *)
    echo "unknown UI surface: $surface" >&2
    exit 2
    ;;
esac

cargo run -q -p termirust-ui-contract --bin generate-tokens -- --check
cargo run -q -p termirust-ui-contract --bin generate-messages -- --check
if [[ -n "$paths" ]]; then
  cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- --paths "$paths" --zero-legacy
  cargo run -q -p termirust-ui-contract --bin verify-localization -- --locales en-US,en-XA,ar-XB --paths "$paths" --zero-legacy
else
  cargo run -q -p termirust-ui-contract --bin verify-design-tokens -- --surface "$surface" --zero-legacy
  cargo run -q -p termirust-ui-contract --bin verify-localization -- --locales en-US,en-XA,ar-XB --surface "$surface" --zero-legacy
fi
cargo test -q -p termirust-ui-contract "$test_filter"

echo "verified $description tokens, copy, semantics, states, scale, locale, and theme contracts"
