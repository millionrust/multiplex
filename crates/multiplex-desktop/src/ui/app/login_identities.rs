//! Identities: logins that hosts share. An identity is a username with a password, a private
//! key (and the key's passphrase, if it has one) or the SSH agent. A host that chooses one
//! signs in with it instead of carrying its own credentials, and editing the identity changes
//! every host that uses it. Passwords and passphrases go to the system credential store; the
//! saved state holds only their ids.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, StyledExt as _, h_flex, v_flex};
use multiplex_ui_contract::MessageId;
use rfd::AsyncFileDialog;

use crate::credentials;
use crate::models::{AuthMode, SavedLoginIdentity};
use crate::ssh_auth::{PrivateKeyCheck, check_private_key};
use crate::storage::{expand_home_path, save_saved_state};
use crate::ui::app::{KeychainTab, MultiplexApp, NavSection};
use crate::ui::localization;
use crate::ui::theme;

fn copy(id: MessageId) -> String {
    localization::static_message(id)
}

pub(super) struct LoginIdentityInputs {
    pub(super) label: Entity<InputState>,
    pub(super) username: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    pub(super) key_path: Entity<InputState>,
    pub(super) passphrase: Entity<InputState>,
    pub(super) agent_socket: Entity<InputState>,
}

impl LoginIdentityInputs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MultiplexApp>) -> Self {
        Self {
            label: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(copy(MessageId::LoginIdentityLabelPlaceholder))
            }),
            username: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(copy(MessageId::LoginIdentityUsernamePlaceholder))
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(copy(MessageId::LoginIdentityPasswordPlaceholder))
            }),
            key_path: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(copy(MessageId::LoginIdentityKeyPlaceholder))
            }),
            passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(copy(MessageId::LoginIdentityPassphrasePlaceholder))
            }),
            agent_socket: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(copy(MessageId::LoginIdentityAgentPlaceholder))
            }),
        }
    }
}

/// The identity being created or edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoginIdentityForm {
    /// The identity being edited, or `None` for a new one.
    pub(super) editing_id: Option<String>,
    pub(super) auth_mode: AuthMode,
    /// Started from the host editor, which gets the new identity once it is saved.
    pub(super) for_host_editor: bool,
}

impl MultiplexApp {
    fn login_identity_input(input: &Entity<InputState>, cx: &Context<Self>) -> String {
        input.read(cx).value().to_string()
    }

    fn fill_login_identity_inputs(
        &mut self,
        identity: Option<&SavedLoginIdentity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = &self.login_identity_inputs;
        let (label, username, key_path, agent) = identity.map_or_else(Default::default, |it| {
            (
                it.label.clone(),
                it.username.clone(),
                it.key_path.clone(),
                it.identity_agent.clone().unwrap_or_default(),
            )
        });
        Self::set_input_value(&inputs.label, label, window, cx);
        Self::set_input_value(&inputs.username, username, window, cx);
        Self::set_input_value(&inputs.key_path, key_path, window, cx);
        Self::set_input_value(&inputs.agent_socket, agent, window, cx);
        Self::set_input_value(&inputs.password, "", window, cx);
        Self::set_input_value(&inputs.passphrase, "", window, cx);
        // A saved secret is never shown; the placeholder says one is kept.
        let kept = copy(MessageId::LoginIdentitySecretKeptPlaceholder);
        let password_placeholder = if identity.is_some_and(|it| it.password_credential_id.is_some())
        {
            kept.clone()
        } else {
            copy(MessageId::LoginIdentityPasswordPlaceholder)
        };
        let passphrase_placeholder =
            if identity.is_some_and(|it| it.passphrase_credential_id.is_some()) {
                kept
            } else {
                copy(MessageId::LoginIdentityPassphrasePlaceholder)
            };
        inputs.password.update(cx, |state, cx| {
            state.set_placeholder(password_placeholder, window, cx)
        });
        inputs.passphrase.update(cx, |state, cx| {
            state.set_placeholder(passphrase_placeholder, window, cx)
        });
    }

    fn show_login_identities(&mut self) {
        self.nav_section = NavSection::Keychain;
        self.keychain_tab = KeychainTab::Identities;
    }

