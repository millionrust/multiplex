//! Library secondary pages: Keychain (Keys + Identities), Vaults, Known
//! Hosts, Logs, Snippets, and Settings. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use termirust_ui_contract::{MessageId, SettingsSectionId};

use crate::models::{
    AuthMode, DEFAULT_VAULT_ID, SessionLogEntry, SessionLogStatus, ThemePreset, VaultKind,
    VaultMemberRole,
};
use crate::replication::{desktop_replication_root, replication_is_configured};
use crate::ui::app::{
    ICON_KEY, ICON_SHIELD_CHECK, KeychainTab, NavSection, TermiRustApp, app_icon,
    platform_shortcut_label,
};
use crate::ui::localization;
use crate::ui::theme;
use crate::ui::util::short_host_key;

fn library_copy(id: MessageId) -> String {
    localization::message_id(id).unwrap_or_default()
}

impl TermiRustApp {
    // termirust-ui-surface:vault-keys-snippets:start
    fn keychain_tab_control(&self, cx: &Context<Self>) -> Div {
        let tab = self.keychain_tab;
        h_flex()
            .p(px(theme::SPACE_MICRO))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::hover())
            .child(
                div()
                    .id("keychain-tab-keys")
                    .debug_selector(|| "keychain-tab-keys".to_string())
                    .flex_1()
                    .h(px(theme::SENSITIVE_TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(theme::SPACE_DENSE))
                    .rounded(px(theme::CONTROL_RADIUS))
                    .text_size(px(theme::TYPE_BODY_SIZE))
                    .font_medium()
                    .cursor_pointer()
                    .when(tab == KeychainTab::Keys, |this| {
                        this.bg(theme::library_card())
                            .shadow_sm()
                            .text_color(theme::text_main())
                    })
                    .when(tab != KeychainTab::Keys, |this| {
                        this.text_color(theme::text_muted())
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keychain_tab = KeychainTab::Keys;
                        cx.notify();
                    }))
                    .child(
                        app_icon(ICON_KEY)
                            .size(px(theme::ICON_SIZE_SMALL))
                            .text_color(if tab == KeychainTab::Keys {
                                theme::accent()
                            } else {
                                theme::text_muted()
                            }),
                    )
                    .child(library_copy(MessageId::KeychainKeysTitle)),
            )
            .child(
                div()
                    .id("keychain-tab-identities")
                    .debug_selector(|| "keychain-tab-identities".to_string())
                    .flex_1()
                    .h(px(theme::SENSITIVE_TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(theme::SPACE_DENSE))
                    .rounded(px(theme::CONTROL_RADIUS))
                    .text_size(px(theme::TYPE_BODY_SIZE))
                    .font_medium()
                    .cursor_pointer()
                    .when(tab == KeychainTab::Identities, |this| {
                        this.bg(theme::library_card())
                            .shadow_sm()
                            .text_color(theme::text_main())
                    })
                    .when(tab != KeychainTab::Identities, |this| {
                        this.text_color(theme::text_muted())
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keychain_tab = KeychainTab::Identities;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::User)
                            .size(px(theme::ICON_SIZE_SMALL))
                            .text_color(if tab == KeychainTab::Identities {
                                theme::accent()
                            } else {
                                theme::text_muted()
                            }),
                    )
                    .child(library_copy(MessageId::KeychainIdentitiesTitle)),
            )
    }

