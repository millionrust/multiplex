# Testing

Use the automated smoke script before committing or before checking a new build manually:

```bash
./scripts/auto-test.sh
```

It runs:

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- `cargo clippy --all-targets --all-features`
- `git diff --check`

## Docker SSH E2E

`cargo test` now includes two Docker-backed end-to-end checks when Docker is available:

- `ssh::tests::docker_ssh_session_connects_and_streams_output`
- `ssh::tests::docker_ssh_session_connects_through_jump_host_chain`
- `ssh::tests::docker_ssh_local_port_forward_proxies_remote_service`
- `ssh::tests::docker_ssh_dynamic_port_forward_proxies_remote_service`
- `ssh::tests::docker_ssh_remote_port_forward_proxies_local_service`
- `ui::app::tests::e2e_ssh_workspace_connects_renders_output_and_closes`
- `ui::app::tests::e2e_ssh_workspace_connects_through_jump_host_and_renders_output`
- `ui::app::tests::e2e_ssh_split_and_broadcast_reaches_all_panes`
- `ui::app::tests::e2e_ssh_auto_reconnect_recovers_after_server_restart`
- `ui::app::tests::e2e_restored_ssh_workspace_reconnects_and_runs_startup_on_launch`
- `ui::app::tests::e2e_restored_ssh_workspace_opens_files_view_on_launch`
- `ui::app::tests::e2e_restored_password_workspace_reconnects_on_launch`
- `ui::app::tests::e2e_local_shell_paste_confirmation_and_search`
- `sftp::tests::docker_sftp_round_trips_directory_upload_download_and_delete`
- `ui::app::tests::e2e_sftp_files_view_navigates_and_deletes_remote_files`
- `ui::app::tests::e2e_sftp_upload_and_download_via_dialog_actions`
- `ui::app::tests::e2e_saved_local_forward_rule_launches_on_connect`
- `ui::app::tests::e2e_saved_dynamic_forward_rule_launches_on_connect`
- `ui::app::tests::e2e_saved_remote_forward_rule_launches_on_connect`
- `ui::app::tests::e2e_saved_host_connect_runs_startup_actions`
- `ui::app::tests::e2e_saved_host_connect_opens_files_view`
- `ui::app::tests::e2e_group_defaults_save_apply_and_remove_round_trip`
- `ui::app::tests::e2e_batch_selection_and_bulk_host_actions`
- `ui::app::tests::e2e_command_palette_replays_recent_command`
- `ui::app::tests::e2e_quick_connect_password_flow_opens_workspace`
- `ui::app::tests::e2e_host_editor_saves_and_removes_user_profile`
- `ui::app::tests::e2e_saved_password_profile_connects_via_keychain`
- `ui::app::tests::e2e_saved_jump_host_profile_resolves_and_connects`
- `ui::app::tests::e2e_imported_private_key_connects_to_docker_ssh`
- `ui::app::tests::e2e_snippet_save_run_pin_and_remove`
- `ui::app::tests::e2e_vault_member_and_sync_round_trip`
- `ui::app::tests::e2e_sync_folder_picker_and_force_pull_conflict_flow`
- `ui::app::tests::e2e_choose_protocol_rejects_unsupported_protocols`
- `ui::app::tests::e2e_copy_on_select_copies_selection_to_clipboard`
- `ui::app::tests::e2e_workspace_duplicate_and_reorder`
- `ui::app::tests::e2e_dragged_workspace_tab_drops_onto_pane_and_merges_split`
- `ui::app::tests::e2e_dragged_workspace_tab_split_rejects_merges_over_max_panes`
- `ui::app::tests::e2e_ssh_logs_record_disconnect_and_logs_section_opens`
- `ui::app::tests::e2e_workspace_and_pane_rename_persist_runtime_state`
- `ui::app::tests::e2e_workspace_disconnect_and_reconnect_all_restores_split_panes`
- `ui::app::tests::e2e_split_divider_drag_updates_and_persists_layout_ratio`
- `ui::app::tests::e2e_duplicate_active_pane_shortcut_splits_workspace`
- `ui::app::tests::e2e_chrome_local_button_click_opens_local_terminal`
- `ui::app::tests::e2e_chrome_new_button_click_opens_new_host_editor`
- `ui::app::tests::e2e_double_click_empty_chrome_opens_local_terminal`
- `ui::app::tests::e2e_library_navigation_shortcuts_switch_sections_and_open_editor`
- `ui::app::tests::e2e_workspace_shortcuts_toggle_views_broadcast_and_cycle_tabs`
- `ui::app::tests::e2e_terminal_shortcuts_open_search_palette_and_close_workspace`
- `ui::app::tests::e2e_terminal_paging_shortcuts_adjust_scrollback`
- `ui::app::tests::e2e_terminal_clipboard_shortcuts_copy_and_cancel_multiline_paste`
- `ui::app::tests::e2e_escape_closes_editor_dialog`
- `ui::app::tests::e2e_clear_shortcut_and_escape_from_files_view`
- `ui::app::tests::e2e_keychain_browse_imports_identity_into_private_key_editor`
- `ui::app::tests::e2e_keychain_identity_tab_loads_password_profile_into_editor`
- `ui::app::tests::e2e_settings_controls_persist_and_reset_preferences`
- `ui::app::tests::e2e_restore_workspaces_disabled_skips_saved_workspace_launch`
- `ui::app::tests::e2e_session_history_limit_change_trims_existing_logs`
- `ui::app::tests::e2e_host_library_selection_loads_editor_and_tracks_last_connected`
- `ui::app::tests::e2e_manual_reconnect_recovers_closed_ssh_pane`
- `ui::app::tests::e2e_onboarding_dismiss_reset_and_local_terminal_marks_complete`
- `ui::app::tests::e2e_onboarding_dismiss_button_click_hides_panel`
- `ui::app::tests::e2e_onboarding_key_button_click_imports_identity_into_editor`
- `ui::app::tests::e2e_onboarding_new_button_click_opens_new_host_editor`
- `ui::app::tests::e2e_onboarding_local_button_click_opens_local_terminal`
- `ui::app::tests::e2e_onboarding_search_button_click_focuses_host_search`
- `ui::app::tests::e2e_saved_host_open_connect_dialog_tab_preserves_profile_context`
- `ui::app::tests::e2e_recent_host_chip_reopens_saved_ssh_workspace`
- `ui::app::tests::e2e_window_resize_persists_saved_window_bounds`
- `ui::app::tests::e2e_connect_dialog_continue_and_save_updates_profile_and_connects`
- `ui::app::tests::e2e_connect_dialog_close_discards_placeholder_workspace`
- `ui::app::tests::e2e_choose_protocol_ssh_path_connects_saved_host`

