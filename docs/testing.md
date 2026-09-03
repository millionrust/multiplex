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

The Rust baseline also runs the capability-scoped read-only MCP gate. Run it directly with:

```bash
./scripts/verify-mcp-readonly.sh
```

The separately approved action surface has an additional gate:

```bash
./scripts/verify-mcp-actions.sh
```

The isolated browser policy, artifact, hostile-page, and opt-in live Chrome gates run with:

```bash
./scripts/verify-browser-capability.sh
```

If Chrome/Chromium is unavailable, its live portion prints `SKIPPED(browser)` while the unit,
MCP-contract, strict Clippy, and static containment checks still run.

## Launch Qualification

Run the bounded automated qualification matrix with:

```bash
./scripts/verify-launch-qualification.sh --automated
```

This adds crash/recovery matrices, update-trust attacks, protocol fuzz smoke, Session stress,
desktop terminal and relay performance budgets, a 100 MiB terminal parser run, isolated-browser
containment, release-workflow checks, and Controller fixture integrity. It does not replace the
required 48-hour endurance run, sustained libFuzzer campaign, signed upgrade/rollback drill, or
physical-device and non-macOS platform journeys documented in
[`N15-qualification.md`](engineering-evidence/N15-qualification.md).

The bounded endurance runner refuses durations below 48 hours:

```bash
./scripts/soak-session-relay.sh --hours 48
```

## Cross-Repository Product Baseline

Run the deterministic Rust, Swift, and Kotlin product-model baseline from this
repository with:

```bash
./scripts/verify-product-model.sh --local
```

`--local` is the default when no mode is supplied. It runs the Rust workspace
format, compile, Clippy, test, documentation, and policy checks; verifies the
shared terminal and route fixtures; runs strict iOS source/lifecycle
verification; runs Android unit tests and builds the debug APK; and checks diff
hygiene across the Rust, Swift, and Kotlin codebases in this repository.

Every verifier step prints a text `PASS`, `FAIL`, or `SKIPPED` result. Local mode
does not require Docker, an iOS runtime, an Android emulator, or provider
credentials. Missing runtime-only dependencies are reported as explicit skips,
not successful executions. In particular, a Docker-named Rust test may return
success through its own skip path when Docker is unavailable; that result is not
evidence that live SSH ran.

To require disposable real SSH and Controller smokes, use:

```bash
./scripts/verify-product-model.sh --live
```

Live mode first runs the complete local baseline. It requires a working Docker
daemon for the bundled desktop/Host golden run, then requires eligible iOS and
Android destinations for direct SSH, private-network Controller, Controller-over-SSH, and
self-hosted relay smokes on both mobile platforms. A missing live prerequisite is a failure with
a setup instruction. Live fixtures use loopback or private-LAN resources, and interruption
terminates the verifier's active child and removes verifier-owned temporary files.

Neither mode prints credentials, SSH keys, application state, terminal content,
or environment values. The current evidence records and known limitations are in
[`docs/engineering-evidence/`](engineering-evidence/).

## Bundled Desktop And Host Golden Run

On macOS with Docker Desktop running and `cargo-bundle` installed, run:

```bash
./scripts/verify-desktop-host-golden-run.sh
```

This N02 gate uses only disposable local fixtures. It:

- starts three separate real `termirust-session-host` processes for a local PTY,
  Docker SSH, and a fingerprint-verified deterministic fake agent
- connects through the authenticated and encrypted Controller channel, closes
  and reopens it at an exact replay watermark, and verifies contiguous output
  with no gaps or duplicates
- transfers writer authority, proves new input, revokes the Controller device,
  and verifies that stale authority can no longer write
- builds and launches the real unsigned release app bundle with an isolated
  config, then proves restored local-PTY and SSH startup
- removes only its owned app process group, Host processes, Docker container,
  copied test key, state, and temporary files

The script fails when Docker is unavailable; it never reports a live skip as a
pass. Its evidence record is
[`N02-desktop-host-golden.md`](engineering-evidence/N02-desktop-host-golden.md).

## Android Controller And Host Golden Run

With one authorized Android device connected, run:

```bash
./scripts/test-mobile-android-controller-host.sh --serial <adb-serial>
```

To use a named emulator instead, run:

```bash
./scripts/test-mobile-android-controller-host.sh --avd Pixel_9
```

