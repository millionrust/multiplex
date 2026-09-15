use gpui::{App, Context, Window};
use termirust_ui_contract::{
    MessageId, SemanticActionValue, VaultKeySnippetAccessibilityCommand, VaultKeySnippetAction,
    VaultKeySnippetControl, VaultKeySnippetControlRole, VaultKeySnippetRow, VaultKeySnippetRowId,
    VaultKeySnippetScreen, VaultKeySnippetSemanticSnapshot, VaultKeySnippetSurfaceState,
    stable_vault_key_snippet_value,
};

use super::{KeyLifecycleDialog, KeychainTab, NavSection, TermiRustApp};
use crate::models::{DEFAULT_VAULT_ID, SavedIdentity, SavedSnippet};
use crate::sftp::AuthorizedKeyAction;

impl TermiRustApp {
    pub(super) fn vault_key_snippet_semantic_snapshot(
        &self,
        _cx: &App,
    ) -> Option<VaultKeySnippetSemanticSnapshot> {
        if let Some(dialog) = self.key_lifecycle_dialog.as_ref() {
            return Some(self.key_lifecycle_semantic_snapshot(dialog));
        }
        if self.pending_snippet_prompts.is_some() {
            return Some(snapshot(
                VaultKeySnippetScreen::SnippetPrompts,
                VaultKeySnippetSurfaceState::Reviewing,
                Vec::new(),
                vec![
                    control(
                        VaultKeySnippetAction::ConfirmSnippetPrompts,
                        None,
                        MessageId::SnippetConfirmInsertAction,
                        false,
                    ),
                    control(
                        VaultKeySnippetAction::CancelSnippetPrompts,
                        None,
                        MessageId::SnippetCancelInsertAction,
                        false,
                    ),
                ],
                self.activity_center.policy().recording_friendly,
            ));
        }
        if let Some(pending) = self.pending_snippet_insert.as_ref() {
            let snippet = self
                .saved
                .snippets
                .iter()
                .find(|snippet| snippet.id == pending.snippet_id);
            let exact = snippet.is_some_and(|snippet| snippet.command == pending.source_command)
                && self.active_pane().map(|pane| pane.id) == Some(pending.pane_id);
            let rows = snippet
                .map(|snippet| {
                    vec![row(
                        snippet_row_id(snippet),
                        None,
                        snippet.display_name(),
                        MessageId::SnippetRowSaved,
                        self.pane(pending.pane_id).map(|pane| pane.title.clone()),
                        true,
                        !exact,
                        false,
                        1,
                        1,
                    )]
                })
                .unwrap_or_default();
            let row = rows.first().map(|row| row.id).unwrap_or_else(|| {
                VaultKeySnippetRowId::snippet(
                    0,
                    stable_vault_key_snippet_value(&pending.snippet_id),
                )
            });
            return Some(snapshot(
                VaultKeySnippetScreen::SnippetInsertReview,
                if exact {
                    VaultKeySnippetSurfaceState::Reviewing
                } else {
                    VaultKeySnippetSurfaceState::Stale
                },
                rows,
                vec![
                    control(
                        VaultKeySnippetAction::ConfirmSnippetInsert {
                            snippet: row,
                            pane_id: pending.pane_id,
                        },
                        None,
                        MessageId::SnippetConfirmInsertAction,
                        !exact,
                    ),
                    control(
                        VaultKeySnippetAction::CancelSnippetInsert,
                        None,
                        MessageId::SnippetCancelInsertAction,
                        false,
                    ),
                ],
                self.activity_center.policy().recording_friendly,
            ));
        }
        match self.nav_section {
            NavSection::Vaults => Some(self.vaults_semantic_snapshot()),
            NavSection::Keychain => Some(self.keychain_semantic_snapshot()),
            NavSection::Snippets => Some(self.snippets_semantic_snapshot()),
            _ => None,
        }
    }

