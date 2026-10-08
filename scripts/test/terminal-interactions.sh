#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# Native terminal cases launch the shipped CLI/Host pair; do not test with stale helpers.
cargo build -p multiplex-cli -p multiplex-session-host --bins --locked
cargo nextest run -p multiplex --bin multiplex --locked --no-fail-fast -E '
    test(ui::keys::) | test(terminal::tests::) |
    test(e2e_status_bar_layout_) | test(e2e_vim_) | test(e2e_shell_history_) | test(e2e_terminal_links_) |
    test(e2e_copy_on_select_) | test(e2e_pane_context_menu_click_) |
    test(e2e_canvas_terminal_clipboard_) | test(e2e_canvas_mouse_reporting_) |
    test(e2e_workspace_tab_click_) | test(e2e_tab_rename_saves_) |
    test(e2e_keyboard_conformance_terminal_shortcuts_) |
    test(sessions_lists_terminals_the_app_did_not_open) |
    test(tab_rename_before_cli_record_) | test(tab_kill_immediately_) |
    test(other_terminals::tests::)
'