With neither option, the script uses the sole authorized device or starts the
first installed AVD. It fails when multiple devices are present unless
`--serial` selects one. The N03 gate builds the real Rust Session Host and
Controller listener, installs production and instrumentation APKs, and proves
SAS pairing, Android Keystore persistence, Session listing, capability refresh,
read-only attach, writer acquisition, typed and multiline input without
duplication, resize, exact-cursor reconnect, revocation, and secret deletion.

The fixture configuration is injected only into the test APK and restored
immediately after building. The script removes its ADB reverse, emulator,
processes, and guarded temporary files. See
[`N03-android-controller-golden.md`](engineering-evidence/N03-android-controller-golden.md).

## Native Mobile Relay Transports

The iOS simulator relay transport gate is:

```bash
./scripts/test-mobile-controller-relay-transport.sh
```

The equivalent Android emulator or attached-device gate is:

```bash
./scripts/test-mobile-android-relay-transport.sh --avd Pixel_9
```

Both gates use a disposable TLS relay and Rust echo Host, open two fresh native mobile
transports, and require exact bidirectional echoes across reconnect. The Android gate injects a
disposable test CA only into the instrumentation HTTP client while retaining production SPKI
pinning and native admission/envelope processing. Generated route credentials and certificates
are restored or removed during cleanup.

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
- `sftp::tests::docker_transfer_manager_enforces_conflicts_resume_and_identity_checks`
- `sftp::tests::docker_active_upload_cancellation_does_not_clobber_destination`
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
- `ui::app::tests::e2e_snippets_toolbar_click_new_save_and_delete`
- `ui::app::tests::e2e_snippet_new_button_click_clears_selected_snippet`
- `ui::app::tests::e2e_snippet_row_click_loads_and_pins`
- `ui::app::tests::e2e_vault_cards_and_buttons_click_create_load_and_delete`
- `ui::app::tests::e2e_vault_member_controls_click_select_role_save_clear_and_remove`
- `ui::app::tests::e2e_vault_member_and_sync_round_trip`
- `ui::app::tests::e2e_sync_folder_picker_and_force_pull_conflict_flow`
- `ui::app::tests::e2e_copy_on_select_copies_selection_to_clipboard`
- `ui::app::tests::e2e_workspace_duplicate_and_reorder`
- `ui::app::tests::e2e_workspace_tab_drag_reorders_and_moves_to_tail`
- `ui::app::tests::e2e_workspace_tab_click_activate_rename_and_close`
- `ui::app::tests::e2e_workspace_tab_menu_click_duplicate_and_close`
- `ui::app::tests::e2e_workspace_tab_menu_click_split_horizontal`
- `ui::app::tests::e2e_workspace_tab_menu_click_duplicate_window_and_rename`
- `ui::app::tests::e2e_pane_context_menu_click_duplicate_and_detach`
- `ui::app::tests::e2e_pane_context_menu_click_copy_paste_clear_and_close`
- `ui::app::tests::e2e_pane_context_menu_click_reconnect_recovers_closed_ssh_pane`
- `ui::app::tests::e2e_dragged_workspace_tab_drops_onto_pane_and_merges_split`
- `ui::app::tests::e2e_dragged_workspace_tab_split_rejects_merges_over_max_panes`
- `ui::app::tests::e2e_ssh_logs_record_disconnect_and_logs_section_opens`
- `ui::app::tests::e2e_workspace_and_pane_rename_persist_runtime_state`
- `ui::app::tests::e2e_workspace_disconnect_and_reconnect_all_restores_split_panes`
- `ui::app::tests::e2e_split_divider_drag_updates_and_persists_layout_ratio`
- `ui::app::tests::e2e_rendered_pane_divider_drag_updates_and_persists_layout_ratio`
- `ui::app::tests::e2e_duplicate_active_pane_shortcut_splits_workspace`
- `ui::app::tests::e2e_chrome_local_button_click_opens_local_terminal`
- `ui::app::tests::e2e_chrome_new_button_click_opens_new_host_editor`
- `ui::app::tests::e2e_double_click_empty_chrome_opens_local_terminal`
- `ui::app::tests::e2e_library_nav_cards_click_switch_sections`
- `ui::app::tests::e2e_keyboard_conformance_primary_navigation_shortcuts_switch_sections_and_focus_content`
- `ui::app::tests::e2e_workspace_shortcuts_toggle_views_broadcast_and_cycle_tabs`
- `ui::app::tests::e2e_keyboard_conformance_terminal_shortcuts_preserve_input_and_restore_focus`
- `ui::app::tests::e2e_terminal_paging_shortcuts_adjust_scrollback`
- `ui::app::tests::e2e_terminal_clipboard_shortcuts_copy_and_cancel_multiline_paste`
- `ui::app::tests::e2e_escape_closes_editor_dialog`
- `ui::app::tests::e2e_clear_shortcut_and_escape_from_files_view`
- `ui::app::tests::e2e_keychain_browse_imports_identity_into_private_key_editor`
- `ui::app::tests::e2e_keychain_rendered_tabs_and_buttons_click_import_and_open_editor`
- `ui::app::tests::e2e_keychain_rendered_empty_add_and_use_button_clicks`
- `ui::app::tests::e2e_keychain_rendered_cards_click_use_identity_and_load_password_profile`
- `ui::app::tests::e2e_keychain_identity_tab_loads_password_profile_into_editor`
- `ui::app::tests::e2e_settings_controls_persist_and_reset_preferences`
- `ui::app::tests::e2e_settings_rendered_theme_and_font_clicks`
- `ui::app::tests::e2e_settings_rendered_local_shell_save_and_reset_onboarding_clicks`
- `ui::app::tests::e2e_restore_workspaces_disabled_skips_saved_workspace_launch`
- `ui::app::tests::e2e_session_history_limit_change_trims_existing_logs`
- `ui::app::tests::e2e_host_library_selection_loads_editor_and_tracks_last_connected`
- `ui::app::tests::e2e_host_grid_row_click_selects_edits_and_opens_connect_dialog`
- `ui::app::tests::e2e_host_list_row_click_edits_and_opens_connect_dialog`
- `ui::app::tests::e2e_host_row_inline_controls_toggle_batch_and_list_favorite`
- `ui::app::tests::e2e_known_hosts_remove_button_click_removes_entry`
- `ui::app::tests::e2e_known_hosts_empty_state_open_hosts_button_click_switches_section`
- `ui::app::tests::e2e_logs_empty_state_open_hosts_button_click_switches_section`
- `ui::app::tests::e2e_hosts_toolbar_buttons_click_open_editor_and_terminal`
- `ui::app::tests::e2e_hosts_quick_connect_button_click_opens_workspace`
- `ui::app::tests::e2e_hosts_bulk_toolbar_buttons_click_select_group_star_and_clear`
- `ui::app::tests::e2e_hosts_view_mode_dropdown_click_switches_grid_and_list`
- `ui::app::tests::e2e_hosts_tag_sort_and_avatar_dropdown_clicks`
- `ui::app::tests::e2e_new_host_split_menu_chevron_click_opens_editor_and_imports_config`
- `ui::app::tests::e2e_manual_reconnect_recovers_closed_ssh_pane`
- `ui::app::tests::e2e_onboarding_dismiss_reset_and_local_terminal_marks_complete`
- `ui::app::tests::e2e_onboarding_dismiss_button_click_hides_panel`
- `ui::app::tests::e2e_onboarding_key_button_click_imports_identity_into_editor`
- `ui::app::tests::e2e_onboarding_new_button_click_opens_new_host_editor`
- `ui::app::tests::e2e_onboarding_local_button_click_opens_local_terminal`
- `ui::app::tests::e2e_onboarding_search_button_click_focuses_host_search`
- `ui::app::tests::e2e_saved_host_open_connect_dialog_tab_preserves_profile_context`
- `ui::app::tests::e2e_choose_protocol_dialog_click_continue_and_close`
- `ui::app::tests::e2e_connect_dialog_click_save_and_close`
- `ui::app::tests::e2e_recent_host_chip_reopens_saved_ssh_workspace`
- `ui::app::tests::e2e_chrome_hosts_and_sftp_tabs_click_switch_views`
- `ui::app::tests::e2e_window_resize_persists_saved_window_bounds`
- `ui::app::tests::e2e_connect_dialog_continue_and_save_updates_profile_and_connects`
- `ui::app::tests::e2e_connect_dialog_close_discards_placeholder_workspace`
- `ui::app::tests::e2e_choose_protocol_ssh_path_connects_saved_host`
- `ui::app::tests::e2e_connect_failure_dialog_click_copy_logs_and_restart`
- `ui::app::tests::e2e_connect_failure_dialog_click_edit_host_and_close`

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
- the bounded SFTP manager streams content, reports monotonic progress and SHA-256 evidence, requires explicit conflict decisions, resumes only matching app-owned staging, rejects identity mismatches, and preserves destinations on cancellation
- the GPUI app can open the remote Files view, navigate folders, delete remote files, save/remove user hosts, and quick-connect with password auth
- the GPUI app can save a password-backed host into the system credential store and later reconnect through the stored-password path without retyping the password
- the GPUI app can save a jump-host profile and later resolve that saved host into a real jump chain during connect
- the GPUI app can import a private key through the picker and immediately use it to authenticate against the Docker SSH server
- the GPUI app can also upload and download files through the same dialog-backed SFTP actions used by the desktop UI
- the snippet workflow can save, pin, run, and remove commands against a live terminal session
- the rendered Snippets view can create and delete snippets through its toolbar buttons, and snippet rows can load into the form and pin through their actual click controls
- the rendered Snippets `New` button clears the active snippet draft through its actual click handler
- the rendered Vaults view can create, clear, load, and delete shared vaults through its actual card and button click handlers, and can drive member role selection, save, clear, load, and remove through the rendered member controls
- the vault and sync workflow can save shared vaults, manage members, push an encrypted bundle, pull it into a fresh app state, and re-home items back to Personal when a vault is deleted
- the Settings sync workflow can also pick the sync folder through the dialog-backed action and resolve pull conflicts through the force-pull path
- the connect flow can reject unsupported protocol choices without opening a session
- terminal copy-on-select can push the selected text into the clipboard
- workspace tabs can be duplicated and reordered through the same state paths used by the chrome drag/drop actions
- workspace tabs can also be dragged through the rendered chrome tab-strip hit-testing to reorder before another tab or move to the tail drop zone
- workspace tabs can also be activated by click, enter rename mode by double-click, and close through the rendered close button
- workspace tab context-menu items can duplicate, duplicate into a new window, rename, split horizontally, and close through their rendered click handlers, and pane context-menu items can copy, open the paste-confirmation flow, clear, close, duplicate, detach, and reconnect through their rendered click handlers
- dropping a workspace tab onto a terminal pane can merge the dragged tab into a real split layout, and over-cap merges are rejected without mutating either workspace
- SSH disconnects are recorded in the persisted Logs history and the Logs section can be opened directly from app state
- workspace titles and pane titles persist across runtime state snapshots, including after split-pane creation
- workspace-wide disconnect/reconnect can tear down and restore every pane in a split local-terminal workspace
- split-pane divider drags update the runtime layout ratio and persist that ratio into restored workspace state
- the rendered split-divider handle also responds to real mouse drag events and persists the resulting layout ratio
- pane duplication now has direct coverage through both the documented `Cmd+D` shortcut path and the pane context-menu state action
- the chrome local-terminal button, chrome new-host button, and empty-chrome double-click path all drive their real click handlers
- the sidebar nav cards drive their real click handlers for Hosts, Vaults, Keychain, Snippets, Settings, Known Hosts, and Logs
- library navigation shortcuts can switch sections, jump back to host search, and open the new-host editor
- workspace shortcuts can open remote Files view, toggle back to Terminal, toggle broadcast input, open a new local terminal tab, cycle between tabs, and jump to Logs
- terminal shortcuts can open search, open/close the command palette, close the active workspace, and page through scrollback via the actual key path
- terminal clipboard shortcuts can copy the active selection, open multi-line paste confirmation, and cancel that paste via `Esc`
- `Esc` can close the editor dialog and return from Files view, and `Cmd+Shift+L` clears the active pane through the real shortcut path
- the Keychain view can import a key file into the private-key editor flow and can load saved password-backed host identities from the Identities tab back into the editor
- the rendered Keychain tabs, key-file add button, key cards, password-identity empty-state button, and identity cards all drive their actual click handlers into the editor flows
- the rendered Keychain empty-state `Add Key File` button and per-row `Use` button both drive the same editor-loading auth flows as the rest of the Keychain UI
- the rendered Keychain `Keys` tab button also returns from Identities to the key library through its actual click handler
- the Settings view can persist theme, font, restore/reconnect/keepalive/history toggles, default shell settings, and SSH startup defaults, and can reset the font-family/startup-directory fields through the same app paths used by the UI
- the rendered Settings theme pills and terminal font-size pills drive their actual click handlers
- the rendered Settings local-shell save button and welcome-panel reset button drive their actual settings-update handlers
- disabling workspace restore on launch skips previously saved restorable workspaces
- lowering the session-history retention limit trims existing logs immediately
- the Hosts library can load a saved host into the editor on selection, preserve favorite/color/description/environment metadata, toggle favorites, and surface a real last-connected timestamp after a live SSH session
- the rendered host grid row can select a host by click, reopen it in the editor through the inline edit control, and open the connect dialog tab by double-click
- the rendered host list row can reopen a host in the editor through the inline edit control and open the connect dialog tab by double-click
- the rendered host-row inline controls can batch-select hosts without opening the editor, and list-row star controls can toggle favorites through their actual click handlers
- the rendered Known Hosts view can remove a pinned host and can return to Hosts through the empty-state `Open Hosts` button
- the rendered Logs empty-state `Open Hosts` button returns to the Hosts section through its actual click handler
- the rendered hosts toolbar can open the new-host editor, open a local terminal, and switch between grid and list view modes through the actual dropdown clicks
- the rendered hosts toolbar can also launch quick connect through the `CONNECT` button and drive select-visible, bulk group assignment, bulk star/unstar, and clear-selection through the actual bulk-action buttons
- the rendered hosts toolbar can diagnose selected saved hosts through the actual `Diagnose` button, show strict unknown-trust guidance, leave known-host storage unchanged, and open no terminal workspace
- connection diagnostics enforce four active workers, a 64-item queue and batch cap, per-profile deduplication, queued/active cancellation, explicit retry, and allowlisted actionable outcomes
- the live diagnostic probe reuses direct, HTTP CONNECT, and jump-host routing while requiring existing exact host trust, and distinguishes credential denial, key mismatch, timeout, cancellation, channel/SFTP failure, and recovery
- diagnostic requests strip startup commands, tmux settings, environment entries, port forwarding, PTY/shell/exec, and agent forwarding before probing SSH channel and SFTP availability
- the rendered hosts toolbar can also drive tag filtering, all sort variants, and both avatar invite/email actions through the actual dropdown clicks
- the rendered new-host split-menu chevron can open the editor through `New Group` and import hosts through `Import from ~/.ssh/config`
- an explicitly closed SSH pane can be reconnected manually through the app’s reconnect path after the Docker server comes back on the same port
- the onboarding panel can be dismissed, reset, and automatically completed by opening a local terminal
- the onboarding dismiss, new-host, add-key, local-terminal, and focus-search buttons all drive their real click handlers
- a saved host can open the dedicated connect-dialog tab while preserving the profile context
- the rendered choose-protocol and username connect dialogs can continue, close, and save/connect through their actual button clicks
- the recent-host chip path can reopen a saved SSH workspace from persisted session history
- the top chrome Hosts/SFTP tabs drive their real click handlers in both library and active-workspace states
- window resize events persist saved window bounds and display id into `state.json`
- the saved-host connect dialog can update the username, persist it back into the profile, connect successfully, and also close cleanly without leaving a placeholder workspace behind
- the saved-host choose-protocol dialog can still follow the supported SSH path and replace its placeholder tab with a real connected workspace
- the rendered connect-failure dialog can copy logs, restart into choose-protocol mode, reopen the host editor, and close the failed placeholder tab through its actual button clicks

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