    fn vaults_semantic_snapshot(&self) -> VaultKeySnippetSemanticSnapshot {
        let mut rows = Vec::new();
        let count = self.saved.vaults.len().max(1);
        for (index, vault) in self.saved.vaults.iter().enumerate() {
            let vault_row = vault_row_id(&vault.id);
            rows.push(row(
                vault_row,
                None,
                vault.display_name(),
                MessageId::VaultKeySnippetStateReady,
                (!vault.description.trim().is_empty()).then(|| vault.description.clone()),
                self.selected_vault_id.as_deref() == Some(vault.id.as_str()),
                false,
                true,
                index + 1,
                count,
            ));
            let member_count = vault.members.len().max(1);
            for (member_index, member) in vault.members.iter().enumerate() {
                rows.push(row(
                    member_row_id(&vault.id, &member.id),
                    Some(vault_row),
                    member.display_name(),
                    MessageId::VaultKeySnippetStateReady,
                    (!member.email.trim().is_empty()).then(|| member.email.clone()),
                    self.selected_vault_member_id.as_deref() == Some(member.id.as_str()),
                    vault.is_personal(),
                    !vault.is_personal(),
                    member_index + 1,
                    member_count,
                ));
            }
        }
        let mut controls = vec![
            control(
                VaultKeySnippetAction::NewVault,
                None,
                MessageId::VaultNewAction,
                false,
            ),
            control(
                VaultKeySnippetAction::SaveVault,
                None,
                MessageId::CommonSave,
                false,
            ),
        ];
        controls.extend(self.saved.vaults.iter().map(|vault| {
            let id = vault_row_id(&vault.id);
            control(
                VaultKeySnippetAction::SelectVault(id),
                Some(id),
                MessageId::VaultSelectAction,
                false,
            )
        }));
        if let Some(vault) = self
            .selected_vault_id
            .as_deref()
            .and_then(|id| self.saved.vaults.iter().find(|vault| vault.id == id))
        {
            let id = vault_row_id(&vault.id);
            controls.push(control(
                VaultKeySnippetAction::DeleteVault(id),
                Some(id),
                MessageId::VaultDeleteAction,
                vault.is_personal(),
            ));
            controls.push(control(
                VaultKeySnippetAction::SaveMember(id),
                Some(id),
                MessageId::VaultMemberSaveAction,
                vault.is_personal(),
            ));
            controls.extend(vault.members.iter().map(|member| {
                let member = member_row_id(&vault.id, &member.id);
                control(
                    VaultKeySnippetAction::DeleteMember(member),
                    Some(member),
                    MessageId::VaultMemberDeleteAction,
                    vault.is_personal(),
                )
            }));
        }
        snapshot(
            VaultKeySnippetScreen::Vaults,
            if self.saved.vaults.is_empty() {
                VaultKeySnippetSurfaceState::Empty
            } else {
                VaultKeySnippetSurfaceState::Ready
            },
            rows,
            controls,
            self.activity_center.policy().recording_friendly,
        )
    }

    fn keychain_semantic_snapshot(&self) -> VaultKeySnippetSemanticSnapshot {
        let count = self.saved.identities.len().max(1);
        let rows = self
            .saved
            .identities
            .iter()
            .enumerate()
            .map(|(index, identity)| {
                row(
                    identity_row_id(identity, self.keychain_tab),
                    None,
                    identity.label.clone(),
                    MessageId::VaultKeySnippetStateReady,
                    Some(identity.key_path.clone()),
                    false,
                    false,
                    true,
                    index + 1,
                    count,
                )
            })
            .collect::<Vec<_>>();
        let mut controls = vec![
            control(
                VaultKeySnippetAction::ShowKeys,
                None,
                MessageId::KeychainShowKeysAction,
                false,
            ),
            control(
                VaultKeySnippetAction::ShowIdentities,
                None,
                MessageId::KeychainShowIdentitiesAction,
                false,
            ),
            control(
                VaultKeySnippetAction::GenerateKey,
                None,
                MessageId::KeyGenerateAction,
                false,
            ),
            control(
                VaultKeySnippetAction::AddKeyFile,
                None,
                MessageId::KeyAddFileAction,
                false,
            ),
        ];
        for (identity, row) in self
            .saved
            .identities
            .iter()
            .zip(rows.iter().map(|row| row.id))
        {
            controls.push(control(
                VaultKeySnippetAction::UseKey(row),
                Some(row),
                MessageId::KeyUseAction,
                false,
            ));
            let disabled = !std::path::Path::new(&format!("{}.pub", identity.key_path)).exists();
            controls.push(control(
                VaultKeySnippetAction::DeployKey(row),
                Some(row),
                MessageId::KeyDeployAction,
                disabled,
            ));
            controls.push(control(
                VaultKeySnippetAction::RemoveRemoteKey(row),
                Some(row),
                MessageId::KeyRemoveRemoteAction,
                disabled,
            ));
        }
        snapshot(
            match self.keychain_tab {
                KeychainTab::Keys => VaultKeySnippetScreen::KeychainKeys,
                KeychainTab::Identities => VaultKeySnippetScreen::KeychainIdentities,
            },
            if rows.is_empty() {
                VaultKeySnippetSurfaceState::Empty
            } else {
                VaultKeySnippetSurfaceState::Ready
            },
            rows,
            controls,
            self.activity_center.policy().recording_friendly,
        )
    }