    fn render_keychain_keys(&self, cx: &Context<Self>) -> Div {
        let identities = self.saved.identities.clone();

        v_flex()
            .flex_1()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SIZE))
                            .text_color(theme::text_muted())
                            .child(library_copy(MessageId::KeychainKeysDescription)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(!identities.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(format!(
                                            "{} {}",
                                            identities.len(),
                                            if identities.len() == 1 {
                                                "key"
                                            } else {
                                                "keys"
                                            }
                                        )),
                                )
                            })
                            .child(
                                Button::new("keychain-generate")
                                    .debug_selector(|| "keychain-generate".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .icon(IconName::Plus)
                                    .label(library_copy(MessageId::KeyGenerateAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_key_generation(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("keychain-browse")
                                    .debug_selector(|| "keychain-browse".to_string())
                                    .small()
                                    .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                    .icon(IconName::FolderOpen)
                                    .label(library_copy(MessageId::KeyAddFileAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_key_file(window, cx);
                                        this.nav_section = NavSection::Hosts;
                                        this.show_editor_panel = true;
                                        this.draft_auth_mode = AuthMode::PrivateKey;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .gap_2()
                        .overflow_y_scrollbar()
                    .children(identities.iter().enumerate().map(
                        |(index, identity)| {
                            let card_identity = identity.clone();
                            let button_identity = identity.clone();
                            let vault_label =
                                self.effective_vault_name(identity.vault_id.as_deref());
                            let has_pub = std::path::Path::new(&format!("{}.pub", identity.key_path))
                                .exists();

                            h_flex()
                                .id(("keychain-key", index))
                                .debug_selector(move || format!("keychain-key-{index}"))
                                .justify_between()
                                .items_center()
                                .gap_4()
                                .p_4()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::library_card())
                                .border_1()
                                .border_color(if index == 0 {
                                    theme::with_alpha(theme::accent(), 0.28)
                                } else {
                                    theme::border()
                                })
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::card_hover()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.use_identity(&card_identity, window, cx);
                                }))
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            div()
                                                .size(px(theme::SENSITIVE_ICON_TILE_SIZE))
                                                .rounded(px(theme::CARD_RADIUS))
                                                .bg(theme::with_alpha(theme::accent(), 0.1))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    app_icon(ICON_KEY)
                                                        .size(px(theme::ICON_SIZE_DEFAULT))
                                                        .text_color(theme::accent()),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap(px(theme::SPACE_FINE))
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_size(px(theme::TYPE_BODY_SIZE))
                                                                .font_semibold()
                                                                .text_color(theme::text_main())
                                                                .child(
                                                                    button_identity.label.clone(),
                                                                ),
                                                        )
                                                        .child(self.status_badge(
                                                            &button_identity.kind,
                                                            theme::library_bg(),
                                                            theme::slate(),
                                                        ))
                                                        .when(
                                                            button_identity.source
                                                                == crate::models::IdentitySource::Imported,
                                                            |this| {
                                                                this.child(self.status_badge(
                                                                    library_copy(MessageId::KeySourceImported),
                                                                    theme::library_bg(),
                                                                    theme::accent(),
                                                                ))
                                                            },
                                                        )
                                                        .when(
                                                            button_identity.source
                                                                == crate::models::IdentitySource::Generated,
                                                            |this| {
                                                                this.child(self.status_badge(
                                                                    library_copy(MessageId::KeySourceGenerated),
                                                                    theme::library_bg(),
                                                                    theme::success(),
                                                                ))
                                                            },
                                                        )
                                                        .when(index == 0, |this| {
                                                            this.child(self.status_badge(
                                                                library_copy(MessageId::KeyDefaultBadge),
                                                                theme::library_bg(),
                                                                theme::accent(),
                                                            ))
                                                        })
                                                        .when(has_pub, |this| {
                                                            this.child(self.status_badge(
                                                                "pub",
                                                                theme::library_bg(),
                                                                theme::success(),
                                                            ))
                                                        })
                                                        .child(self.status_badge(
                                                            vault_label,
                                                            theme::library_bg(),
                                                            theme::accent(),
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                        .text_color(theme::text_muted())
                                                        .child(button_identity.key_path.clone()),
                                                ),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when(has_pub, |this| {
                                            let deploy_identity_id = button_identity.id.clone();
                                            let remove_identity_id = button_identity.id.clone();
                                            this.child(
                                                Button::new(("keychain-deploy", index))
                                                    .debug_selector(move || {
                                                        format!("keychain-deploy-{index}")
                                                    })
                                                    .small()
                                                    .custom(Self::action_button_style(
                                                        theme::ActionTone::Accent,
                                                        cx,
                                                    ))
                                                    .label(library_copy(MessageId::KeyDeployAction))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.open_key_host_picker(
                                                                deploy_identity_id.clone(),
                                                                crate::sftp::AuthorizedKeyAction::Add,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new(("keychain-remove-remote", index))
                                                    .debug_selector(move || {
                                                        format!("keychain-remove-remote-{index}")
                                                    })
                                                    .small()
                                                    .custom(Self::action_button_style(
                                                        theme::ActionTone::Neutral,
                                                        cx,
                                                    ))
                                                    .label(library_copy(MessageId::KeyRemoveRemoteAction))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.open_key_host_picker(
                                                                remove_identity_id.clone(),
                                                                crate::sftp::AuthorizedKeyAction::Remove,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        })
                                        .child(
                                            Button::new(("keychain-use", index))
                                                .debug_selector(move || {
                                                    format!("keychain-use-{index}")
                                                })
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::AccentSoft,
                                                    cx,
                                                ))
                                                .label(library_copy(MessageId::KeyUseAction))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.use_identity(
                                                            &button_identity,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        ),
                                )
                                .into_any_element()
                        },
                    ))
                    .when(identities.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                app_icon(ICON_KEY)
                                    .size(px(theme::ICON_SIZE_LARGE))
                                    .text_color(theme::accent()),
                                library_copy(MessageId::KeychainEmptyTitle),
                                library_copy(MessageId::KeychainEmptyDescription),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("keys-empty-generate")
                                            .debug_selector(|| {
                                                "keys-empty-generate".to_string()
                                            })
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .icon(IconName::Plus)
                                            .label(library_copy(MessageId::KeyGenerateAction))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_key_generation(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("keys-empty-add")
                                            .debug_selector(|| "keys-empty-add".to_string())
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .icon(IconName::FolderOpen)
                                            .label(library_copy(MessageId::KeyAddFileAction))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.pick_key_file(window, cx);
                                                this.nav_section = NavSection::Hosts;
                                                this.show_editor_panel = true;
                                                this.draft_auth_mode = AuthMode::PrivateKey;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn render_keychain_identities(&self, cx: &Context<Self>) -> Div {
        let profiles_with_password: Vec<_> = self
            .saved
            .profiles
            .iter()
            .filter(|p| p.auth_mode == AuthMode::Password)
            .collect();

        v_flex()
            .flex_1()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SIZE))
                            .text_color(theme::text_muted())
                            .child(library_copy(MessageId::KeychainIdentitiesDescription)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{} {}",
                                profiles_with_password.len(),
                                if profiles_with_password.len() == 1 {
                                    "identity"
                                } else {
                                    "identities"
                                }
                            )),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(
                        profiles_with_password
                            .iter()
                            .enumerate()
                            .map(|(index, profile)| {
                                let profile_id = profile.id.clone();
                                let vault_label =
                                    self.effective_vault_name(profile.vault_id.as_deref());
                                h_flex()
                                    .id(("identity-card", index))
                                    .debug_selector(move || format!("identity-card-{index}"))
                                    .justify_between()
                                    .items_center()
                                    .gap_4()
                                    .p_4()
                                    .rounded(px(theme::CARD_RADIUS))
                                    .bg(theme::library_card())
                                    .border_1()
                                    .border_color(theme::border())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::card_hover()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.load_profile_into_inputs(&profile_id, window, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .items_center()
                                            .child(
                                                div()
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
                                                    .gap(px(theme::SPACE_FINE))
                                                    .child(
                                                        div()
                                                            .text_size(px(theme::TYPE_BODY_SIZE))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(profile.display_name()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                            .text_color(theme::text_muted())
                                                            .child(format!(
                                                                "{}@{}",
                                                                profile.username, profile.host
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(self.status_badge(
                                        "password",
                                        theme::library_bg(),
                                        theme::text_muted(),
                                    ))
                                    .child(self.status_badge(
                                        vault_label,
                                        theme::library_bg(),
                                        theme::accent(),
                                    ))
                                    .into_any_element()
                            }),
                    )
                    .when(profiles_with_password.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::User)
                                    .size(px(theme::ICON_SIZE_LARGE))
                                    .text_color(theme::accent()),
                                library_copy(MessageId::KeychainPasswordEmptyTitle),
                                library_copy(MessageId::KeychainPasswordEmptyDescription),
                            )
                            .child(
                                h_flex().gap_2().justify_center().child(
                                    Button::new("password-identities-open-hosts")
                                        .debug_selector(|| {
                                            "password-identities-open-hosts".to_string()
                                        })
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label(library_copy(MessageId::KeychainNewHostAction))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_editor_for_new_host(window, cx);
                                        })),
                                ),
                            ),
                        )
                    }),
            )
    }

    pub(super) fn render_keychain_view(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex().justify_between().items_center().child(
                    div()
                        .text_size(px(theme::TYPE_HEADING_SIZE))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(library_copy(MessageId::KeychainKeysTitle)),
                ),
            )
            .child(self.keychain_tab_control(cx))
            .child(match self.keychain_tab {
                KeychainTab::Keys => self.render_keychain_keys(cx),
                KeychainTab::Identities => self.render_keychain_identities(cx),
            })
    }

    pub(super) fn render_vaults_view(&self, cx: &Context<Self>) -> Div {
        let vaults = self.saved.vaults.clone();
        let selected_vault = self
            .selected_vault_id
            .as_deref()
            .and_then(|vault_id| self.vault_by_id(vault_id))
            .cloned()
            .or_else(|| self.default_vault().cloned());

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(library_copy(MessageId::VaultsTitle)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{} {}",
                                vaults.len(),
                                if vaults.len() == 1 { "vault" } else { "vaults" }
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SIZE))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child(library_copy(MessageId::VaultsDescription)),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field(
                        library_copy(MessageId::VaultNameField),
                        Input::new(&self.vault_inputs.label),
                    ))
                    .child(self.form_field(
                        library_copy(MessageId::VaultDescriptionField),
                        Input::new(&self.vault_inputs.description),
                    ))
                    .when_some(selected_vault.as_ref(), |this, vault| {
                        let vault = vault.clone();
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_BODY_SIZE))
                                                .font_medium()
                                                .text_color(theme::text_main())
                                                .child(library_copy(MessageId::VaultMembersTitle)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::text_muted())
                                                .child(format!(
                                                    "{} {}",
                                                    vault.members.len(),
                                                    if vault.members.len() == 1 {
                                                        "member"
                                                    } else {
                                                        "members"
                                                    }
                                                )),
                                        ),
                                )
                                .when(vault.is_personal(), |this| {
                                    this.child(
                                        div()
                                            .p_3()
                                            .rounded(px(theme::CARD_RADIUS))
                                            .bg(theme::with_alpha(theme::hover(), 0.72))
                                            .border_1()
                                            .border_color(theme::border())
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(library_copy(MessageId::VaultPersonalNotice)),
                                    )
                                })
                                .when(!vault.is_personal(), |this| {
                                    this.child(self.form_field(
                                        library_copy(MessageId::VaultMemberNameField),
                                        Input::new(&self.vault_member_inputs.name),
                                    ))
                                    .child(self.form_field(
                                        library_copy(MessageId::VaultMemberEmailField),
                                        Input::new(&self.vault_member_inputs.email),
                                    ))
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                                    .font_medium()
                                                    .text_color(theme::text_main())
                                                    .child(library_copy(
                                                        MessageId::VaultMemberRoleField,
                                                    )),
                                            )
                                            .child(
                                                h_flex().gap_2().children(
                                                    [
                                                        VaultMemberRole::Owner,
                                                        VaultMemberRole::Editor,
                                                        VaultMemberRole::Viewer,
                                                    ]
                                                    .into_iter()
                                                    .enumerate()
                                                    .map(|(index, role)| {
                                                        let selected =
                                                            self.draft_vault_member_role == role;
                                                        div()
                                                            .id(("vault-member-role", index))
                                                            .debug_selector(move || {
                                                                format!("vault-member-role-{index}")
                                                            })
                                                            .px_3()
                                                            .py(px(
                                                                theme::SENSITIVE_MEMBER_PADDING_Y,
                                                            ))
                                                            .rounded(px(theme::PILL_RADIUS))
                                                            .bg(if selected {
                                                                theme::accent_soft()
                                                            } else {
                                                                theme::with_alpha(
                                                                    theme::hover(),
                                                                    0.72,
                                                                )
                                                            })
                                                            .border_1()
                                                            .border_color(if selected {
                                                                theme::with_alpha(
                                                                    theme::accent(),
                                                                    0.42,
                                                                )
                                                            } else {
                                                                theme::border()
                                                            })
                                                            .cursor_pointer()
                                                            .hover(|style| style.bg(theme::hover()))
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.draft_vault_member_role =
                                                                        role;
                                                                    this.error_message.clear();
                                                                    cx.notify();
                                                                },
                                                            ))
                                                            .child(
                                                                div()
                                                                    .text_size(px(
                                                                        theme::TYPE_BODY_SMALL_SIZE,
                                                                    ))
                                                                    .font_medium()
                                                                    .text_color(if selected {
                                                                        theme::text_main()
                                                                    } else {
                                                                        theme::text_muted()
                                                                    })
                                                                    .child(role.label()),
                                                            )
                                                            .into_any_element()
                                                    }),
                                                ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("vault-member-clear")
                                                            .debug_selector(|| {
                                                                "vault-member-clear".to_string()
                                                            })
                                                            .small()
                                                            .custom(Self::action_button_style(
                                                                theme::ActionTone::Neutral,
                                                                cx,
                                                            ))
                                                            .label(library_copy(
                                                                MessageId::VaultMemberClearAction,
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.clear_vault_member_form(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("vault-member-save")
                                                            .debug_selector(|| {
                                                                "vault-member-save".to_string()
                                                            })
                                                            .small()
                                                            .custom(Self::action_button_style(
                                                                theme::ActionTone::Accent,
                                                                cx,
                                                            ))
                                                            .label(library_copy(
                                                                MessageId::VaultMemberSaveAction,
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.save_vault_member(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                })
                                .child(v_flex().gap_2().children(
                                    vault.members.iter().enumerate().map(|(index, member)| {
                                        let member_id = member.id.clone();
                                        let remove_id = member.id.clone();
                                        let selected = self.selected_vault_member_id.as_deref()
                                            == Some(member.id.as_str());

                                        h_flex()
                                            .id(("vault-member-card", index))
                                            .debug_selector(move || {
                                                format!("vault-member-card-{index}")
                                            })
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .p_3()
                                            .rounded(px(theme::CARD_RADIUS))
                                            .bg(if selected {
                                                theme::with_alpha(theme::accent(), 0.1)
                                            } else {
                                                theme::with_alpha(theme::hover(), 0.72)
                                            })
                                            .border_1()
                                            .border_color(if selected {
                                                theme::with_alpha(theme::accent(), 0.42)
                                            } else {
                                                theme::border()
                                            })
                                            .cursor_pointer()
                                            .hover(|style| style.bg(theme::hover()))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.load_vault_member_into_inputs(
                                                    &member_id, window, cx,
                                                );
                                            }))
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .gap(px(theme::SPACE_MICRO))
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .text_size(px(
                                                                        theme::TYPE_BODY_SMALL_SIZE,
                                                                    ))
                                                                    .font_semibold()
                                                                    .text_color(theme::text_main())
                                                                    .child(member.display_name()),
                                                            )
                                                            .child(self.status_badge(
                                                                member.role.label(),
                                                                theme::library_bg(),
                                                                if member.role
                                                                    == VaultMemberRole::Owner
                                                                {
                                                                    theme::accent()
                                                                } else if member.role
                                                                    == VaultMemberRole::Editor
                                                                {
                                                                    theme::success()
                                                                } else {
                                                                    theme::slate()
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                            .text_color(theme::text_muted())
                                                            .child(member.email.clone()),
                                                    ),
                                            )
                                            .when(!vault.is_personal(), |this| {
                                                this.child(
                                                    Button::new(("vault-member-remove", index))
                                                        .debug_selector(move || {
                                                            format!("vault-member-remove-{index}")
                                                        })
                                                        .small()
                                                        .ghost()
                                                        .icon(IconName::Delete)
                                                        .label(library_copy(
                                                            MessageId::VaultMemberDeleteAction,
                                                        ))
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                this.remove_vault_member(
                                                                    &remove_id, window, cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                            })
                                            .into_any_element()
                                    }),
                                )),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("vault-new")
                                    .debug_selector(|| "vault-new".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label(library_copy(MessageId::VaultNewAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_vault_form(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("vault-save")
                                    .debug_selector(|| "vault-save".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label(localization::common_save())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_vault(window, cx);
                                    })),
                            )
                            .when(
                                self.selected_vault_id
                                    .as_deref()
                                    .is_some_and(|vault_id| vault_id != DEFAULT_VAULT_ID),
                                |this| {
                                    this.child(
                                        Button::new("vault-delete")
                                            .debug_selector(|| "vault-delete".to_string())
                                            .small()
                                            .ghost()
                                            .icon(IconName::Delete)
                                            .label(localization::common_delete())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.remove_selected_vault(window, cx);
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(vaults.iter().enumerate().map(|(index, vault)| {
                        let vault_id = vault.id.clone();
                        let selected = self.selected_vault_id.as_deref() == Some(vault.id.as_str());
                        let (host_count, identity_count, snippet_count) =
                            self.vault_item_counts(&vault.id);
                        let member_count = vault.members.len();

                        h_flex()
                            .id(("vault-card", index))
                            .debug_selector(move || format!("vault-card-{index}"))
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(if selected {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::card_hover_subtle()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_vault_into_inputs(&vault_id, window, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(theme::SPACE_FINE))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_BODY_SIZE))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(vault.display_name()),
                                            )
                                            .child(self.status_badge(
                                                vault.kind.label(),
                                                theme::library_bg(),
                                                if vault.kind == VaultKind::Personal {
                                                    theme::accent()
                                                } else {
                                                    theme::slate()
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(if vault.description.trim().is_empty() {
                                                library_copy(MessageId::VaultNoDescription)
                                            } else {
                                                vault.description.clone()
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(self.status_badge(
                                                localization::vault_host_count(host_count),
                                                theme::library_bg(),
                                                theme::success(),
                                            ))
                                            .child(self.status_badge(
                                                localization::vault_key_count(identity_count),
                                                theme::library_bg(),
                                                theme::accent(),
                                            ))
                                            .child(self.status_badge(
                                                localization::vault_snippet_count(snippet_count),
                                                theme::library_bg(),
                                                theme::warning(),
                                            ))
                                            .child(self.status_badge(
                                                localization::vault_member_count(member_count),
                                                theme::library_bg(),
                                                theme::slate(),
                                            )),
                                    ),
                            )
                            .into_any_element()
                    })),
            )
    }

    // termirust-ui-surface:vault-keys-snippets:end
    pub(super) fn render_known_hosts_view(&self, cx: &Context<Self>) -> Div {
        let entries = self.known_hosts.entries().unwrap_or_default();

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::known_hosts_title()),
                    )
                    .when(!entries.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::known_hosts_count(entries.len())),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child(localization::known_hosts_description()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(entries.iter().enumerate().map(|(index, (endpoint, key))| {
                        let remove_endpoint = endpoint.clone();
                        h_flex()
                            .id(("snippet-card", index))
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                app_icon(ICON_SHIELD_CHECK)
                                                    .size(px(14.))
                                                    .text_color(theme::success()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(endpoint.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(short_host_key(key)),
                                    ),
                            )
                            .child(
                                Button::new(("remove-known-host", index))
                                    .debug_selector(move || {
                                        format!("remove-known-host-{index}")
                                    })
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        match this.known_hosts.remove(&remove_endpoint) {
                                            Ok(true) => {
                                                this.status_message =
                                                    localization::known_hosts_removed_status(
                                                        remove_endpoint.clone(),
                                                    );
                                                this.error_message.clear();
                                            }
                                            Ok(false) => {
                                                this.status_message = localization::
                                                    known_hosts_already_removed_status();
                                            }
                                            Err(e) => {
                                                this.error_message = e.to_string();
                                            }
                                        }
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    }))
                    .when(entries.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                app_icon(ICON_SHIELD_CHECK)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No hosts pinned yet",
                                "Trust records appear here after the first successful SSH connection to a host.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("known-hosts-open-hosts")
                                            .debug_selector(|| "known-hosts-open-hosts".to_string())
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .label(localization::open_connections_action())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.nav_section = NavSection::Hosts;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    pub(super) fn render_logs_view(&self, _cx: &Context<Self>) -> Div {
        let logs: Vec<&SessionLogEntry> = self.saved.session_logs.iter().rev().collect();

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::session_history_title()),
                    )
                    .when(!logs.is_empty(), |this| {
                        this.child(
                            h_flex().gap_2().items_center().child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::session_history_count(logs.len())),
                            ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(self.panes.iter().filter(|p| p.connected).map(|pane| {
                        h_flex()
                            .justify_between()
                            .items_center()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::with_alpha(theme::success(), 0.3))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div().size(px(10.)).rounded(px(999.)).bg(theme::success()),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(pane.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .text_color(theme::text_muted())
                                                    .child(pane.endpoint.clone()),
                                            ),
                                    ),
                            )
                            .child(self.status_badge(
                                localization::session_history_active_status(),
                                theme::library_bg(),
                                theme::success(),
                            ))
                            .into_any_element()
                    }))
                    .children(logs.iter().map(|entry| {
                        let (status_color, status_label) = match entry.status {
                            SessionLogStatus::Connected => (theme::success(), "Connected"),
                            SessionLogStatus::Connecting => (theme::accent(), "Connecting"),
                            SessionLogStatus::Disconnected => (theme::text_muted(), "Closed"),
                            SessionLogStatus::Error => (theme::danger(), "Error"),
                        };

                        h_flex()
                            .justify_between()
                            .items_center()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div()
                                            .size(px(10.))
                                            .rounded(px(999.))
                                            .bg(theme::with_alpha(status_color, 0.5)),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(14.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(entry.title.clone()),
                                                    )
                                                    .child(self.status_badge(
                                                        status_label,
                                                        theme::library_bg(),
                                                        status_color,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .text_color(theme::text_muted())
                                                    .child(format!(
                                                        "{}  {}@{}",
                                                        entry.endpoint(),
                                                        entry.username,
                                                        entry.host,
                                                    )),
                                            )
                                            .child(
                                                h_flex().gap_2().child(
                                                    div()
                                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                        .text_color(theme::text_muted())
                                                        .child(localization::session_history_started_duration(
                                                            entry.started_display(),
                                                            entry.duration_display(),
                                                        )),
                                                ),
                                            )
                                            .when_some(
                                                entry.error_message.as_ref(),
                                                |this, msg| {
                                                    this.child(
                                                        div()
                                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                            .text_color(theme::danger())
                                                            .child(msg.clone()),
                                                    )
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(entry.duration_display()),
                            )
                            .into_any_element()
                    }))
                    .when(logs.is_empty() && self.panes.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::BookOpen)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No session history yet",
                                "Connection history appears here after you open your first SSH workspace.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("logs-open-hosts")
                                            .debug_selector(|| "logs-open-hosts".to_string())
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                _cx,
                                            ))
                                            .label(localization::open_connections_action())
                                            .on_click(_cx.listener(|this, _, _, cx| {
                                                this.nav_section = NavSection::Hosts;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    // termirust-ui-surface:vault-keys-snippets:start
    pub(super) fn render_snippets_view(&self, _cx: &Context<Self>) -> Div {
        let snippets = self.saved.snippets.clone();

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(library_copy(MessageId::SnippetsTitle)),
                    )
                    .when(!snippets.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::snippet_count(snippets.len())),
                        )
                    }),
            )
            .child(
                div()
                    .max_w(px(theme::SENSITIVE_FORM_MAX_WIDTH))
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child(library_copy(MessageId::SnippetLibraryDescription)),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field(
                        library_copy(MessageId::SnippetLabelField),
                        Input::new(&self.snippet_inputs.label),
                    ))
                    .child(self.render_vault_picker(
                        self.snippet_vault_id.as_deref(),
                        |vault_id, this, _, cx| {
                            this.snippet_vault_id = Some(vault_id.clone());
                            this.selected_vault_id = Some(vault_id.clone());
                            this.status_message = localization::snippet_assigned_vault(
                                this.effective_vault_name(Some(&vault_id)),
                            );
                            this.error_message.clear();
                            cx.notify();
                        },
                        _cx,
                    ))
                    .child(self.form_field(
                        library_copy(MessageId::SnippetGroupField),
                        Input::new(&self.snippet_inputs.group),
                    ))
                    .child(self.form_field(
                        library_copy(MessageId::SnippetCommandField),
                        Input::new(&self.snippet_inputs.command),
                    ))
                    .child(
                        h_flex()
                            .p(px(theme::SPACE_MICRO))
                            .rounded(px(theme::CONTROL_RADIUS))
                            .bg(theme::hover())
                            .children([true, false].into_iter().enumerate().map(
                                |(index, pinned)| {
                                    let active = self.snippet_pinned == pinned;
                                    Button::new(("snippet-pin-toggle", index))
                                        .debug_selector(move || {
                                            format!("snippet-pin-toggle-{index}")
                                        })
                                        .small()
                                        .custom(Self::segmented_button_style(active, _cx))
                                        .label(if pinned {
                                            library_copy(MessageId::SnippetPinnedLabel)
                                        } else {
                                            library_copy(MessageId::SnippetLibraryLabel)
                                        })
                                        .on_click(_cx.listener(move |this, _, _, cx| {
                                            this.toggle_snippet_pinned(pinned, cx);
                                        }))
                                        .into_any_element()
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("snippet-new")
                                    .debug_selector(|| "snippet-new".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        _cx,
                                    ))
                                    .label(library_copy(MessageId::SnippetNewAction))
                                    .on_click(_cx.listener(|this, _, window, cx| {
                                        this.clear_snippet_form(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("snippet-save")
                                    .debug_selector(|| "snippet-save".to_string())
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        _cx,
                                    ))
                                    .label(localization::common_save())
                                    .on_click(_cx.listener(|this, _, window, cx| {
                                        this.save_snippet(window, cx);
                                    })),
                            )
                            .when(self.selected_snippet_id.is_some(), |this| {
                                this.child(
                                    Button::new("snippet-delete")
                                        .debug_selector(|| "snippet-delete".to_string())
                                        .small()
                                        .ghost()
                                        .icon(IconName::Delete)
                                        .label(localization::common_delete())
                                        .on_click(_cx.listener(|this, _, window, cx| {
                                            this.remove_selected_snippet(window, cx);
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scrollbar()
                    .children(snippets.iter().enumerate().map(|(index, snippet)| {
                        let snippet_id = snippet.id.clone();
                        let insert_snippet_id = snippet.id.clone();
                        let group_label = snippet.group.trim().to_string();
                        let vault_label = self.effective_vault_name(snippet.vault_id.as_deref());
                        let toggle_snippet_id = snippet.id.clone();
                        let toggle_pinned = !snippet.pinned;

                        h_flex()
                            .id(("snippet-card", index))
                            .debug_selector(move || format!("snippet-card-{index}"))
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(
                                if self.selected_snippet_id.as_deref() == Some(snippet.id.as_str())
                                {
                                    theme::accent()
                                } else {
                                    theme::border()
                                },
                            )
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::card_hover_subtle()))
                            .on_click(_cx.listener(move |this, _, window, cx| {
                                this.load_snippet_into_inputs(&snippet_id, window, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(theme::SPACE_FINE))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_BODY_SIZE))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(snippet.display_name()),
                                            )
                                            .when(!group_label.is_empty(), |this| {
                                                this.child(self.status_badge(
                                                    group_label.clone(),
                                                    theme::library_bg(),
                                                    theme::slate(),
                                                ))
                                            })
                                            .when(snippet.pinned, |this| {
                                                this.child(self.status_badge(
                                                    library_copy(MessageId::SnippetPinnedLabel),
                                                    theme::library_bg(),
                                                    theme::warning(),
                                                ))
                                            })
                                            .child(self.status_badge(
                                                vault_label,
                                                theme::library_bg(),
                                                theme::accent(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(snippet.command.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("snippet-pin", index))
                                            .debug_selector(move || format!("snippet-pin-{index}"))
                                            .small()
                                            .custom(Self::action_button_style(
                                                if snippet.pinned {
                                                    theme::ActionTone::AccentSoft
                                                } else {
                                                    theme::ActionTone::Neutral
                                                },
                                                _cx,
                                            ))
                                            .label(if snippet.pinned {
                                                library_copy(MessageId::SnippetUnpinActionLabel)
                                            } else {
                                                library_copy(MessageId::SnippetPinActionLabel)
                                            })
                                            .on_click(_cx.listener(move |this, _, window, cx| {
                                                this.set_saved_snippet_pinned(
                                                    &toggle_snippet_id,
                                                    toggle_pinned,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("snippet-insert", index))
                                            .debug_selector(move || {
                                                format!("snippet-insert-{index}")
                                            })
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Success,
                                                _cx,
                                            ))
                                            .label(localization::snippet_insert_action())
                                            .on_click(_cx.listener(move |this, _, window, cx| {
                                                this.insert_saved_snippet(
                                                    &insert_snippet_id,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                            .into_any_element()
                    }))
                    .when(snippets.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::BookOpen)
                                    .size(px(theme::ICON_SIZE_LARGE))
                                    .text_color(theme::accent()),
                                library_copy(MessageId::SnippetEmptyTitle),
                                library_copy(MessageId::SnippetEmptyDescription),
                            )
                            .child(
                                h_flex().gap_2().justify_center().child(
                                    Button::new("snippets-empty-new")
                                        .debug_selector(|| "snippets-empty-new".to_string())
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            _cx,
                                        ))
                                        .label(library_copy(MessageId::SnippetNewAction))
                                        .on_click(_cx.listener(|this, _, window, cx| {
                                            this.clear_snippet_form(window, cx);
                                        })),
                                ),
                            ),
                        )
                    }),
            )
    }

    // termirust-ui-surface:vault-keys-snippets:end
    // termirust-ui-surface:settings:start
    pub(super) fn settings_section_card<E: IntoElement>(
        &self,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        body: E,
    ) -> Div {
        let title: SharedString = title.into();
        let description: SharedString = description.into();
        v_flex()
            .w_full()
            .gap(px(theme::SPACE_5))
            .px(px(theme::SPACE_6))
            .py(px(theme::SPACE_5))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow_sm()
            .child(
                v_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .line_height(relative(1.5))
                            .text_color(theme::text_muted())
                            .child(description),
                    ),
            )
            .child(body)
    }

    fn settings_subhead(
        &self,
        title: impl Into<SharedString>,
        hint: impl Into<SharedString>,
    ) -> Div {
        let title: SharedString = title.into();
        let hint: SharedString = hint.into();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SIZE))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
    }

    fn settings_hierarchy_heading(&self, section: SettingsSectionId) -> Div {
        v_flex()
            .gap(px(theme::SPACE_FINE))
            .pt(px(theme::SPACE_2))
            .child(
                div()
                    .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(library_copy(section.title())),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_muted())
                    .child(library_copy(section.description())),
            )
    }

    pub(super) fn settings_divider(&self) -> Div {
        div()
            .h(px(theme::BORDER_HAIRLINE))
            .w_full()
            .bg(theme::with_alpha(theme::border(), 0.6))
    }

    fn render_mobile_devices_settings(&self, cx: &Context<Self>) -> Div {
        let devices = self.saved.settings.mobile_devices.clone();
        v_flex()
            .gap_2()
            .child(self.settings_subhead(
                library_copy(MessageId::SettingsMobileDevicesTitle),
                library_copy(MessageId::SettingsMobileDevicesDescription),
            ))
            .when(devices.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(library_copy(MessageId::SettingsMobileDevicesEmpty)),
                )
            })
            .children(devices.into_iter().enumerate().map(|(index, device)| {
                let device_id = device.device_id.clone();
                let platform = device
                    .platform
                    .unwrap_or_else(|| library_copy(MessageId::SettingsMobilePlatformFallback));
                let status = if device.revoked_at_millis.is_some() {
                    library_copy(MessageId::SettingsMobileStatusRevoked)
                } else if device.last_seen_at_millis.is_some() {
                    library_copy(MessageId::SettingsMobileStatusSeen)
                } else {
                    library_copy(MessageId::SettingsMobileStatusApproved)
                };
                let revoked = device.revoked_at_millis.is_some();
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .p_3()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .font_medium()
                                            .text_color(theme::text_main())
                                            .child(device.label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(platform),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(device_id.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(status),
                                    ),
                            ),
                    )
                    .when(!revoked, |this| {
                        let revoke_id = device_id.clone();
                        this.child(
                            Button::new(("settings-revoke-mobile-device", index))
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .label(library_copy(MessageId::SettingsMobileRevokeAction))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.revoke_mobile_device(&revoke_id, cx);
                                })),
                        )
                    })
            }))
    }

    fn settings_shortcut_row(
        &self,
        keys: &'static str,
        description: impl Into<SharedString>,
    ) -> Div {
        let description = description.into();
        h_flex()
            .justify_between()
            .items_center()
            .gap_3()
            .py(px(theme::SPACE_2))
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(description),
            )
            .child(
                div()
                    .px_2()
                    .py(px(theme::SPACE_1))
                    .rounded(px(theme::CONTROL_RADIUS))
                    .bg(theme::with_alpha(theme::hover(), 0.85))
                    .border_1()
                    .border_color(theme::border())
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .font_medium()
                    .text_color(theme::text_muted())
                    .child(platform_shortcut_label(keys)),
            )
    }

    fn settings_shortcut_group<const N: usize, D>(
        &self,
        title: impl Into<SharedString>,
        rows: [(&'static str, D); N],
    ) -> Div
    where
        D: Into<SharedString>,
    {
        let title = title.into();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .children(
                rows.into_iter()
                    .map(|(keys, desc)| self.settings_shortcut_row(keys, desc).into_any_element()),
            )
    }

    pub(super) fn render_settings_view(&self, cx: &Context<Self>) -> Div {
        let settings_snapshot = self
            .settings_semantic_snapshot(cx)
            .expect("Settings snapshot is available while rendering Settings");
        let query_active = settings_snapshot.query_active;
        let result_count = settings_snapshot.search_results.len();
        let section_visible = |section: SettingsSectionId| {
            !query_active
                || settings_snapshot
                    .search_results
                    .iter()
                    .any(|setting| setting.section() == section)
        };
        let appearance_visible = section_visible(SettingsSectionId::Appearance);
        let terminal_visible = section_visible(SettingsSectionId::Terminal);
        let projects_sessions_visible = section_visible(SettingsSectionId::ProjectsSessions);
        let presets_runtimes_visible = section_visible(SettingsSectionId::PresetsRuntimes);
        let notifications_visible = section_visible(SettingsSectionId::Notifications);
        let keyboard_visible = section_visible(SettingsSectionId::Keyboard);
        let storage_visible = section_visible(SettingsSectionId::StoragePrivacyDiagnostics);
        let remote_devices_visible = section_visible(SettingsSectionId::RemoteDevices);
        let theme_preset = self.saved.settings.theme_preset;
        let terminal_font_size = self.saved.settings.terminal_font_size;
        let restore_workspaces_on_launch = self.saved.settings.restore_workspaces_on_launch;
        let session_log_limit = self.saved.settings.session_log_limit;
        let onboarding_dismissed = self.saved.settings.onboarding_dismissed;
        let auto_reconnect_attempts = self.saved.settings.auto_reconnect_attempts;
        let auto_reconnect_delay_secs = self.saved.settings.auto_reconnect_delay_secs;
        let ssh_keepalive_secs = self.saved.settings.ssh_keepalive_secs;
        let copy_on_select = self.saved.settings.copy_on_select;
        let confirm_multiline_paste = self.saved.settings.confirm_multiline_paste;
        let session_log_count = self.saved.session_logs.len();
        let has_default_ssh_dir = self.saved.settings.default_ssh_startup_directory.is_some();
        let diagnostics_enabled = self.saved.settings.diagnostics_enabled;
        let diagnostics_file_limit = self.saved.settings.diagnostics_max_file_mib;
        let diagnostics_retention = self.saved.settings.diagnostics_retention_days;
        let diagnostics_busy = self.diagnostic_operation.is_some();
        let diagnostics_usage = crate::diagnostics::usage().unwrap_or_default();
        let diagnostics_model = crate::ui::settings::diagnostics_view_model(
            diagnostics_enabled,
            crate::diagnostics::status(),
            diagnostics_usage,
            diagnostics_retention,
            self.diagnostic_preview.is_some(),
            diagnostics_busy,
        );
        let diagnostics_preview_summary = self.diagnostic_preview.as_ref().map(|preview| {
            let manifest = preview.manifest();
            localization::diagnostics_preview_summary(
                manifest.total_entries,
                manifest.total_bytes,
                manifest.redactions,
            )
        });
        let health_busy =
            self.health_operation.is_some() || self.metadata_recovery_operation.is_some();
        let health_model =
            crate::ui::settings::health_view_model(self.health_report.as_ref(), health_busy);
        let recovery_model = crate::ui::settings::recovery_view_model(
            self.health_report.as_ref(),
            self.metadata_recovery_plan.as_ref(),
            self.metadata_recovery_operation.is_some(),
        );

        let appearance_card = self.settings_section_card(
            library_copy(MessageId::SettingsThemeLabel),
            library_copy(MessageId::SettingsThemeDescription),
            v_flex()
                .gap_3()
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [ThemePreset::Ocean, ThemePreset::Daylight]
                            .into_iter()
                            .enumerate()
                            .map(|(index, preset)| {
                                let selected = preset == theme_preset;
                                div()
                                    .id(("settings-theme", index))
                                    .debug_selector(move || format!("settings-theme-{index}"))
                                    .px_3()
                                    .py(px(theme::SPACE_3))
                                    .rounded(px(theme::PILL_RADIUS))
                                    .bg(if selected {
                                        theme::accent_soft()
                                    } else {
                                        theme::with_alpha(theme::hover(), 0.72)
                                    })
                                    .border_1()
                                    .border_color(if selected {
                                        theme::with_alpha(theme::accent(), 0.42)
                                    } else {
                                        theme::border()
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_theme_preset(preset, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .font_medium()
                                            .text_color(if selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(library_copy(match preset {
                                                ThemePreset::Daylight => {
                                                    MessageId::SettingsThemeDaylight
                                                }
                                                _ => MessageId::SettingsThemeOcean,
                                            })),
                                    )
                                    .into_any_element()
                            }),
                    ),
                )
                .child(
                    h_flex().gap_3().flex_wrap().children(
                        [
                            (
                                MessageId::SettingsPreviewLibrary,
                                MessageId::SettingsPreviewLibraryDescription,
                                theme::library_card(),
                                theme::text_main(),
                            ),
                            (
                                MessageId::SettingsPreviewChrome,
                                MessageId::SettingsPreviewChromeDescription,
                                theme::chrome_bg(),
                                theme::text_on_dark(),
                            ),
                            (
                                MessageId::SettingsPreviewTerminal,
                                MessageId::SettingsPreviewTerminalDescription,
                                theme::terminal_bg(),
                                theme::text_on_dark(),
                            ),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, description, bg, fg))| {
                            v_flex()
                                .id(("settings-preview", index))
                                .w(px(theme::SETTINGS_THEME_PREVIEW_WIDTH))
                                .gap_1()
                                .p_3()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(bg)
                                .border_1()
                                .border_color(theme::with_alpha(fg, 0.18))
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_semibold()
                                        .text_color(fg)
                                        .child(library_copy(label)),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::with_alpha(fg, 0.78))
                                        .child(library_copy(description)),
                                )
                                .into_any_element()
                        }),
                    ),
                ),
        );

        let localization_card = localization::development_controls_enabled().then(|| {
            self.settings_section_card(
                localization::development_localization_title(),
                localization::development_localization_hint(),
                h_flex().gap_2().flex_wrap().children(
                    localization::development_locales()
                        .into_iter()
                        .enumerate()
                        .map(|(index, locale)| {
                            let selected = locale == localization::current_locale();
                            Button::new(("settings-development-locale", index))
                                .small()
                                .custom(Self::action_button_style(
                                    if selected {
                                        theme::ActionTone::AccentSoft
                                    } else {
                                        theme::ActionTone::Neutral
                                    },
                                    cx,
                                ))
                                .label(locale.tag())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if localization::set_development_locale(locale.tag()).is_ok() {
                                        this.status_message =
                                            localization::development_locale_active(locale);
                                        this.error_message.clear();
                                        cx.notify();
                                    }
                                }))
                                .into_any_element()
                        }),
                ),
            )
        });

        let terminal_card = self.settings_section_card(
            library_copy(MessageId::SettingsSectionTerminal),
            library_copy(MessageId::SettingsSectionTerminalDescription),
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsTerminalFontSizeLabel),
                    library_copy(MessageId::SettingsTerminalFontSizeDescription),
                ))
                .child(h_flex().gap_2().flex_wrap().children(
                    [12u16, 13, 14, 15, 16, 18].into_iter().enumerate().map(
                        |(index, font_size)| {
                            let selected = font_size == terminal_font_size;
                            div()
                                .id(("settings-font-size", index))
                                .debug_selector(move || format!("settings-font-size-{index}"))
                                .px_3()
                                .py(px(theme::SPACE_3))
                                .rounded(px(theme::PILL_RADIUS))
                                .bg(if selected {
                                    theme::accent_soft()
                                } else {
                                    theme::with_alpha(theme::hover(), 0.72)
                                })
                                .border_1()
                                .border_color(if selected {
                                    theme::with_alpha(theme::accent(), 0.42)
                                } else {
                                    theme::border()
                                })
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::hover()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.update_terminal_font_size(font_size, window, cx);
                                }))
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_medium()
                                        .text_color(if selected {
                                            theme::text_main()
                                        } else {
                                            theme::text_muted()
                                        })
                                        .child(localization::settings_font_size_option(font_size)),
                                )
                                .into_any_element()
                        },
                    ),
                ))
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsCopyOnSelectLabel),
                    library_copy(MessageId::SettingsCopyOnSelectDescription),
                ))
                .child(
                    h_flex()
                        .p(px(theme::SPACE_MICRO))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::hover())
                        .children(
                            [true, false]
                                .into_iter()
                                .enumerate()
                                .map(|(index, enabled)| {
                                    let active = enabled == copy_on_select;
                                    Button::new(("settings-copy-on-select", index))
                                        .small()
                                        .custom(Self::segmented_button_style(active, cx))
                                        .label(library_copy(if enabled {
                                            MessageId::SettingsAutoCopyValue
                                        } else {
                                            MessageId::SettingsManualCopyValue
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_copy_on_select(enabled, cx);
                                        }))
                                        .into_any_element()
                                }),
                        ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsConfirmMultilinePasteLabel),
                    library_copy(MessageId::SettingsConfirmMultilinePasteDescription),
                ))
                .child(
                    h_flex()
                        .p(px(theme::SPACE_MICRO))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::hover())
                        .children(
                            [true, false]
                                .into_iter()
                                .enumerate()
                                .map(|(index, enabled)| {
                                    let active = enabled == confirm_multiline_paste;
                                    Button::new(("settings-confirm-paste", index))
                                        .small()
                                        .custom(Self::segmented_button_style(active, cx))
                                        .label(library_copy(if enabled {
                                            MessageId::SettingsConfirmPasteValue
                                        } else {
                                            MessageId::SettingsDirectPasteValue
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_confirm_multiline_paste(enabled, cx);
                                        }))
                                        .into_any_element()
                                }),
                        ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsTerminalFontFamilyLabel),
                    library_copy(MessageId::SettingsTerminalFontFamilyDescription),
                ))
                .child(self.form_field(
                    library_copy(MessageId::SettingsTerminalFontFamilyLabel),
                    Input::new(&self.settings_inputs.terminal_font_family),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-terminal-font-family-save")
                                .debug_selector(|| "settings-terminal-font-family-save".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(library_copy(MessageId::SettingsSaveFontFamilyAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_terminal_font_family(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-terminal-font-family-reset")
                                .debug_selector(|| {
                                    "settings-terminal-font-family-reset".to_string()
                                })
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsResetAction))
                                .disabled(self.saved.settings.terminal_font_family.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_terminal_font_family(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsFontFamilyInstallHint)),
                        ),
                ),
        );

        let startup_card = self.settings_section_card(
            library_copy(MessageId::SettingsStartupTitle),
            library_copy(MessageId::SettingsStartupDescription),
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .p(px(theme::SPACE_MICRO))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::hover())
                        .children(
                            [true, false]
                                .into_iter()
                                .enumerate()
                                .map(|(index, restore)| {
                                    let active = restore == restore_workspaces_on_launch;
                                    Button::new(("settings-restore-workspaces", index))
                                        .debug_selector(move || {
                                            format!("settings-restore-workspaces-{index}")
                                        })
                                        .small()
                                        .custom(Self::segmented_button_style(active, cx))
                                        .label(library_copy(if restore {
                                            MessageId::SettingsRestoreValue
                                        } else {
                                            MessageId::SettingsLibraryValue
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_restore_workspaces_on_launch(restore, cx);
                                        }))
                                        .into_any_element()
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-reset-onboarding")
                                .debug_selector(|| "settings-reset-onboarding".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(if onboarding_dismissed {
                                    MessageId::SettingsShowWelcomeAction
                                } else {
                                    MessageId::SettingsWelcomeVisible
                                }))
                                .disabled(!onboarding_dismissed)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reset_onboarding_panel(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(if onboarding_dismissed {
                                    MessageId::SettingsOnboardingDismissedHint
                                } else {
                                    MessageId::SettingsOnboardingVisibleHint
                                })),
                        ),
                ),
        );

        let sessions_card = self.settings_section_card(
            library_copy(MessageId::SettingsSessionsTitle),
            library_copy(MessageId::SettingsSessionsDescription),
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsSessionHistoryLimitLabel),
                    library_copy(MessageId::SettingsSessionHistoryLimitDescription),
                ))
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [100u16, 200, 500, 1000]
                            .into_iter()
                            .enumerate()
                            .map(|(index, limit)| {
                                let selected = limit == session_log_limit;
                                Button::new(("settings-session-log-limit", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(localization::settings_history_option(limit))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_session_log_limit(limit, cx);
                                    }))
                                    .into_any_element()
                            }),
                    ),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::settings_history_current(session_log_count)),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsDefaultSshDirectoryLabel),
                    library_copy(MessageId::SettingsDefaultSshDirectoryDescription),
                ))
                .child(self.form_field(
                    library_copy(MessageId::SettingsDefaultSshDirectoryLabel),
                    Input::new(&self.settings_inputs.default_ssh_startup_directory),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-default-ssh-dir-save")
                                .debug_selector(|| "settings-default-ssh-dir-save".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(library_copy(MessageId::SettingsSaveDefaultDirectoryAction))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_default_ssh_startup_directory(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-default-ssh-dir-clear")
                                .debug_selector(|| "settings-default-ssh-dir-clear".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsClearAction))
                                .disabled(!has_default_ssh_dir)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_default_ssh_startup_directory(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(
                                    MessageId::SettingsDefaultDirectoryPriorityHint,
                                )),
                        ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsAutoReconnectLabel),
                    library_copy(MessageId::SettingsAutoReconnectDescription),
                ))
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [0u8, 1, 3, 5, 10]
                            .into_iter()
                            .enumerate()
                            .map(|(index, attempts)| {
                                let selected = attempts == auto_reconnect_attempts;
                                Button::new(("settings-auto-reconnect-attempts", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(if attempts == 0 {
                                        library_copy(MessageId::SettingsValueOff)
                                    } else {
                                        localization::settings_attempts_option(attempts)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_attempts(attempts, cx);
                                    }))
                                    .into_any_element()
                            }),
                    ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsSshKeepaliveLabel),
                    library_copy(MessageId::SettingsSshKeepaliveDescription),
                ))
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [0u16, 15, 30, 60, 120]
                            .into_iter()
                            .enumerate()
                            .map(|(index, secs)| {
                                let selected = secs == ssh_keepalive_secs;
                                Button::new(("settings-ssh-keepalive", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(if secs == 0 {
                                        library_copy(MessageId::SettingsValueOff)
                                    } else {
                                        localization::settings_seconds_option(secs)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_ssh_keepalive_secs(secs, cx);
                                    }))
                                    .into_any_element()
                            }),
                    ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    library_copy(MessageId::SettingsReconnectDelayLabel),
                    library_copy(MessageId::SettingsReconnectDelayDescription),
                ))
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [2u8, 5, 10, 30]
                            .into_iter()
                            .enumerate()
                            .map(|(index, delay)| {
                                let selected = delay == auto_reconnect_delay_secs;
                                Button::new(("settings-auto-reconnect-delay", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        if selected {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .label(localization::settings_seconds_option(u16::from(delay)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_delay(delay, cx);
                                    }))
                                    .into_any_element()
                            }),
                    ),
                ),
        );

        let local_shell_card = self.settings_section_card(
            library_copy(MessageId::SettingsLocalShellTitle),
            library_copy(MessageId::SettingsLocalShellDescription),
            v_flex()
                .gap_3()
                .child(self.form_field(
                    library_copy(MessageId::SettingsLocalShellProgramLabel),
                    Input::new(&self.settings_inputs.local_shell_program),
                ))
                .child(self.form_field(
                    library_copy(MessageId::SettingsLocalShellCwdLabel),
                    Input::new(&self.settings_inputs.local_shell_cwd),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-local-shell-save")
                                .debug_selector(|| "settings-local-shell-save".to_string())
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(library_copy(MessageId::SettingsSaveShellAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_local_shell_settings(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsLocalShellArgsHint)),
                        ),
                ),
        );

        let diagnostics_card = self.settings_section_card(
            localization::diagnostics_settings_title(),
            localization::diagnostics_settings_description(),
            v_flex()
                .gap_4()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(match crate::diagnostics::status() {
                                termirust_diagnostics::DiagnosticStatus::Healthy => {
                                    IconName::CircleCheck
                                }
                                termirust_diagnostics::DiagnosticStatus::Dropping => {
                                    IconName::TriangleAlert
                                }
                                termirust_diagnostics::DiagnosticStatus::DiskError => {
                                    IconName::CircleX
                                }
                                termirust_diagnostics::DiagnosticStatus::Disabled => {
                                    IconName::CircleX
                                }
                            })
                            .size(px(theme::ICON_SIZE_COMPACT))
                            .text_color(theme::text_muted()),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child(diagnostics_model.status.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .p(px(theme::SPACE_MICRO))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::hover())
                        .children(
                            [true, false]
                                .into_iter()
                                .enumerate()
                                .map(|(index, enabled)| {
                                    Button::new(("settings-diagnostics-enabled", index))
                                        .small()
                                        .custom(Self::segmented_button_style(
                                            enabled == diagnostics_enabled,
                                            cx,
                                        ))
                                        .label(if enabled {
                                            localization::diagnostics_enable_action()
                                        } else {
                                            localization::diagnostics_disable_action()
                                        })
                                        .disabled(diagnostics_busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_diagnostics_enabled(enabled, cx);
                                        }))
                                        .into_any_element()
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(diagnostics_model.usage.clone()),
                )
                .child(self.settings_subhead(
                    localization::diagnostics_file_limit_label(),
                    localization::diagnostics_privacy_notice(),
                ))
                .child(h_flex().gap_2().flex_wrap().children(
                    [1_u8, 5, 10].into_iter().enumerate().map(|(index, limit)| {
                        Button::new(("settings-diagnostics-file-limit", index))
                            .small()
                            .custom(Self::action_button_style(
                                if limit == diagnostics_file_limit {
                                    theme::ActionTone::Accent
                                } else {
                                    theme::ActionTone::Neutral
                                },
                                cx,
                            ))
                            .label(localization::diagnostics_file_limit_option(limit))
                            .disabled(diagnostics_busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_diagnostics_file_limit(limit, cx);
                            }))
                            .into_any_element()
                    }),
                ))
                .child(self.settings_subhead(
                    localization::diagnostics_retention_label(),
                    localization::diagnostics_clear_notice(),
                ))
                .child(h_flex().gap_2().flex_wrap().children(
                    [1_u8, 7, 14].into_iter().enumerate().map(|(index, days)| {
                        Button::new(("settings-diagnostics-retention", index))
                            .small()
                            .custom(Self::action_button_style(
                                if days == diagnostics_retention {
                                    theme::ActionTone::Accent
                                } else {
                                    theme::ActionTone::Neutral
                                },
                                cx,
                            ))
                            .label(localization::diagnostics_retention_option(days))
                            .disabled(diagnostics_busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_diagnostics_retention(days, cx);
                            }))
                            .into_any_element()
                    }),
                ))
                .child(self.settings_divider())
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("settings-diagnostics-clear")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .label(localization::diagnostics_clear_action())
                                .disabled(!diagnostics_model.can_clear)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_diagnostics(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-diagnostics-preview")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(localization::diagnostics_preview_action())
                                .disabled(!diagnostics_model.can_preview)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.preview_diagnostics_export(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-diagnostics-export")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(localization::diagnostics_export_action())
                                .disabled(!diagnostics_model.can_export)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_previewed_diagnostics(cx);
                                })),
                        )
                        .when(diagnostics_busy, |this| {
                            this.child(
                                Button::new("settings-diagnostics-cancel")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label(localization::common_cancel())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_diagnostics_operation(cx);
                                    })),
                            )
                        }),
                )
                .when(diagnostics_busy, |this| {
                    this.child(
                        div()
                            .id("settings-diagnostics-operation-status")
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child(localization::diagnostics_operation_running()),
                    )
                })
                .when_some(diagnostics_preview_summary, |this, summary| {
                    this.child(
                        v_flex()
                            .id("settings-diagnostics-preview-summary")
                            .gap_2()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_main())
                            .child(div().font_semibold().child(summary))
                            .child(localization::diagnostics_preview_included())
                            .child(localization::diagnostics_preview_excluded()),
                    )
                }),
        );

        let health_card = self.settings_section_card(
            localization::health_settings_title(),
            localization::health_settings_description(),
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(if self
                                .health_report
                                .as_ref()
                                .is_some_and(termirust_store::HealthReport::is_healthy)
                            {
                                IconName::CircleCheck
                            } else {
                                IconName::TriangleAlert
                            })
                            .size(px(theme::ICON_SIZE_COMPACT))
                            .text_color(theme::text_muted()),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child(health_model.status.clone()),
                        ),
                )
                .children(health_model.findings.iter().enumerate().map(|(index, finding)| {
                    let kind = finding.kind;
                    let can_rebuild = finding.can_rebuild;
                    let action_label = match kind {
                        termirust_store::HealthCheckKind::ProjectSessionIndex => {
                            localization::health_rebuild_project_session_action()
                        }
                        termirust_store::HealthCheckKind::PaletteIndex => {
                            localization::health_rebuild_palette_action()
                        }
                        _ => String::new(),
                    };
                    h_flex()
                        .id(("settings-health-finding", index))
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .p(px(theme::SPACE_COMPACT))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .bg(theme::hover())
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_semibold()
                                        .text_color(theme::text_main())
                                        .child(finding.label.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(finding.state.clone()),
                                ),
                        )
                        .when(can_rebuild, |this| {
                            this.child(
                                Button::new(("settings-health-rebuild", index))
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label(action_label)
                                    .on_click(cx.listener(move |app, _, _, cx| match kind {
                                        termirust_store::HealthCheckKind::ProjectSessionIndex => {
                                            app.rebuild_derived_index(
                                                termirust_store::IndexRepairKind::ProjectSessionIndex,
                                                cx,
                                            );
                                        }
                                        termirust_store::HealthCheckKind::PaletteIndex => {
                                            app.rebuild_derived_index(
                                                termirust_store::IndexRepairKind::PaletteIndex,
                                                cx,
                                            );
                                        }
                                        _ => {}
                                    })),
                            )
                        })
                        .into_any_element()
                }))
                .when(recovery_model.visible, |this| {
                    this.child(
                        v_flex()
                            .id("settings-metadata-recovery")
                            .gap_2()
                            .p_3()
                            .rounded(px(theme::CONTROL_RADIUS))
                            .border_1()
                            .border_color(theme::with_alpha(theme::warning(), 0.55))
                            .bg(theme::with_alpha(theme::warning(), 0.08))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::recovery_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::recovery_description()),
                            )
                            .when(self.metadata_recovery_plan.is_some(), |this| {
                                this.child(
                                    div()
                                        .id("settings-metadata-recovery-impact")
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_main())
                                        .child(localization::recovery_impact(
                                            recovery_model.changed_files,
                                            recovery_model.unchanged_files,
                                            recovery_model.backup_bytes,
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(localization::recovery_safety_notice()),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .when(health_model.can_prepare_restore, |this| {
                                        this.child(
                                            Button::new("settings-recovery-prepare")
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label(localization::recovery_prepare_action())
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.prepare_metadata_recovery(cx);
                                                })),
                                        )
                                    })
                                    .when(recovery_model.can_confirm, |this| {
                                        this.child(
                                            Button::new("settings-recovery-confirm")
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Danger,
                                                    cx,
                                                ))
                                                .label(localization::recovery_confirm_action())
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.confirm_metadata_recovery(cx);
                                                })),
                                        )
                                    })
                                    .when(recovery_model.can_cancel, |this| {
                                        this.child(
                                            Button::new("settings-recovery-cancel")
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label(localization::common_cancel())
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.cancel_metadata_recovery(cx);
                                                })),
                                        )
                                    }),
                            ),
                    )
                })
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::health_unaffected_notice()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("settings-health-scan")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label(localization::health_scan_action())
                                .disabled(!health_model.can_scan)
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.scan_store_health(cx);
                                })),
                        )
                        .when(health_busy, |this| {
                            this.child(
                                Button::new("settings-health-cancel")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label(localization::health_cancel_action())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.cancel_health_operation(cx);
                                    })),
                            )
                        }),
                ),
        );

        let portable_card = self.settings_section_card(
            library_copy(MessageId::SettingsPortableDataTitle),
            library_copy(MessageId::SettingsPortableDataDescription),
            h_flex()
                .gap_2()
                .child(
                    Button::new("settings-export-data")
                        .small()
                        .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                        .label(library_copy(MessageId::SettingsExportDataAction))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.export_portable_data(cx);
                        })),
                )
                .child(
                    Button::new("settings-import-data")
                        .small()
                        .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                        .label(library_copy(MessageId::SettingsImportDataAction))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.import_portable_data(window, cx);
                        })),
                ),
        );

        let encrypted_card = self.settings_section_card(
            library_copy(MessageId::SettingsEncryptedBackupTitle),
            library_copy(MessageId::SettingsEncryptedBackupDescription),
            v_flex()
                .gap_3()
                .child(self.form_field(
                    library_copy(MessageId::SettingsExportPassphraseLabel),
                    Input::new(&self.settings_inputs.export_backup_passphrase),
                ))
                .child(self.form_field(
                    library_copy(MessageId::SettingsConfirmPassphraseLabel),
                    Input::new(&self.settings_inputs.export_backup_confirm),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-export-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(library_copy(MessageId::SettingsExportEncryptedAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsPassphraseSafetyNotice)),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-export-mobile-vault")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(library_copy(MessageId::SettingsExportMobileVaultAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export_mobile_vault(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsMobileVaultDescription)),
                        ),
                )
                .child(self.form_field(
                    library_copy(MessageId::SettingsMobilePairingLabel),
                    Input::new(&self.settings_inputs.mobile_pairing_request),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-import-mobile-pairing-request")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsApproveMobileAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_mobile_pairing_request(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsMobilePairingDescription)),
                        ),
                )
                .child(self.render_mobile_devices_settings(cx))
                .child(self.settings_divider())
                .child(self.form_field(
                    library_copy(MessageId::SettingsImportPassphraseLabel),
                    Input::new(&self.settings_inputs.import_backup_passphrase),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-import-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsImportEncryptedAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsEncryptedImportDescription)),
                        ),
                ),
        );

        let secure_sync_configured =
            desktop_replication_root().is_ok_and(|root| replication_is_configured(&root));
        let last_sync = self
            .saved
            .settings
            .sync_last_pulled_at
            .map(|_| library_copy(MessageId::SettingsSecureSyncLastCompleted))
            .unwrap_or_else(|| library_copy(MessageId::SettingsSecureSyncNeverCompleted));
        let sync_card = self.settings_section_card(
            library_copy(MessageId::SettingsSecureSyncTitle),
            library_copy(MessageId::SettingsSecureSyncDescription),
            v_flex()
                .gap_3()
                .child(self.form_field(
                    library_copy(MessageId::SettingsSyncFolderLabel),
                    Input::new(&self.settings_inputs.sync_folder_input),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-pick-folder")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsChooseFolderAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-sync-save-folder")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsSaveFolderAction))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_sync_folder_input(window, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            Button::new("settings-secure-sync")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(if secure_sync_configured {
                                    library_copy(MessageId::SettingsSecureSyncNowAction)
                                } else {
                                    library_copy(MessageId::SettingsSecureSyncEnableAction)
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sync_secure_folder(cx);
                                })),
                        )
                        .when(self.replication_recovery_required, |this| {
                            this.child(
                                Button::new("settings-secure-sync-recover")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label(library_copy(MessageId::SettingsSecureSyncRecoverAction))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.recover_secure_replication(cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(if secure_sync_configured {
                                    library_copy(MessageId::SettingsSecureSyncConfigured)
                                } else {
                                    library_copy(MessageId::SettingsSecureSyncNotConfigured)
                                }),
                        ),
                )
                .when(!self.replication_conflicts.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .gap_3()
                            .p_3()
                            .border_1()
                            .border_color(theme::with_alpha(theme::warning(), 0.5))
                            .rounded_md()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SIZE))
                                    .text_color(theme::text_main())
                                    .child(library_copy(
                                        MessageId::SettingsSecureSyncConflictReview,
                                    )),
                            )
                            .children(self.replication_conflicts.iter().enumerate().map(
                                |(conflict_index, conflict)| {
                                    let selected = self
                                        .replication_conflict_choices
                                        .get(conflict_index)
                                        .copied()
                                        .flatten();
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::text_muted())
                                                .child(conflict.title.clone()),
                                        )
                                        .child(h_flex().gap_2().flex_wrap().children(
                                            conflict.candidates.iter().enumerate().map(
                                                move |(candidate_index, candidate)| {
                                                    let device = candidate
                                                        .device_id
                                                        .trim_start_matches("device-")
                                                        .chars()
                                                        .take(8)
                                                        .collect::<String>();
                                                    Button::new((
                                                        "settings-sync-conflict",
                                                        conflict_index * 16 + candidate_index,
                                                    ))
                                                    .small()
                                                    .custom(Self::action_button_style(
                                                        if selected == Some(candidate_index) {
                                                            theme::ActionTone::AccentSoft
                                                        } else {
                                                            theme::ActionTone::Neutral
                                                        },
                                                        cx,
                                                    ))
                                                        .label(localization::dynamic_user_data_message(
                                                            MessageId::SettingsSecureSyncCandidateAction,
                                                            vec![candidate.summary.clone(), device],
                                                        ))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.select_replication_conflict(
                                                            conflict_index,
                                                            candidate_index,
                                                            cx,
                                                        );
                                                    }))
                                                },
                                            ),
                                        ))
                                },
                            ))
                            .child(
                                Button::new("settings-sync-resolve")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label(library_copy(
                                        MessageId::SettingsSecureSyncApplySelectionAction,
                                    ))
                                    .disabled(
                                        self.replication_conflict_choices
                                            .iter()
                                            .any(Option::is_none),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resolve_replication_conflicts(cx);
                                    })),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-push")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(
                                    MessageId::SettingsSecureSyncLegacyExportAction,
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.push_to_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(library_copy(MessageId::SettingsSecureSyncLegacyHint)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(last_sync),
                ),
        );

        let shortcuts_card = self.settings_section_card(
            library_copy(MessageId::SettingsKeyboardShortcutsTitle),
            library_copy(MessageId::SettingsKeyboardShortcutsDescription),
            v_flex()
                .gap_4()
                .child(self.settings_shortcut_group(
                    library_copy(MessageId::SettingsShortcutNavigation),
                    [
                        (
                            "Cmd+1",
                            library_copy(MessageId::SettingsShortcutOpenActivity),
                        ),
                        ("Cmd+2", localization::projects_shortcut_description()),
                        (
                            "Cmd+3",
                            library_copy(MessageId::SettingsShortcutOpenConnections),
                        ),
                        (
                            "Cmd+4",
                            library_copy(MessageId::SettingsShortcutOpenSessions),
                        ),
                        ("Cmd+5", library_copy(MessageId::SettingsShortcutOpenFiles)),
                        (
                            "Cmd+6",
                            library_copy(MessageId::SettingsShortcutOpenDevices),
                        ),
                        (
                            "Cmd+7",
                            library_copy(MessageId::SettingsShortcutOpenSettings),
                        ),
                        (
                            "Cmd+,",
                            library_copy(MessageId::SettingsShortcutJumpSettings),
                        ),
                        ("Cmd+L", library_copy(MessageId::SettingsShortcutFocusHosts)),
                        ("Cmd+N", library_copy(MessageId::SettingsShortcutNewHost)),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    library_copy(MessageId::SettingsShortcutWorkspace),
                    [
                        (
                            "Cmd+K",
                            library_copy(MessageId::SettingsShortcutCommandPalette),
                        ),
                        (
                            "Cmd+F",
                            library_copy(MessageId::SettingsShortcutTerminalSearch),
                        ),
                        (
                            "Cmd+T",
                            library_copy(MessageId::SettingsShortcutNewTerminal),
                        ),
                        (
                            "Cmd+W",
                            library_copy(MessageId::SettingsShortcutCloseWorkspace),
                        ),
                        (
                            "Cmd+D",
                            library_copy(MessageId::SettingsShortcutDuplicatePane),
                        ),
                        (
                            "Cmd+Alt+Right",
                            library_copy(MessageId::SettingsShortcutNextWorkspace),
                        ),
                        (
                            "Cmd+Alt+Left",
                            library_copy(MessageId::SettingsShortcutPreviousWorkspace),
                        ),
                        (
                            "Cmd+Shift+B",
                            library_copy(MessageId::SettingsShortcutBroadcast),
                        ),
                        (
                            "Cmd+Shift+L",
                            library_copy(MessageId::SettingsShortcutClearTerminal),
                        ),
                        (
                            "Cmd+Shift+F",
                            library_copy(MessageId::SettingsShortcutFilesBrowser),
                        ),
                        (
                            "Cmd+Shift+T",
                            library_copy(MessageId::SettingsShortcutToggleFiles),
                        ),
                        ("Esc", library_copy(MessageId::SettingsShortcutCloseDialog)),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    library_copy(MessageId::SettingsShortcutTerminal),
                    [
                        ("Cmd+C", library_copy(MessageId::SettingsShortcutCopy)),
                        ("Cmd+V", library_copy(MessageId::SettingsShortcutPaste)),
                        (
                            "Shift+PageUp",
                            library_copy(MessageId::SettingsShortcutScrollBack),
                        ),
                        (
                            "Shift+PageDown",
                            library_copy(MessageId::SettingsShortcutScrollForward),
                        ),
                        (
                            "Up / Down",
                            library_copy(MessageId::SettingsShortcutAutocompleteMove),
                        ),
                        (
                            "Enter",
                            library_copy(MessageId::SettingsShortcutAutocompleteAccept),
                        ),
                    ],
                )),
        );

        let notification_card = self.render_notification_settings_card(cx);
        let remote_devices_card = self.render_remote_devices_settings_card(cx);
        let cli_card = self.render_cli_settings_card(cx);

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                v_flex()
                    .gap(px(theme::SPACE_1))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(library_copy(MessageId::SettingsTitle)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted())
                            .child(library_copy(MessageId::SettingsSubtitle)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.settings_inputs.search).flex_1())
                    .when(query_active, |this| {
                        this.child(
                            Button::new("settings-search-clear")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(library_copy(MessageId::SettingsSearchClear))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    Self::set_input_value(
                                        &this.settings_inputs.search,
                                        "",
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    }),
            )
            .when(query_active, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::settings_search_count(result_count)),
                )
            })
            .child(
                v_flex()
                    .id("settings-scroll-viewport")
                    .debug_selector(|| "settings-scroll-viewport".to_string())
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("settings-scroll")
                            .gap_4()
                            .track_scroll(&self.settings_scroll)
                            .when(query_active && result_count == 0, |this| {
                                this.child(
                                    v_flex()
                                        .items_center()
                                        .gap_2()
                                        .p_5()
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                                .font_semibold()
                                                .text_color(theme::text_main())
                                                .child(library_copy(
                                                    MessageId::SettingsSearchEmptyTitle,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                                .text_color(theme::text_muted())
                                                .child(library_copy(
                                                    MessageId::SettingsSearchEmptyDescription,
                                                )),
                                        ),
                                )
                            })
                            .when(appearance_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(SettingsSectionId::Appearance),
                                )
                                .child(appearance_card)
                                .when_some(localization_card, |this, card| this.child(card))
                            })
                            .when(terminal_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(SettingsSectionId::Terminal),
                                )
                                .child(terminal_card)
                            })
                            .when(projects_sessions_visible, |this| {
                                this.child(self.settings_hierarchy_heading(
                                    SettingsSectionId::ProjectsSessions,
                                ))
                                .child(startup_card)
                                .child(sessions_card)
                            })
                            .when(presets_runtimes_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(
                                        SettingsSectionId::PresetsRuntimes,
                                    ),
                                )
                                .child(local_shell_card)
                                .child(cli_card)
                            })
                            .when(notifications_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(
                                        SettingsSectionId::Notifications,
                                    ),
                                )
                                .child(notification_card)
                            })
                            .when(keyboard_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(SettingsSectionId::Keyboard),
                                )
                                .child(shortcuts_card)
                            })
                            .when(storage_visible, |this| {
                                this.child(self.settings_hierarchy_heading(
                                    SettingsSectionId::StoragePrivacyDiagnostics,
                                ))
                                .child(diagnostics_card)
                                .child(health_card)
                                .child(portable_card)
                                .child(encrypted_card)
                                .child(sync_card)
                            })
                            .when(remote_devices_visible, |this| {
                                this.child(
                                    self.settings_hierarchy_heading(
                                        SettingsSectionId::RemoteDevices,
                                    ),
                                )
                                .child(remote_devices_card)
                            })
                            .overflow_y_scrollbar(),
                    ),
            )
    }
    // termirust-ui-surface:settings:end
}