## Legacy Bundled-App SSH Smoke

The older SSH-only packaged-app smoke remains available for focused debugging:

```bash
./scripts/test-real-app-ssh-ax.sh
```

If the release binary is already up to date and you just want to rerun the
desktop smoke faster, you can reuse it with:

```bash
TERMIRUST_SKIP_RELEASE_BUILD=1 ./scripts/test-real-app-ssh-ax.sh
```

That narrower smoke path:

- builds `TermiRust.app` with `cargo bundle --release`
- starts the Docker SSH fixture from `tests/fixtures/ssh-server/`
- seeds a temporary restorable SSH workspace that uses the fixture key
- launches the real bundled desktop app
- verifies the Docker server accepted the SSH login
- verifies the restored startup directory / startup command path runs on the remote side
- verifies `known_hosts.json` was written for the launched-app endpoint

Prefer the N02 golden-run command for qualification. The older command predates
the isolated Controller, Host lifecycle, replay, authority, and comprehensive
cleanup assertions.

## What Still Needs Manual UI Testing

Some desktop behaviors still benefit from manual checks because the app is a native GPUI desktop app and the automated coverage is still thinner on platform-native integrations:

- SFTP upload/download through the native file picker itself, rather than the app logic behind the dialog.
- Platform-native keychain behavior and OS file-dialog behavior.
- Visual polish across macOS, Windows, and Linux.

For manual checks, use a disposable local SSH server or VM first, not production infrastructure.