    fn snippets_semantic_snapshot(&self) -> VaultKeySnippetSemanticSnapshot {
        let count = self.saved.snippets.len().max(1);
        let rows = self
            .saved
            .snippets
            .iter()
            .enumerate()
            .map(|(index, snippet)| {
                row(
                    snippet_row_id(snippet),
                    None,
                    snippet.display_name(),
                    MessageId::SnippetRowSaved,
                    (!snippet.group.trim().is_empty()).then(|| snippet.group.clone()),
                    self.selected_snippet_id.as_deref() == Some(snippet.id.as_str()),
                    false,
                    true,
                    index + 1,
                    count,
                )
            })
            .collect::<Vec<_>>();
        let target = self.active_pane().map(|pane| (pane.id, pane.title.clone()));
        let mut controls = vec![
            control(
                VaultKeySnippetAction::NewSnippet,
                None,
                MessageId::SnippetNewAction,
                false,
            ),
            control(
                VaultKeySnippetAction::SaveSnippet,
                None,
                MessageId::CommonSave,
                false,
            ),
        ];
        for (snippet, row) in self
            .saved
            .snippets
            .iter()
            .zip(rows.iter().map(|row| row.id))
        {
            for (action, name) in [
                (
                    VaultKeySnippetAction::SelectSnippet(row),
                    MessageId::SnippetSelectAction,
                ),
                (
                    VaultKeySnippetAction::ToggleSnippetPinned(row),
                    MessageId::SnippetPinAction,
                ),
                (
                    VaultKeySnippetAction::DeleteSnippet(row),
                    MessageId::SnippetDeleteAction,
                ),
            ] {
                controls.push(control(action, Some(row), name, false));
            }
            let (pane_id, title) = target.clone().unwrap_or_default();
            let mut insert = control(
                VaultKeySnippetAction::InsertSnippetAsText {
                    snippet: snippet_row_id(snippet),
                    pane_id,
                },
                Some(row),
                MessageId::SnippetInsertAction,
                pane_id == 0,
            );
            insert.value = (pane_id != 0).then_some(title);
            controls.push(insert);
        }
        snapshot(
            VaultKeySnippetScreen::Snippets,
            if rows.is_empty() {
                VaultKeySnippetSurfaceState::Empty
            } else if target.is_none() {
                VaultKeySnippetSurfaceState::TerminalRequired
            } else {
                VaultKeySnippetSurfaceState::Ready
            },
            rows,
            controls,
            self.activity_center.policy().recording_friendly,
        )
    }