    pub(super) fn open_new_login_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill_login_identity_inputs(None, window, cx);
        self.login_identity_form = Some(LoginIdentityForm {
            editing_id: None,
            auth_mode: AuthMode::Password,
            for_host_editor: false,
        });
        self.show_login_identities();
        self.error_message.clear();
        cx.notify();
    }

    /// "New identity" in the host editor: the same form, and back to the host afterwards
    /// with the new identity chosen.
    pub(super) fn open_new_login_identity_from_host_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_login_identity(window, cx);
        if let Some(form) = self.login_identity_form.as_mut() {
            form.for_host_editor = true;
        }
    }

    pub(super) fn edit_login_identity(
        &mut self,
        identity_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(identity) = self.saved.login_identity(identity_id).cloned() else {
            return;
        };
        self.fill_login_identity_inputs(Some(&identity), window, cx);
        self.login_identity_form = Some(LoginIdentityForm {
            editing_id: Some(identity.id.clone()),
            auth_mode: identity.auth_mode,
            for_host_editor: false,
        });
        self.show_login_identities();
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn set_login_identity_auth_mode(&mut self, mode: AuthMode, cx: &mut Context<Self>) {
        if let Some(form) = self.login_identity_form.as_mut() {
            form.auth_mode = mode;
            self.error_message.clear();
            cx.notify();
        }
    }

    pub(super) fn close_login_identity_form(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.login_identity_form = None;
        // Typed secrets do not outlive the form.
        Self::set_input_value(&self.login_identity_inputs.password, "", window, cx);
        Self::set_input_value(&self.login_identity_inputs.passphrase, "", window, cx);
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn pick_login_identity_key_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = Self::take_dialog_path_for_tests() {
            Self::set_input_value(
                &self.login_identity_inputs.key_path,
                path.display().to_string(),
                window,
                cx,
            );
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = AsyncFileDialog::new()
                .set_title("Choose private key file")
                .pick_file()
                .await
                .map(|file| file.path().to_path_buf())
            else {
                return;
            };
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    Self::set_input_value(
                        &app.login_identity_inputs.key_path,
                        path.display().to_string(),
                        window,
                        cx,
                    );
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Checks the form, keeps its secrets in the credential store and saves the identity.
    /// Says whether it was saved; when it was not, `error_message` says why.
    pub(super) fn save_login_identity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let for_host_editor = self
            .login_identity_form
            .as_ref()
            .is_some_and(|form| form.for_host_editor);
        match self.try_save_login_identity(cx) {
            Ok(identity) => {
                self.close_login_identity_form(window, cx);
                if for_host_editor {
                    self.nav_section = NavSection::Hosts;
                    self.show_editor_panel = true;
                    self.set_draft_login_identity(Some(identity.id.clone()), window, cx);
                }
                self.status_message =
                    localization::login_identity_saved_status(identity.display_name());
                true
            }
            Err(error) => {
                self.error_message = error;
                cx.notify();
                false
            }
        }
    }

    fn try_save_login_identity(
        &mut self,
        cx: &Context<Self>,
    ) -> Result<SavedLoginIdentity, String> {
        let Some(form) = self.login_identity_form.clone() else {
            return Err(copy(MessageId::LoginIdentityUsernameRequiredError));
        };
        let inputs = &self.login_identity_inputs;
        let label = Self::login_identity_input(&inputs.label, cx)
            .trim()
            .to_string();
        let username = Self::login_identity_input(&inputs.username, cx)
            .trim()
            .to_string();
        let password = Self::login_identity_input(&inputs.password, cx);
        let passphrase = Self::login_identity_input(&inputs.passphrase, cx);
        let key_path = expand_home_path(Self::login_identity_input(&inputs.key_path, cx).trim());
        let agent_socket = Self::login_identity_input(&inputs.agent_socket, cx)
            .trim()
            .to_string();

        let id = form
            .editing_id
            .clone()
            .unwrap_or_else(SavedLoginIdentity::new_id);
        let existing = self.saved.login_identity(&id).cloned();
        let saved_password = existing
            .as_ref()
            .and_then(|it| it.password_credential_id.clone());
        let saved_passphrase = existing
            .as_ref()
            .and_then(|it| it.passphrase_credential_id.clone());

        if username.is_empty() {
            return Err(copy(MessageId::LoginIdentityUsernameRequiredError));
        }
        // Whether a passphrase has to be kept for the key: checked against the file now, so a
        // wrong one is caught here rather than on the first connect.
        let mut key_needs_passphrase = false;
        match form.auth_mode {
            AuthMode::Password if password.is_empty() && saved_password.is_none() => {
                return Err(copy(MessageId::LoginIdentityPasswordRequiredError));
            }
            AuthMode::PrivateKey => {
                if key_path.is_empty() {
                    return Err(copy(MessageId::LoginIdentityKeyRequiredError));
                }
                let kept_passphrase = if passphrase.is_empty() {
                    saved_passphrase
                        .as_deref()
                        .and_then(|credential_id| credentials::load_password(credential_id).ok())
                } else {
                    None
                };
                let candidate = if passphrase.is_empty() {
                    kept_passphrase.as_deref()
                } else {
                    Some(passphrase.as_str())
                };
                match check_private_key(&key_path, candidate) {
                    PrivateKeyCheck::Opens => {
                        key_needs_passphrase = candidate.is_some()
                            && check_private_key(&key_path, None) != PrivateKeyCheck::Opens;
                    }
                    PrivateKeyCheck::NeedsPassphrase => {
                        return Err(copy(MessageId::LoginIdentityPassphraseNeededError));
                    }
                    PrivateKeyCheck::WrongPassphrase => {
                        return Err(copy(MessageId::LoginIdentityPassphraseWrongError));
                    }
                    PrivateKeyCheck::Unreadable => {
                        return Err(copy(MessageId::LoginIdentityKeyUnreadableError));
                    }
                }
            }
            _ => {}
        }

        let store = |credential_id: String, secret: &str| {
            credentials::store_password(&credential_id, secret)
                .map(|()| credential_id)
                .map_err(|error| error.to_string())
        };
        let forget = |credential_id: Option<String>| {
            if let Some(credential_id) = credential_id {
                let _ = credentials::delete_password(&credential_id);
            }
        };
        let password_credential_id = if form.auth_mode == AuthMode::Password {
            if password.is_empty() {
                saved_password
            } else {
                Some(store(
                    credentials::identity_password_credential_id(&id),
                    &password,
                )?)
            }
        } else {
            forget(saved_password);
            None
        };
        let passphrase_credential_id = if key_needs_passphrase {
            if passphrase.is_empty() {
                saved_passphrase
            } else {
                Some(store(
                    credentials::identity_passphrase_credential_id(&id),
                    &passphrase,
                )?)
            }
        } else {
            forget(saved_passphrase);
            None
        };

        let identity = SavedLoginIdentity {
            id,
            label,
            vault_id: existing
                .and_then(|it| it.vault_id)
                .or_else(|| Some(self.effective_vault_id(self.selected_vault_id.as_deref()))),
            username,
            auth_mode: form.auth_mode,
            key_path: if form.auth_mode == AuthMode::PrivateKey {
                key_path
            } else {
                String::new()
            },
            identity_agent: (form.auth_mode == AuthMode::LocalAgent && !agent_socket.is_empty())
                .then_some(agent_socket),
            password_credential_id,
            passphrase_credential_id,
        };
        self.saved.upsert_login_identity(identity.clone());
        save_saved_state(&self.saved).map_err(|error| error.to_string())?;
        Ok(identity)
    }

    /// Deletes an identity and the secrets it kept. Hosts that used it keep its username.
    pub(super) fn delete_login_identity(&mut self, identity_id: &str, cx: &mut Context<Self>) {
        let Some(identity) = self.saved.remove_login_identity(identity_id) else {
            return;
        };
        for credential_id in [
            identity.password_credential_id.as_deref(),
            identity.passphrase_credential_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = credentials::delete_password(credential_id);
        }
        if self.draft_login_identity_id.as_deref() == Some(identity_id) {
            self.draft_login_identity_id = None;
        }
        if self
            .login_identity_form
            .as_ref()
            .is_some_and(|form| form.editing_id.as_deref() == Some(identity_id))
        {
            self.login_identity_form = None;
        }
        if let Err(error) = save_saved_state(&self.saved) {
            self.error_message = error.to_string();
        } else {
            self.status_message =
                localization::login_identity_deleted_status(identity.display_name());
            self.error_message.clear();
        }
        cx.notify();
    }

    /// Chooses the identity the host being edited signs in with, or none.
    pub(super) fn set_draft_login_identity(
        &mut self,
        identity_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = identity_id
            .as_deref()
            .and_then(|identity_id| self.saved.login_identity(identity_id))
            .cloned();
        self.draft_login_identity_id = identity.as_ref().map(|identity| identity.id.clone());
        if let Some(identity) = identity {
            // The form shows what the host will use, so leaving the identity later starts from
            // its username rather than an empty field.
            self.draft_auth_mode = identity.auth_mode;
            Self::set_input_value(&self.inputs.username, identity.username.clone(), window, cx);
            Self::set_input_value(&self.inputs.password, "", window, cx);
            Self::set_input_value(&self.inputs.key_passphrase, "", window, cx);
        }
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn login_identity_method_label(identity: &SavedLoginIdentity) -> String {
        copy(match identity.auth_mode {
            AuthMode::Password => MessageId::LoginIdentityMethodPassword,
            AuthMode::PrivateKey if identity.passphrase_credential_id.is_some() => {
                MessageId::LoginIdentityMethodKeyPassphrase
            }
            AuthMode::PrivateKey => MessageId::LoginIdentityMethodKey,
            AuthMode::LocalAgent => MessageId::LoginIdentityMethodAgent,
        })
    }

    /// The Identities tab in Keys: the saved identities, and the form while one is edited.
    pub(super) fn render_login_identities_tab(&self, cx: &Context<Self>) -> Div {
        let identities = &self.saved.login_identities;
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SIZE))
                            .text_color(theme::text_muted())
                            .child(copy(MessageId::LoginIdentitiesDescription)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::login_identity_count(identities.len())),
                            )
                            .child(
                                Self::design_button(
                                    "login-identity-new",
                                    theme::ActionTone::Accent,
                                    cx,
                                )
                                .debug_selector(|| "login-identity-new".to_string())
                                .icon(Icon::new(IconName::Plus))
                                .label(copy(MessageId::LoginIdentityNewAction))
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.open_new_login_identity(window, cx);
                                    },
                                )),
                            ),
                    ),
            )
            .when_some(self.login_identity_form.clone(), |this, form| {
                this.child(self.render_login_identity_form(form, cx))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(identities.iter().enumerate().map(|(index, identity)| {
                        self.render_login_identity_card(index, identity, cx)
                    }))
                    .when(
                        identities.is_empty() && self.login_identity_form.is_none(),
                        |this| {
                            this.child(
                                self.render_library_empty_state(
                                    Icon::new(IconName::User)
                                        .size(px(theme::ICON_SIZE_LARGE))
                                        .text_color(theme::accent()),
                                    copy(MessageId::LoginIdentitiesEmptyTitle),
                                    copy(MessageId::LoginIdentitiesEmptyDescription),
                                ),
                            )
                        },
                    ),
            )
    }

    fn render_login_identity_card(
        &self,
        index: usize,
        identity: &SavedLoginIdentity,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let edit_id = identity.id.clone();
        let delete_id = identity.id.clone();
        let hosts = self.saved.hosts_using_login_identity(&identity.id);
        h_flex()
            .id(("login-identity-card", index))
            .debug_selector(move || format!("login-identity-card-{index}"))
            .justify_between()
            .items_center()
            .gap_4()
            .p_4()
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border())
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .size(px(theme::SENSITIVE_ICON_TILE_SIZE))
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::with_alpha(theme::accent(), 0.1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::User)
                                    .size(px(theme::ICON_SIZE_DEFAULT))
                                    .text_color(theme::accent()),
                            ),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_FINE))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .truncate()
                                    .child(identity.display_name()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .truncate()
                                    .child(localization::host_editor_identity_summary(
                                        identity.username.clone(),
                                        Self::login_identity_method_label(identity),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::login_identity_host_count(hosts)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        Self::design_button(
                            ("login-identity-edit", index),
                            theme::ActionTone::Neutral,
                            cx,
                        )
                        .debug_selector(move || format!("login-identity-edit-{index}"))
                        .label(copy(MessageId::LoginIdentityEditAction))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.edit_login_identity(&edit_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        Self::design_button(
                            ("login-identity-delete", index),
                            theme::ActionTone::Danger,
                            cx,
                        )
                        .debug_selector(move || format!("login-identity-delete-{index}"))
                        .label(copy(MessageId::LoginIdentityDeleteAction))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_login_identity(&delete_id, cx);
                        })),
                    ),
            )
            .into_any_element()
    }

    fn render_login_identity_form(
        &self,
        form: LoginIdentityForm,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let inputs = &self.login_identity_inputs;
        let mode = form.auth_mode;
        v_flex()
            .id("login-identity-form")
            .debug_selector(|| "login-identity-form".to_string())
            .gap_3()
            .p_4()
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::border_strong())
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(copy(if form.editing_id.is_some() {
                        MessageId::LoginIdentityFormEditTitle
                    } else {
                        MessageId::LoginIdentityFormNewTitle
                    })),
            )
            .child(self.form_field(
                copy(MessageId::LoginIdentityLabelField),
                Input::new(&inputs.label),
            ))
            .child(self.form_field(
                copy(MessageId::LoginIdentityUsernameField),
                Input::new(&inputs.username),
            ))
            .child(self.segmented_control(
                "login-identity-method",
                [
                    (
                        AuthMode::Password,
                        copy(MessageId::LoginIdentityMethodPassword),
                    ),
                    (
                        AuthMode::PrivateKey,
                        copy(MessageId::LoginIdentityMethodKey),
                    ),
                    (
                        AuthMode::LocalAgent,
                        copy(MessageId::LoginIdentityMethodAgent),
                    ),
                ],
                mode,
                false,
                cx,
                |this, mode, _, cx| this.set_login_identity_auth_mode(mode, cx),
            ))
            .when(mode == AuthMode::Password, |this| {
                this.child(self.form_field(
                    copy(MessageId::LoginIdentityPasswordField),
                    Input::new(&inputs.password).mask_toggle(),
                ))
            })
            .when(mode == AuthMode::PrivateKey, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_end()
                        .child(div().flex_1().min_w_0().child(self.form_field(
                            copy(MessageId::LoginIdentityKeyField),
                            Input::new(&inputs.key_path),
                        )))
                        .child(
                            Self::design_button(
                                "login-identity-browse",
                                theme::ActionTone::Neutral,
                                cx,
                            )
                            .debug_selector(|| "login-identity-browse".to_string())
                            .label(copy(MessageId::LoginIdentityBrowseAction))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.pick_login_identity_key_file(window, cx);
                                },
                            )),
                        ),
                )
                .child(self.form_field(
                    copy(MessageId::LoginIdentityPassphraseField),
                    Input::new(&inputs.passphrase).mask_toggle(),
                ))
            })
            .when(mode == AuthMode::LocalAgent, |this| {
                this.child(self.form_field(
                    copy(MessageId::LoginIdentityAgentField),
                    Input::new(&inputs.agent_socket),
                ))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Self::design_button(
                            "login-identity-cancel",
                            theme::ActionTone::Neutral,
                            cx,
                        )
                        .debug_selector(|| "login-identity-cancel".to_string())
                        .label(copy(MessageId::LoginIdentityCancelAction))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_login_identity_form(window, cx);
                        })),
                    )
                    .child(
                        Self::design_button("login-identity-save", theme::ActionTone::Accent, cx)
                            .debug_selector(|| "login-identity-save".to_string())
                            .label(copy(MessageId::LoginIdentitySaveAction))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_login_identity(window, cx);
                            })),
                    ),
            )
    }

    /// The host editor's choice of identity: None, then each saved identity. While one is
    /// chosen, the editor shows who the host signs in as instead of its own credential fields.
    pub(super) fn render_editor_identity_picker(&self, cx: &Context<Self>) -> Stateful<Div> {
        let chosen = self.draft_login_identity_id.clone();
        let option = |id: &'static str,
                      index: usize,
                      label: String,
                      selected: bool,
                      value: Option<String>| {
            div()
                .id((id, index))
                .debug_selector(move || format!("{id}-{index}"))
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::CONTROL_RADIUS))
                .border_1()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::border()
                })
                .bg(if selected {
                    theme::with_alpha(theme::accent(), 0.12)
                } else {
                    theme::library_card()
                })
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(if selected {
                    theme::text_main()
                } else {
                    theme::text_muted()
                })
                .cursor_pointer()
                .hover(|style| style.bg(theme::card_hover()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_draft_login_identity(value.clone(), window, cx);
                }))
                .child(label)
        };
        let linked = chosen
            .as_deref()
            .and_then(|identity_id| self.saved.login_identity(identity_id));
        v_flex()
            .id("editor-identity")
            .debug_selector(|| "editor-identity".to_string())
            .gap(px(theme::SPACE_2))
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(copy(MessageId::HostEditorIdentityField)),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(option(
                        "editor-identity-option",
                        0,
                        copy(MessageId::HostEditorIdentityNone),
                        chosen.is_none(),
                        None,
                    ))
                    .children(self.saved.login_identities.iter().enumerate().map(
                        |(index, identity)| {
                            option(
                                "editor-identity-option",
                                index + 1,
                                identity.display_name(),
                                chosen.as_deref() == Some(identity.id.as_str()),
                                Some(identity.id.clone()),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_NANO_SIZE))
                    .text_color(theme::text_muted())
                    .child(match linked {
                        Some(identity) => localization::host_editor_identity_summary(
                            identity.username.clone(),
                            Self::login_identity_method_label(identity),
                        ),
                        None => copy(MessageId::HostEditorIdentityHelp),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::LoginIdentityForm;
    use crate::models::AuthMode;

    #[test]
    fn a_new_form_starts_with_a_password() {
        let form = LoginIdentityForm {
            editing_id: None,
            auth_mode: AuthMode::Password,
            for_host_editor: false,
        };
        assert_eq!(form.auth_mode, AuthMode::default());
    }
}