Those tests build `tests/fixtures/ssh-server/`, start a disposable OpenSSH container, connect through the real `russh` session path, and verify both:

- the raw SSH runtime can authenticate, stream output, and disconnect cleanly
- the raw SSH runtime can also authenticate through a real jump-host tunnel chain
- the raw SSH runtime can open local forwards, SOCKS5 dynamic forwards, and remote reverse forwards against the Docker SSH fixture
- the GPUI app can save local, SOCKS5 dynamic, and remote reverse forwarding rules in the host editor and launch them automatically on connect
- the GPUI app can connect a saved host and still honor its startup directory / startup command / environment plus Files-view launch mode
- the Settings defaults for SSH startup directory and local shell program/cwd are applied by normal connect and open-local-terminal flows
- the GPUI app can save group defaults, reapply them into a new draft, and remove them again
- the GPUI app can batch-select hosts and apply bulk group/favorite operations across filtered library selections
- the command palette can surface a real recent command and replay it into the active session
- the GPUI app can open a workspace, render terminal output, accept typed terminal input, split panes, broadcast commands, auto-reconnect after a non-user disconnect, and connect through a real jump host
- restored SSH workspaces can reconnect on launch, run startup actions, and open directly into the Files view
- restored SSH workspaces can also reconnect through the saved password-credential path on launch
- the local terminal path can confirm/cancel multi-line paste and drive workspace search against real terminal output
- the SFTP runtime can list directories, upload files, download files, and delete remote files against the same Docker SSH target
- the GPUI app can open the remote Files view, navigate folders, delete remote files, save/remove user hosts, and quick-connect with password auth
- the GPUI app can save a password-backed host into the system credential store and later reconnect through the stored-password path without retyping the password
- the GPUI app can save a jump-host profile and later resolve that saved host into a real jump chain during connect
- the GPUI app can import a private key through the picker and immediately use it to authenticate against the Docker SSH server
- the GPUI app can also upload and download files through the same dialog-backed SFTP actions used by the desktop UI
- the snippet workflow can save, pin, run, and remove commands against a live terminal session
- the vault and sync workflow can save shared vaults, manage members, push an encrypted bundle, pull it into a fresh app state, and re-home items back to Personal when a vault is deleted
- the Settings sync workflow can also pick the sync folder through the dialog-backed action and resolve pull conflicts through the force-pull path
- the connect flow can reject unsupported protocol choices without opening a session
- terminal copy-on-select can push the selected text into the clipboard
- workspace tabs can be duplicated and reordered through the same state paths used by the chrome drag/drop actions
- dropping a workspace tab onto a terminal pane can merge the dragged tab into a real split layout, and over-cap merges are rejected without mutating either workspace
- SSH disconnects are recorded in the persisted Logs history and the Logs section can be opened directly from app state
- workspace titles and pane titles persist across runtime state snapshots, including after split-pane creation
- workspace-wide disconnect/reconnect can tear down and restore every pane in a split local-terminal workspace
- split-pane divider drags update the runtime layout ratio and persist that ratio into restored workspace state
- pane duplication now has direct coverage through both the documented `Cmd+D` shortcut path and the pane context-menu state action
- the chrome local-terminal button, chrome new-host button, and empty-chrome double-click path all drive their real click handlers
- library navigation shortcuts can switch sections, jump back to host search, and open the new-host editor
- workspace shortcuts can open remote Files view, toggle back to Terminal, toggle broadcast input, open a new local terminal tab, cycle between tabs, and jump to Logs
- terminal shortcuts can open search, open/close the command palette, close the active workspace, and page through scrollback via the actual key path
- terminal clipboard shortcuts can copy the active selection, open multi-line paste confirmation, and cancel that paste via `Esc`
- `Esc` can close the editor dialog and return from Files view, and `Cmd+Shift+L` clears the active pane through the real shortcut path
- the Keychain view can import a key file into the private-key editor flow and can load saved password-backed host identities from the Identities tab back into the editor
- the Settings view can persist theme, font, restore/reconnect/keepalive/history toggles, default shell settings, and SSH startup defaults, and can reset the font-family/startup-directory fields through the same app paths used by the UI
- disabling workspace restore on launch skips previously saved restorable workspaces
- lowering the session-history retention limit trims existing logs immediately
- the Hosts library can load a saved host into the editor on selection, preserve favorite/color/description/environment metadata, toggle favorites, and surface a real last-connected timestamp after a live SSH session
- an explicitly closed SSH pane can be reconnected manually through the app’s reconnect path after the Docker server comes back on the same port
- the onboarding panel can be dismissed, reset, and automatically completed by opening a local terminal
- the onboarding dismiss, new-host, add-key, local-terminal, and focus-search buttons all drive their real click handlers
- a saved host can open the dedicated connect-dialog tab while preserving the profile context
- the recent-host chip path can reopen a saved SSH workspace from persisted session history
- window resize events persist saved window bounds and display id into `state.json`
- the saved-host connect dialog can update the username, persist it back into the profile, connect successfully, and also close cleanly without leaving a placeholder workspace behind
- the saved-host choose-protocol dialog can still follow the supported SSH path and replace its placeholder tab with a real connected workspace