    fn key_lifecycle_semantic_snapshot(
        &self,
        dialog: &KeyLifecycleDialog,
    ) -> VaultKeySnippetSemanticSnapshot {
        let (state, rows) = match dialog {
            KeyLifecycleDialog::Generate => (VaultKeySnippetSurfaceState::Editing, Vec::new()),
            KeyLifecycleDialog::Generating => (VaultKeySnippetSurfaceState::Generating, Vec::new()),
            KeyLifecycleDialog::Generated { .. } => {
                (VaultKeySnippetSurfaceState::Completed, Vec::new())
            }
            KeyLifecycleDialog::Review { .. } => {
                (VaultKeySnippetSurfaceState::Reviewing, Vec::new())
            }
            KeyLifecycleDialog::Running { .. } => {
                (VaultKeySnippetSurfaceState::Running, Vec::new())
            }
            KeyLifecycleDialog::Result { success, .. } => (
                if *success {
                    VaultKeySnippetSurfaceState::Completed
                } else {
                    VaultKeySnippetSurfaceState::Error
                },
                Vec::new(),
            ),
            KeyLifecycleDialog::HostPicker {
                identity_id,
                action,
            } => {
                let owner = key_operation_owner(identity_id, *action);
                let count = self.saved.profiles.len().max(1);
                let rows = self
                    .saved
                    .profiles
                    .iter()
                    .enumerate()
                    .map(|(index, profile)| {
                        row(
                            VaultKeySnippetRowId::host(
                                owner,
                                stable_vault_key_snippet_value(&profile.id),
                            ),
                            None,
                            profile.display_name(),
                            MessageId::VaultKeySnippetStateReady,
                            Some(profile.endpoint()),
                            false,
                            false,
                            true,
                            index + 1,
                            count,
                        )
                    })
                    .collect();
                (VaultKeySnippetSurfaceState::Reviewing, rows)
            }
        };
        let mut controls = match dialog {
            KeyLifecycleDialog::Generate | KeyLifecycleDialog::Review { .. } => vec![control(
                VaultKeySnippetAction::ConfirmKeyOperation,
                None,
                MessageId::KeyConfirmOperationAction,
                false,
            )],
            KeyLifecycleDialog::Running { .. } => vec![control(
                VaultKeySnippetAction::CancelKeyOperation,
                None,
                MessageId::CommonCancel,
                false,
            )],
            KeyLifecycleDialog::HostPicker { .. } => rows
                .iter()
                .map(|row| {
                    control(
                        VaultKeySnippetAction::SelectHost(row.id),
                        Some(row.id),
                        MessageId::KeySelectHostAction,
                        false,
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        if !matches!(
            dialog,
            KeyLifecycleDialog::Generating | KeyLifecycleDialog::Running { .. }
        ) {
            controls.push(control(
                VaultKeySnippetAction::CloseKeyLifecycle,
                None,
                MessageId::CommonClose,
                false,
            ));
        }
        snapshot(
            VaultKeySnippetScreen::KeyLifecycle,
            state,
            rows,
            controls,
            self.activity_center.policy().recording_friendly,
        )
    }

    pub(super) fn handle_vault_key_snippet_accessibility_command(
        &mut self,
        command: VaultKeySnippetAccessibilityCommand,
        _value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.vault_key_snippet_semantic_snapshot(cx) else {
            return;
        };
        match command {
            VaultKeySnippetAccessibilityCommand::FocusRow(_)
            | VaultKeySnippetAccessibilityCommand::FocusControl(_) => {
                self.project_list_focus.focus(window)
            }
            VaultKeySnippetAccessibilityCommand::ActivateRow(row) => {
                if snapshot.rows.iter().any(|candidate| {
                    candidate.id == row && candidate.activatable && !candidate.disabled
                }) {
                    self.activate_sensitive_row(row, window, cx);
                }
            }
            VaultKeySnippetAccessibilityCommand::ActivateControl(action) => {
                if snapshot
                    .controls
                    .iter()
                    .any(|control| control.action == action && !control.disabled)
                {
                    self.activate_sensitive_control(action, window, cx);
                }
            }
            VaultKeySnippetAccessibilityCommand::SetControlValue(_) => {}
        }
    }

    pub(super) fn begin_snippet_insert(
        &mut self,
        row: VaultKeySnippetRowId,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_pane().map(|pane| pane.id) != Some(pane_id) {
            self.error_message = crate::ui::localization::snippet_error_stale_terminal();
            cx.notify();
            return;
        }
        let Some(snippet_id) = self
            .saved
            .snippets
            .iter()
            .find(|snippet| snippet_row_id(snippet) == row)
            .map(|snippet| snippet.id.clone())
        else {
            self.error_message = crate::ui::localization::snippet_error_stale();
            cx.notify();
            return;
        };
        self.prepare_snippet_insert(&snippet_id, pane_id, window, cx);
    }

    fn activate_sensitive_row(
        &mut self,
        row: VaultKeySnippetRowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use termirust_ui_contract::VaultKeySnippetRowKind as Kind;
        match row.kind {
            Kind::Vault => {
                if let Some(id) = self
                    .saved
                    .vaults
                    .iter()
                    .find(|item| vault_row_id(&item.id) == row)
                    .map(|item| item.id.clone())
                {
                    self.load_vault_into_inputs(&id, window, cx);
                }
            }
            Kind::Member => {
                if let Some(id) = self
                    .saved
                    .vaults
                    .iter()
                    .flat_map(|vault| vault.members.iter().map(move |member| (vault, member)))
                    .find(|(vault, member)| member_row_id(&vault.id, &member.id) == row)
                    .map(|(_, member)| member.id.clone())
                {
                    self.load_vault_member_into_inputs(&id, window, cx);
                }
            }
            Kind::Key | Kind::Identity => {
                if let Some(identity) = self
                    .saved
                    .identities
                    .iter()
                    .find(|item| identity_row_id(item, self.keychain_tab) == row)
                    .cloned()
                {
                    self.use_identity(&identity, window, cx);
                }
            }
            Kind::Snippet => {
                if let Some(id) = self
                    .saved
                    .snippets
                    .iter()
                    .find(|item| snippet_row_id(item) == row)
                    .map(|item| item.id.clone())
                {
                    self.load_snippet_into_inputs(&id, window, cx);
                }
            }
            Kind::Host => {
                let Some(KeyLifecycleDialog::HostPicker {
                    identity_id,
                    action,
                }) = self.key_lifecycle_dialog.clone()
                else {
                    return;
                };
                let owner = key_operation_owner(&identity_id, action);
                if row.owner != owner {
                    return;
                }
                if let Some(profile_id) = self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| {
                        VaultKeySnippetRowId::host(
                            owner,
                            stable_vault_key_snippet_value(&profile.id),
                        ) == row
                    })
                    .map(|profile| profile.id.clone())
                {
                    self.review_key_operation(identity_id, profile_id, action, cx);
                }
            }
        }
    }

    fn activate_sensitive_control(
        &mut self,
        action: VaultKeySnippetAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            VaultKeySnippetAction::ShowKeys => {
                self.keychain_tab = KeychainTab::Keys;
                cx.notify();
            }
            VaultKeySnippetAction::ShowIdentities => {
                self.keychain_tab = KeychainTab::Identities;
                cx.notify();
            }
            VaultKeySnippetAction::NewVault => self.clear_vault_form(window, cx),
            VaultKeySnippetAction::SaveVault => self.save_vault(window, cx),
            VaultKeySnippetAction::SelectVault(row)
            | VaultKeySnippetAction::UseKey(row)
            | VaultKeySnippetAction::SelectSnippet(row)
            | VaultKeySnippetAction::SelectHost(row) => {
                self.activate_sensitive_row(row, window, cx)
            }
            VaultKeySnippetAction::DeleteVault(row)
                if self
                    .selected_vault_id
                    .as_deref()
                    .is_some_and(|id| vault_row_id(id) == row) =>
            {
                self.remove_selected_vault(window, cx)
            }
            VaultKeySnippetAction::SaveMember(row)
                if self
                    .selected_vault_id
                    .as_deref()
                    .is_some_and(|id| vault_row_id(id) == row) =>
            {
                self.save_vault_member(window, cx)
            }
            VaultKeySnippetAction::DeleteMember(row) => {
                if let Some(id) = self.selected_vault_id.as_deref().and_then(|vault_id| {
                    self.saved
                        .vaults
                        .iter()
                        .find(|vault| vault.id == vault_id)?
                        .members
                        .iter()
                        .find(|member| member_row_id(vault_id, &member.id) == row)
                        .map(|member| member.id.clone())
                }) {
                    self.remove_vault_member(&id, window, cx);
                }
            }
            VaultKeySnippetAction::GenerateKey => self.open_key_generation(window, cx),
            VaultKeySnippetAction::AddKeyFile => self.pick_key_file(window, cx),
            VaultKeySnippetAction::DeployKey(row) | VaultKeySnippetAction::RemoveRemoteKey(row) => {
                let operation = if matches!(action, VaultKeySnippetAction::DeployKey(_)) {
                    AuthorizedKeyAction::Add
                } else {
                    AuthorizedKeyAction::Remove
                };
                if let Some(id) = self
                    .saved
                    .identities
                    .iter()
                    .find(|identity| identity_row_id(identity, self.keychain_tab) == row)
                    .map(|identity| identity.id.clone())
                {
                    self.open_key_host_picker(id, operation, window, cx);
                }
            }
            VaultKeySnippetAction::ConfirmKeyOperation => {
                if matches!(
                    self.key_lifecycle_dialog,
                    Some(KeyLifecycleDialog::Generate)
                ) {
                    self.choose_generated_key_destination(window, cx);
                } else {
                    self.start_key_operation(window, cx);
                }
            }
            VaultKeySnippetAction::CancelKeyOperation => self.cancel_key_operation(cx),
            VaultKeySnippetAction::CloseKeyLifecycle => self.close_key_lifecycle(window, cx),
            VaultKeySnippetAction::NewSnippet => self.clear_snippet_form(window, cx),
            VaultKeySnippetAction::SaveSnippet => self.save_snippet(window, cx),
            VaultKeySnippetAction::DeleteSnippet(row)
                if self
                    .selected_snippet_id
                    .as_deref()
                    .and_then(|id| self.saved.snippets.iter().find(|snippet| snippet.id == id))
                    .is_some_and(|snippet| snippet_row_id(snippet) == row) =>
            {
                self.remove_selected_snippet(window, cx)
            }
            VaultKeySnippetAction::ToggleSnippetPinned(row) => {
                if let Some((id, pinned)) = self
                    .saved
                    .snippets
                    .iter()
                    .find(|snippet| snippet_row_id(snippet) == row)
                    .map(|snippet| (snippet.id.clone(), !snippet.pinned))
                {
                    self.set_saved_snippet_pinned(&id, pinned, window, cx);
                }
            }
            VaultKeySnippetAction::InsertSnippetAsText { snippet, pane_id } => {
                self.begin_snippet_insert(snippet, pane_id, window, cx)
            }
            VaultKeySnippetAction::ConfirmSnippetPrompts => self.confirm_snippet_prompts(cx),
            VaultKeySnippetAction::CancelSnippetPrompts => self.cancel_snippet_prompts(cx),
            VaultKeySnippetAction::ConfirmSnippetInsert { snippet, pane_id } => {
                if self.pending_snippet_insert.as_ref().is_some_and(|pending| {
                    pending.pane_id == pane_id
                        && self
                            .saved
                            .snippets
                            .iter()
                            .find(|item| item.id == pending.snippet_id)
                            .is_some_and(|item| snippet_row_id(item) == snippet)
                }) {
                    self.confirm_pending_snippet_insert(cx);
                }
            }
            VaultKeySnippetAction::CancelSnippetInsert => self.cancel_pending_snippet_insert(cx),
            _ => {}
        }
    }
}

fn snapshot(
    screen: VaultKeySnippetScreen,
    state: VaultKeySnippetSurfaceState,
    rows: Vec<VaultKeySnippetRow>,
    controls: Vec<VaultKeySnippetControl>,
    recording_friendly: bool,
) -> VaultKeySnippetSemanticSnapshot {
    VaultKeySnippetSemanticSnapshot {
        screen,
        state,
        rows,
        controls,
        recording_friendly,
    }
}

#[allow(clippy::too_many_arguments)]
fn row(
    id: VaultKeySnippetRowId,
    parent: Option<VaultKeySnippetRowId>,
    name: String,
    status: MessageId,
    detail: Option<String>,
    selected: bool,
    disabled: bool,
    activatable: bool,
    position: usize,
    set_size: usize,
) -> VaultKeySnippetRow {
    VaultKeySnippetRow {
        id,
        parent,
        name,
        status,
        detail,
        selected,
        disabled,
        activatable,
        destructive: false,
        position,
        set_size,
    }
}

fn control(
    action: VaultKeySnippetAction,
    parent: Option<VaultKeySnippetRowId>,
    name: MessageId,
    disabled: bool,
) -> VaultKeySnippetControl {
    VaultKeySnippetControl {
        action,
        parent,
        role: VaultKeySnippetControlRole::Button,
        name,
        value: None,
        secret: None,
        selected: false,
        disabled,
        invalid: false,
        destructive: matches!(
            action,
            VaultKeySnippetAction::DeleteVault(_)
                | VaultKeySnippetAction::DeleteMember(_)
                | VaultKeySnippetAction::RemoveRemoteKey(_)
                | VaultKeySnippetAction::DeleteSnippet(_)
        ),
    }
}

fn vault_row_id(id: &str) -> VaultKeySnippetRowId {
    VaultKeySnippetRowId::vault(stable_vault_key_snippet_value(id))
}
fn member_row_id(vault: &str, member: &str) -> VaultKeySnippetRowId {
    VaultKeySnippetRowId::member(
        stable_vault_key_snippet_value(vault),
        stable_vault_key_snippet_value(member),
    )
}
fn identity_row_id(identity: &SavedIdentity, tab: KeychainTab) -> VaultKeySnippetRowId {
    let owner =
        stable_vault_key_snippet_value(identity.vault_id.as_deref().unwrap_or(DEFAULT_VAULT_ID));
    let value = stable_vault_key_snippet_value(&identity.id);
    match tab {
        KeychainTab::Keys => VaultKeySnippetRowId::key(owner, value),
        KeychainTab::Identities => VaultKeySnippetRowId::identity(owner, value),
    }
}
fn snippet_row_id(snippet: &SavedSnippet) -> VaultKeySnippetRowId {
    VaultKeySnippetRowId::snippet(
        stable_vault_key_snippet_value(snippet.effective_vault_id()),
        stable_vault_key_snippet_value(&snippet.id),
    )
}
fn key_operation_owner(identity: &str, action: AuthorizedKeyAction) -> u128 {
    stable_vault_key_snippet_value(&format!("{identity}:{action:?}"))
}
