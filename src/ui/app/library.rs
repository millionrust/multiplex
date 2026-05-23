//! Library secondary pages: Keychain (Keys + Identities), Vaults, Known
//! Hosts, Logs, Snippets, and Settings. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, point, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::{
    AuthMode, DEFAULT_VAULT_ID, SessionLogEntry, SessionLogStatus, ThemePreset, VaultKind,
    VaultMemberRole,
};
use crate::ui::app::{
    ICON_KEY, ICON_SHIELD_CHECK, KeychainTab, NavSection, TermiRustApp, app_icon,
    primary_shortcut_label,
};
use crate::ui::theme;
use crate::ui::util::{format_relative_time, short_host_key};

impl TermiRustApp {
    fn keychain_tab_control(&self, cx: &Context<Self>) -> Div {
        let tab = self.keychain_tab;
        h_flex()
            .p(px(3.))
            .rounded(px(8.))
            .bg(theme::hover())
            .child(
                div()
                    .id("keychain-tab-keys")
                    .debug_selector(|| "keychain-tab-keys".to_string())
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(px(14.))
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
                    .child(app_icon(ICON_KEY).size(px(12.)).text_color(
                        if tab == KeychainTab::Keys {
                            theme::accent()
                        } else {
                            theme::text_muted()
                        },
                    ))
                    .child("Keys"),
            )
            .child(
                div()
                    .id("keychain-tab-identities")
                    .debug_selector(|| "keychain-tab-identities".to_string())
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(px(14.))
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
                    .child(Icon::new(IconName::User).size(px(12.)).text_color(
                        if tab == KeychainTab::Identities {
                            theme::accent()
                        } else {
                            theme::text_muted()
                        },
                    ))
                    .child("Identities"),
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
                            .text_size(px(14.))
                            .text_color(theme::text_muted())
                            .child("Reusable identities for host authentication."),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(!identities.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(13.))
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
                                Button::new("keychain-browse")
                                    .debug_selector(|| "keychain-browse".to_string())
                                    .small()
                                    .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                    .icon(IconName::FolderOpen)
                                    .label("Add Key File")
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
                                                .size(px(36.))
                                                .rounded(px(12.))
                                                .bg(theme::with_alpha(theme::accent(), 0.1))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    app_icon(ICON_KEY)
                                                        .size(px(16.))
                                                        .text_color(theme::accent()),
                                                ),
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
                                                                    "Imported",
                                                                    theme::library_bg(),
                                                                    theme::accent(),
                                                                ))
                                                            },
                                                        )
                                                        .when(index == 0, |this| {
                                                            this.child(self.status_badge(
                                                                "Default",
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
                                                        .text_size(px(12.))
                                                        .text_color(theme::text_muted())
                                                        .child(button_identity.key_path.clone()),
                                                ),
                                        ),
                                )
                                .child(
                                    Button::new(("keychain-use", index))
                                        .debug_selector(move || format!("keychain-use-{index}"))
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::AccentSoft,
                                            cx,
                                        ))
                                        .label("Use")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.use_identity(&button_identity, window, cx);
                                        })),
                                )
                                .into_any_element()
                        },
                    ))
                    .when(identities.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                app_icon(ICON_KEY)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No identities available",
                                "Add a key file to build a reusable identity library for your hosts.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("keys-empty-add")
                                            .debug_selector(|| "keys-empty-add".to_string())
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .icon(IconName::FolderOpen)
                                            .label("Add Key File")
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
                            .text_size(px(14.))
                            .text_color(theme::text_muted())
                            .child("Saved host identities with password authentication."),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
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
                                                    .size(px(36.))
                                                    .rounded(px(12.))
                                                    .bg(theme::with_alpha(theme::accent(), 0.1))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        Icon::new(IconName::User)
                                                            .size(px(16.))
                                                            .text_color(theme::accent()),
                                                    ),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(px(14.))
                                                            .font_semibold()
                                                            .text_color(theme::text_main())
                                                            .child(profile.display_name()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
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
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No password identities saved",
                                "Save a host with password authentication to keep its secure credential reference here.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("password-identities-open-hosts")
                                            .debug_selector(|| {
                                                "password-identities-open-hosts".to_string()
                                            })
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .label("New Host")
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
                        .text_size(px(22.))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child("Keys"),
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
                            .text_size(px(22.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Vaults"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
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
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Vaults are the top-level containers for hosts, identities, and snippets. Shared vaults are local-only metadata for now; sync comes later."),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field("Name", Input::new(&self.vault_inputs.label)))
                    .child(self.form_field(
                        "Description",
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
                                                .text_size(px(14.))
                                                .font_medium()
                                                .text_color(theme::text_main())
                                                .child("Members"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
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
                                            .rounded(px(12.))
                                            .bg(theme::with_alpha(theme::hover(), 0.72))
                                            .border_1()
                                            .border_color(theme::border())
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child("The personal vault is device-local and keeps a single owner profile."),
                                    )
                                })
                                .when(!vault.is_personal(), |this| {
                                    this.child(self.form_field(
                                        "Member Name",
                                        Input::new(&self.vault_member_inputs.name),
                                    ))
                                    .child(self.form_field(
                                        "Member Email",
                                        Input::new(&self.vault_member_inputs.email),
                                    ))
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .font_medium()
                                                    .text_color(theme::text_main())
                                                    .child("Role"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .children([
                                                        VaultMemberRole::Owner,
                                                        VaultMemberRole::Editor,
                                                        VaultMemberRole::Viewer,
                                                    ]
                                                    .into_iter()
                                                    .enumerate()
                                                    .map(|(index, role)| {
                                                        let selected = self.draft_vault_member_role == role;
                                                        div()
                                                            .id(("vault-member-role", index))
                                                            .debug_selector(move || {
                                                                format!("vault-member-role-{index}")
                                                            })
                                                            .px_3()
                                                            .py(px(7.))
                                                            .rounded(px(999.))
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
                                                                this.draft_vault_member_role = role;
                                                                this.error_message.clear();
                                                                cx.notify();
                                                            }))
                                                            .child(
                                                                div()
                                                                    .text_size(px(13.))
                                                                    .font_medium()
                                                                    .text_color(if selected {
                                                                        theme::text_main()
                                                                    } else {
                                                                        theme::text_muted()
                                                                    })
                                                                    .child(role.label()),
                                                            )
                                                            .into_any_element()
                                                    })),
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
                                                            .label("Clear Member")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                this.clear_vault_member_form(window, cx);
                                                            })),
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
                                                            .label("Save Member")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                this.save_vault_member(window, cx);
                                                            })),
                                                    ),
                                            ),
                                    )
                                })
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .children(vault.members.iter().enumerate().map(|(index, member)| {
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
                                                .rounded(px(12.))
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
                                                    this.load_vault_member_into_inputs(&member_id, window, cx);
                                                }))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap(px(1.))
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(13.))
                                                                        .font_semibold()
                                                                        .text_color(theme::text_main())
                                                                        .child(member.display_name()),
                                                                )
                                                                .child(self.status_badge(
                                                                    member.role.label(),
                                                                    theme::library_bg(),
                                                                    if member.role == VaultMemberRole::Owner {
                                                                        theme::accent()
                                                                    } else if member.role == VaultMemberRole::Editor {
                                                                        theme::success()
                                                                    } else {
                                                                        theme::slate()
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
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
                                                            .label("Remove")
                                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                                this.remove_vault_member(&remove_id, window, cx);
                                                            })),
                                                    )
                                                })
                                                .into_any_element()
                                        })),
                                ),
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
                                    .label("New")
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
                                    .label("Save")
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
                                            .label("Delete")
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
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(if vault.description.trim().is_empty() {
                                                "No description yet".to_string()
                                            } else {
                                                vault.description.clone()
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(self.status_badge(
                                                format!("{host_count} hosts"),
                                                theme::library_bg(),
                                                theme::success(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{identity_count} keys"),
                                                theme::library_bg(),
                                                theme::accent(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{snippet_count} snippets"),
                                                theme::library_bg(),
                                                theme::warning(),
                                            ))
                                            .child(self.status_badge(
                                                format!("{member_count} members"),
                                                theme::library_bg(),
                                                theme::slate(),
                                            )),
                                    ),
                            )
                            .into_any_element()
                    })),
            )
    }

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
                            .child("Known Hosts"),
                    )
                    .when(!entries.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "{} trusted {}",
                                    entries.len(),
                                    if entries.len() == 1 { "host" } else { "hosts" }
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Host keys are pinned on first connect (TOFU). Remove an entry here if a server has legitimately changed its key."),
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
                                            .text_size(px(12.))
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
                                                this.status_message = format!(
                                                    "Removed known host '{}'.",
                                                    remove_endpoint
                                                );
                                                this.error_message.clear();
                                            }
                                            Ok(false) => {
                                                this.status_message =
                                                    "Host was already removed.".to_string();
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
                                            .label("Open Hosts")
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
                            .child("Session History"),
                    )
                    .when(!logs.is_empty(), |this| {
                        this.child(
                            h_flex().gap_2().items_center().child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::text_muted())
                                    .child(format!("{} sessions", logs.len())),
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
                                                    .text_size(px(12.))
                                                    .text_color(theme::text_muted())
                                                    .child(pane.endpoint.clone()),
                                            ),
                                    ),
                            )
                            .child(self.status_badge(
                                "Active",
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
                                                    .text_size(px(12.))
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
                                                        .text_size(px(12.))
                                                        .text_color(theme::text_muted())
                                                        .child(format!(
                                                            "Started {}  Duration {}",
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
                                                            .text_size(px(12.))
                                                            .text_color(theme::danger())
                                                            .child(msg.clone()),
                                                    )
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
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
                                            .label("Open Hosts")
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
                            .text_size(px(22.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Snippets"),
                    )
                    .when(!snippets.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "{} {}",
                                    snippets.len(),
                                    if snippets.len() == 1 {
                                        "snippet"
                                    } else {
                                        "snippets"
                                    }
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .max_w(px(820.))
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(theme::text_muted())
                    .child("Save repeatable commands, pin the important ones, and send them to the active terminal in one click. Use {{HOST}}, {{USER}}, {{PORT}}, {{TITLE}}, or {{ADDRESS}} for auto-substitution; use {{?Name}} to ask for a value at run time — a small prompt panel opens in the workspace before the command is sent."),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .child(self.form_field("Label", Input::new(&self.snippet_inputs.label)))
                    .child(self.render_vault_picker(
                        self.snippet_vault_id.as_deref(),
                        |vault_id, this, _, cx| {
                            this.snippet_vault_id = Some(vault_id.clone());
                            this.selected_vault_id = Some(vault_id.clone());
                            this.status_message = format!(
                                "Assigning this snippet to {}.",
                                this.effective_vault_name(Some(&vault_id))
                            );
                            this.error_message.clear();
                            cx.notify();
                        },
                        _cx,
                    ))
                    .child(self.form_field("Group", Input::new(&self.snippet_inputs.group)))
                    .child(self.form_field("Command", Input::new(&self.snippet_inputs.command)))
                    .child(
                        h_flex()
                            .p(px(3.))
                            .rounded(px(8.))
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
                                        .label(if pinned { "Pinned" } else { "Library" })
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
                                    .label("New")
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
                                    .label("Save")
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
                                        .label("Delete")
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
                        let run_command = snippet.command.clone();
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
                            .border_color(if self.selected_snippet_id.as_deref()
                                == Some(snippet.id.as_str())
                            {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::card_hover_subtle()))
                            .on_click(_cx.listener(move |this, _, window, cx| {
                                this.load_snippet_into_inputs(&snippet_id, window, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
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
                                                    "Pinned",
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
                                            .text_size(px(12.))
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
                                            .label(if snippet.pinned { "Unpin" } else { "Pin" })
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
                                        Button::new(("snippet-run", index))
                                            .debug_selector(move || format!("snippet-run-{index}"))
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Success,
                                                _cx,
                                            ))
                                            .label("Run")
                                            .on_click(_cx.listener(move |this, _, window, cx| {
                                                this.run_snippet_command(&run_command, window, cx);
                                            })),
                                    ),
                            )
                            .into_any_element()
                    }))
                    .when(snippets.is_empty(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::BookOpen)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                "No snippets yet",
                                "Save repeatable commands here so they can be searched, pinned, and sent into active terminals.",
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_center()
                                    .child(
                                        Button::new("snippets-empty-new")
                                            .debug_selector(|| "snippets-empty-new".to_string())
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                _cx,
                                            ))
                                            .label("New Snippet")
                                            .on_click(_cx.listener(|this, _, window, cx| {
                                                this.clear_snippet_form(window, cx);
                                            })),
                                    ),
                            ),
                        )
                    }),
            )
    }

    fn settings_section_card<E: IntoElement>(
        &self,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        body: E,
    ) -> Div {
        let title: SharedString = title.into();
        let description: SharedString = description.into();
        v_flex()
            .w_full()
            .gap(px(16.))
            .px(px(22.))
            .py(px(20.))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::soft_border())
            .shadow(vec![
                gpui::BoxShadow {
                    color: theme::card_shadow_color(),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(2.),
                    spread_radius: px(0.),
                },
                gpui::BoxShadow {
                    color: theme::card_shadow_color(),
                    offset: point(px(0.), px(8.)),
                    blur_radius: px(24.),
                    spread_radius: px(-8.),
                },
            ])
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
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
                    .text_size(px(14.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
    }

    fn settings_divider(&self) -> Div {
        div()
            .h(px(1.))
            .w_full()
            .bg(theme::with_alpha(theme::border(), 0.6))
    }

    fn settings_shortcut_row(&self, keys: &'static str, description: &'static str) -> Div {
        h_flex()
            .justify_between()
            .items_center()
            .gap_3()
            .py(px(4.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_main())
                    .child(description),
            )
            .child(
                div()
                    .px_2()
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(theme::with_alpha(theme::hover(), 0.85))
                    .border_1()
                    .border_color(theme::border())
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(theme::text_muted())
                    .child(keys.replace("Cmd", primary_shortcut_label())),
            )
    }

    fn settings_shortcut_group<const N: usize>(
        &self,
        title: &'static str,
        rows: [(&'static str, &'static str); N],
    ) -> Div {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(13.))
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

        let appearance_card = self.settings_section_card(
            "Appearance",
            "Switch the global UI palette across the whole desktop app.",
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
                                    .py(px(8.))
                                    .rounded(px(999.))
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
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(if selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(preset.label()),
                                    )
                                    .into_any_element()
                            }),
                    ),
                )
                .child(
                    h_flex().gap_3().flex_wrap().children(
                        [
                            ("Library", theme::library_card(), theme::text_main()),
                            ("Chrome", theme::chrome_bg(), theme::text_on_dark()),
                            ("Terminal", theme::terminal_bg(), theme::text_on_dark()),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, bg, fg))| {
                            v_flex()
                                .id(("settings-preview", index))
                                .w(px(180.))
                                .gap_1()
                                .p_3()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(bg)
                                .border_1()
                                .border_color(theme::with_alpha(fg, 0.18))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_semibold()
                                        .text_color(fg)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::with_alpha(fg, 0.78))
                                        .child(match label {
                                            "Library" => "Forms, host cards, and management views",
                                            "Chrome" => "Tabs, status bar, and workspace header",
                                            _ => "Terminal panels and focused work sessions",
                                        }),
                                )
                                .into_any_element()
                        }),
                    ),
                ),
        );

        let terminal_card = self.settings_section_card(
            "Terminal",
            "Tune what feels right inside every PTY: font size, selection behavior, and clipboard flow.",
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    "Font size",
                    "Apply a larger or tighter monospace size across every terminal pane.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([12u16, 13, 14, 15, 16, 18].into_iter().enumerate().map(
                            |(index, font_size)| {
                                let selected = font_size == terminal_font_size;
                                div()
                                    .id(("settings-font-size", index))
                                    .debug_selector(move || {
                                        format!("settings-font-size-{index}")
                                    })
                                    .px_3()
                                    .py(px(8.))
                                    .rounded(px(999.))
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
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(if selected {
                                                theme::text_main()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(format!("{font_size} px")),
                                    )
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Copy on select",
                    "When enabled, releasing the mouse over a selection automatically copies it to the clipboard, like classic Unix terminals and Termius.",
                ))
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, enabled)| {
                                let active = enabled == copy_on_select;
                                Button::new(("settings-copy-on-select", index))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if enabled { "Auto Copy" } else { "Manual Only" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_copy_on_select(enabled, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Multi-line paste safety",
                    "Hold the paste in a confirmation banner when the clipboard contains newlines, so you don't accidentally execute a script you didn't mean to.",
                ))
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, enabled)| {
                                let active = enabled == confirm_multiline_paste;
                                Button::new(("settings-confirm-paste", index))
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if enabled { "Confirm" } else { "Direct" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_confirm_multiline_paste(enabled, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Font family",
                    "Override the monospace font used in every terminal pane. Leave blank to inherit the app default.",
                ))
                .child(self.form_field(
                    "Font Family",
                    Input::new(&self.settings_inputs.terminal_font_family),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-terminal-font-family-save")
                                .debug_selector(|| {
                                    "settings-terminal-font-family-save".to_string()
                                })
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Font Family")
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
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Reset")
                                .disabled(self.saved.settings.terminal_font_family.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_terminal_font_family(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Font names are passed to the platform font system; install the family first."),
                        ),
                ),
        );

        let startup_card = self.settings_section_card(
            "Startup",
            "Pick how the app comes back when you launch it and whether the first-run guide reappears.",
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(theme::hover())
                        .children([true, false].into_iter().enumerate().map(
                            |(index, restore)| {
                                let active = restore == restore_workspaces_on_launch;
                                Button::new(("settings-restore-workspaces", index))
                                    .debug_selector(move || {
                                        format!("settings-restore-workspaces-{index}")
                                    })
                                    .small()
                                    .custom(Self::segmented_button_style(active, cx))
                                    .label(if restore {
                                        "Restore Workspaces"
                                    } else {
                                        "Open Library"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_restore_workspaces_on_launch(restore, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-reset-onboarding")
                                .debug_selector(|| "settings-reset-onboarding".to_string())
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label(if onboarding_dismissed {
                                    "Show Welcome Panel Again"
                                } else {
                                    "Welcome Panel Visible"
                                })
                                .disabled(!onboarding_dismissed)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reset_onboarding_panel(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(if onboarding_dismissed {
                                    "Bring the first-run Hosts guide back after you have dismissed it."
                                } else {
                                    "The first-run Hosts guide is already available in the library."
                                }),
                        ),
                ),
        );

        let sessions_card = self.settings_section_card(
            "Sessions",
            "Control how connection history is retained and where SSH sessions begin by default.",
            v_flex()
                .gap_4()
                .child(self.settings_subhead(
                    "History retention",
                    "Keep this many connection history entries locally before older items roll off.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([100u16, 200, 500, 1000].into_iter().enumerate().map(
                            |(index, limit)| {
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
                                    .label(format!("{limit} entries"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_session_log_limit(limit, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(format!(
                            "{session_log_count} history entries currently stored."
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Default SSH startup directory",
                    "When a host has no startup directory set, SSH sessions cd into this directory after connecting.",
                ))
                .child(self.form_field(
                    "Startup Directory",
                    Input::new(&self.settings_inputs.default_ssh_startup_directory),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-default-ssh-dir-save")
                                .debug_selector(|| {
                                    "settings-default-ssh-dir-save".to_string()
                                })
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Default Directory")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_default_ssh_startup_directory(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-default-ssh-dir-clear")
                                .debug_selector(|| {
                                    "settings-default-ssh-dir-clear".to_string()
                                })
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Clear")
                                .disabled(!has_default_ssh_dir)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_default_ssh_startup_directory(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Per-host startup directories always take priority over this default."),
                        ),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Auto-reconnect",
                    "When an SSH session drops with an error or unexpected disconnect, retry this many times before giving up.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([0u8, 1, 3, 5, 10].into_iter().enumerate().map(
                            |(index, attempts)| {
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
                                        "Off".to_string()
                                    } else {
                                        format!("{attempts} attempts")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_attempts(attempts, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "SSH keep-alive",
                    "Send a SSH-level keep-alive ping at this interval so idle sessions survive NAT timeouts and load balancer drops.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([0u16, 15, 30, 60, 120].into_iter().enumerate().map(
                            |(index, secs)| {
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
                                        "Off".to_string()
                                    } else {
                                        format!("{secs}s")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_ssh_keepalive_secs(secs, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                )
                .child(self.settings_divider())
                .child(self.settings_subhead(
                    "Reconnect delay",
                    "Wait this many seconds between automatic retry attempts.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children([2u8, 5, 10, 30].into_iter().enumerate().map(
                            |(index, delay)| {
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
                                    .label(format!("{delay}s"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_auto_reconnect_delay(delay, cx);
                                    }))
                                    .into_any_element()
                            },
                        )),
                ),
        );

        let local_shell_card = self.settings_section_card(
            "Local Shell",
            "Choose which shell binary and working directory new local terminals use.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Shell Program",
                    Input::new(&self.settings_inputs.local_shell_program),
                ))
                .child(self.form_field(
                    "Working Directory",
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
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Save Shell Defaults")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_local_shell_settings(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Args stay empty for now; this sets the default executable and startup directory."),
                        ),
                ),
        );

        let portable_card = self.settings_section_card(
            "Portable Data Bundle",
            "Export or import hosts, vaults, identities, snippets, and known-host trust records as a local JSON bundle. Passwords and system credential-store secrets are intentionally excluded, so this is safe for portability but not a full account sync.",
            h_flex()
                .gap_2()
                .child(
                    Button::new("settings-export-data")
                        .small()
                        .custom(Self::action_button_style(
                            theme::ActionTone::Neutral,
                            cx,
                        ))
                        .label("Export Data")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.export_portable_data(cx);
                        })),
                )
                .child(
                    Button::new("settings-import-data")
                        .small()
                        .custom(Self::action_button_style(
                            theme::ActionTone::Accent,
                            cx,
                        ))
                        .label("Import Data")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.import_portable_data(window, cx);
                        })),
                ),
        );

        let encrypted_card = self.settings_section_card(
            "Encrypted Backup",
            "Wrap the same portable bundle in passphrase-based encryption for device backups, handoff, or manual sync. The file stays locally managed; no cloud account is involved yet.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Export Passphrase",
                    Input::new(&self.settings_inputs.export_backup_passphrase),
                ))
                .child(self.form_field(
                    "Confirm Passphrase",
                    Input::new(&self.settings_inputs.export_backup_confirm),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-export-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Export Encrypted Backup")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Use a strong passphrase you can recover later. The file cannot be opened without it."),
                        ),
                )
                .child(self.settings_divider())
                .child(self.form_field(
                    "Import Passphrase",
                    Input::new(&self.settings_inputs.import_backup_passphrase),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-import-encrypted-data")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Import Encrypted Backup")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_encrypted_portable_data(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Import merges vaults, hosts, snippets, and trust records without exposing the plaintext bundle on disk."),
                        ),
                ),
        );

        let last_pushed = self
            .saved
            .settings
            .sync_last_pushed_at
            .map(|ts| format!("Last push: {}", format_relative_time(ts)))
            .unwrap_or_else(|| "Never pushed.".to_string());
        let last_pulled = self
            .saved
            .settings
            .sync_last_pulled_at
            .map(|ts| format!("Last pull: {}", format_relative_time(ts)))
            .unwrap_or_else(|| "Never pulled.".to_string());
        let sync_card = self.settings_section_card(
            "Shared-folder sync",
            "Cross-device sync without a server. Point at a Dropbox / iCloud Drive / Google Drive / Syncthing folder. Push writes the encrypted bundle; Pull merges the latest one. Your existing cloud drive carries the bundle between machines, so the encrypted file never lives on our servers.",
            v_flex()
                .gap_3()
                .child(self.form_field(
                    "Sync Folder",
                    Input::new(&self.settings_inputs.sync_folder_input),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-pick-folder")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Choose Folder…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-sync-save-folder")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Neutral,
                                    cx,
                                ))
                                .label("Save Folder Path")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_sync_folder_input(window, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("settings-sync-push")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::Accent,
                                    cx,
                                ))
                                .label("Push to Folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.push_to_sync_folder(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-sync-pull")
                                .small()
                                .custom(Self::action_button_style(
                                    theme::ActionTone::AccentSoft,
                                    cx,
                                ))
                                .label("Pull from Folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pull_from_sync_folder(window, cx);
                                })),
                        )
                        .when(self.sync_pull_pending_warning, |this| {
                            this.child(
                                Button::new("settings-sync-pull-force")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label("Force Overwrite")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.force_pull_from_sync_folder(window, cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child("Push reuses the passphrase set in Encrypted Backup; Pull uses the import passphrase."),
                        ),
                )
                .child(
                    h_flex()
                        .gap_4()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(last_pushed),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(last_pulled),
                        ),
                ),
        );

        let shortcuts_card = self.settings_section_card(
            "Keyboard Shortcuts",
            "Every shortcut available right now. Anything that uses a modifier follows your platform convention (Cmd on macOS, Ctrl elsewhere).",
            v_flex()
                .gap_4()
                .child(self.settings_shortcut_group(
                    "Navigation",
                    [
                        ("Cmd+1", "Open Hosts"),
                        ("Cmd+2", "Open Vaults"),
                        ("Cmd+3", "Open Keychain"),
                        ("Cmd+4", "Open Snippets"),
                        ("Cmd+5", "Open Settings"),
                        ("Cmd+6", "Open Known Hosts"),
                        ("Cmd+7", "Open Logs"),
                        ("Cmd+,", "Jump to Settings"),
                        ("Cmd+L", "Focus host search / toggle Logs"),
                        ("Cmd+N", "Create a new host (in library)"),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    "Workspace",
                    [
                        ("Cmd+K", "Open the command palette"),
                        ("Cmd+F", "Search the active terminal"),
                        ("Cmd+T", "Open a new local terminal in a fresh tab"),
                        ("Cmd+W", "Close the active workspace tab"),
                        ("Cmd+D", "Duplicate the active pane"),
                        ("Cmd+Alt+Right", "Cycle to the next workspace tab"),
                        ("Cmd+Alt+Left", "Cycle to the previous workspace tab"),
                        ("Cmd+Shift+B", "Toggle broadcast input across panes"),
                        ("Cmd+Shift+L", "Clear the active pane screen and scrollback"),
                        ("Cmd+Shift+F", "Open the workspace files browser"),
                        ("Cmd+Shift+T", "Toggle Files / Terminal view"),
                        ("Esc", "Close dialogs or return from Files"),
                    ],
                ))
                .child(self.settings_divider())
                .child(self.settings_shortcut_group(
                    "Terminal",
                    [
                        ("Cmd+C", "Copy current selection"),
                        ("Cmd+V", "Paste from clipboard"),
                        ("Shift+PageUp", "Scroll back one screen"),
                        ("Shift+PageDown", "Scroll forward one screen"),
                        ("Up / Down", "Move autocomplete selection"),
                        ("Enter", "Accept the highlighted suggestion"),
                    ],
                )),
        );

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child("Local desktop preferences"),
                    ),
            )
            .child(
                v_flex().flex_1().min_h_0().child(
                    v_flex()
                        .id("settings-scroll")
                        .gap_4()
                        .track_scroll(&self.settings_scroll)
                        .child(appearance_card)
                        .child(terminal_card)
                        .child(startup_card)
                        .child(sessions_card)
                        .child(local_shell_card)
                        .child(shortcuts_card)
                        .child(portable_card)
                        .child(encrypted_card)
                        .child(sync_card)
                        .overflow_y_scrollbar(),
                ),
            )
    }
}