If Docker is unavailable, the rest of the suite still runs and the SSH E2E tests self-skip.

## Optional Live SSH Smoke

The default suite does not touch any real server. To verify that your local machine or a test VM accepts SSH, set these environment variables:

```bash
TERMIRUST_TEST_SSH_HOST=localhost \
TERMIRUST_TEST_SSH_USER="$(whoami)" \
TERMIRUST_TEST_SSH_PORT=22 \
TERMIRUST_TEST_SSH_KEY="$HOME/.ssh/termirust_test_key" \
./scripts/auto-test.sh
```

If you use your normal SSH agent or default key, omit `TERMIRUST_TEST_SSH_KEY`.

This smoke check proves the target is reachable and authenticated before you test the app UI against the same host.

## Real Bundled-App SSH Smoke

On macOS, you can also verify the packaged `TermiRust.app` against a disposable
Docker SSH server:

```bash
./scripts/test-real-app-ssh-ax.sh
```

If the release binary is already up to date and you just want to rerun the
desktop smoke faster, you can reuse it with:

```bash
TERMIRUST_SKIP_RELEASE_BUILD=1 ./scripts/test-real-app-ssh-ax.sh
```

That smoke path:

- builds `TermiRust.app` with `cargo bundle --release`
- starts the Docker SSH fixture from `tests/fixtures/ssh-server/`
- seeds a temporary restorable SSH workspace that uses the fixture key
- launches the real bundled desktop app
- verifies the Docker server accepted the SSH login
- verifies the restored startup directory / startup command path runs on the remote side
- verifies `known_hosts.json` was written for the launched-app endpoint

It is intentionally separate from `auto-test.sh` because it is macOS-specific,
launches a real desktop bundle, and mutates local app state temporarily while it
runs.

## What Still Needs Manual UI Testing

Some desktop behaviors still benefit from manual checks because the app is a native GPUI desktop app and the automated coverage is still thinner on visually-driven native interactions:

- Window resizing, tab dragging through rendered hit-testing, and split-pane divider interaction.
- SFTP upload/download through the native file picker itself, rather than the app logic behind the dialog.
- Platform-native keychain and OS file-dialog behavior.
- Visual polish across macOS, Windows, and Linux.

For manual checks, use a disposable local SSH server or VM first, not production infrastructure.
