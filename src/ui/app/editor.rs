//! Termius-style host editor side panel: Address / General / SSH sections,
//! header (title + vault picker + overflow + collapse), footer (Connect /
//! Save / Delete), and the open / close / connect helpers.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, Entity, InteractiveElement as _, ParentElement, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use termirust_ui_contract::MessageId;

use crate::models::{AuthMode, ThemePreset};
use crate::ui::app::{
    EditorMenu, ICON_KEY, ICON_PANEL_COLLAPSE_RIGHT, ICON_TAG, NavSection, TermiRustApp, app_icon,
};
use crate::ui::localization;
use crate::ui::theme;

fn editor_message(id: MessageId) -> String {
    localization::message_id(id).unwrap_or_default()
}

impl TermiRustApp {
    pub(super) fn open_editor_for_new_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.nav_section = NavSection::Hosts;
        self.clear_profile_form(window, cx);
        self.show_editor_panel = true;
        self.show_new_host_menu = false;
    }

    pub(super) fn open_editor_for_new_host_with_address(
        &mut self,
        address: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_editor_for_new_host(window, cx);
        let trimmed = address.trim().to_string();
        if !trimmed.is_empty() {
            Self::set_input_value(&self.inputs.host, trimmed.clone(), window, cx);
            Self::set_input_value(&self.inputs.label, trimmed, window, cx);
        }
        Self::set_input_value(
            &self.shell_inputs.create_host_address,
            String::new(),
            window,
            cx,
        );
    }

    pub(super) fn close_editor_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_editor_panel = false;
        cx.notify();
    }

    pub(super) fn connect_from_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host_value = self.inputs.host.read(cx).value().trim().to_string();
        let label_value = self.inputs.label.read(cx).value().trim().to_string();
        if host_value.is_empty() && !label_value.is_empty() {
            Self::set_input_value(&self.inputs.host, label_value.clone(), window, cx);
        } else if label_value.is_empty() && !host_value.is_empty() {
            Self::set_input_value(&self.inputs.label, host_value.clone(), window, cx);
        }
        if self.inputs.username.read(cx).value().trim().is_empty() {
            let current_user = std::env::var("USER")
                .ok()
                .or_else(|| std::env::var("USERNAME").ok())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "user".to_string());
            Self::set_input_value(&self.inputs.username, current_user, window, cx);
        }
        let host_after = self.inputs.host.read(cx).value().trim().to_string();
        if host_after.is_empty() {
            self.error_message = editor_message(MessageId::HostEditorHostRequired);
            cx.notify();
            return;
        }
        self.save_profile(window, cx);
        let Some(profile_id) = self.selected_profile_id.clone() else {
            self.error_message = editor_message(MessageId::HostEditorSaveError);
            cx.notify();
            return;
        };
        self.show_editor_panel = false;
        self.open_choose_protocol_tab(&profile_id, window, cx);
    }

    fn editor_input_row(
        &self,
        icon: Option<Icon>,
        state: &Entity<InputState>,
        suffix: Option<AnyElement>,
    ) -> Div {
        let _ = suffix;
        h_flex()
            .w_full()
            .gap(px(theme::SPACE_3))
            .items_center()
            .when_some(icon, |this, ic| {
                this.child(
                    div()
                        .size(px(theme::ICON_SIZE_MEDIUM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            ic.size(px(theme::HOST_ICON_SIZE_BODY))
                                .text_color(theme::text_muted()),
                        ),
                )
            })
            .child(Input::new(state).flex_1())
    }

    fn editor_labeled_input_row(
        &self,
        label: &str,
        help: Option<&str>,
        icon: Option<Icon>,
        state: &Entity<InputState>,
    ) -> Div {
        v_flex()
            .w_full()
            .gap(px(theme::SPACE_FINE))
            .child(
                div()
                    .text_size(px(theme::TYPE_MICRO_SIZE))
                    .font_medium()
                    .text_color(theme::text_muted())
                    .child(label.to_string()),
            )
            .child(self.editor_input_row(icon, state, None))
            .when_some(help, |this, help| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_NANO_SIZE))
                        .text_color(theme::text_muted())
                        .child(help.to_string()),
                )
            })
    }

    fn editor_section_card(&self, title: Option<&str>, body: Div) -> Div {
        v_flex()
            .w_full()
            .p(px(theme::HOST_ICON_SIZE_BODY))
            .gap(px(theme::SPACE_COMPACT))
            .rounded(px(theme::HOST_CARD_RADIUS))
            .bg(theme::with_alpha(theme::hover(), 0.25))
            .when_some(title, |this, t| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(t.to_string()),
                )
            })
            .child(body)
    }

    fn editor_theme_row(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let preset = self.saved.settings.theme_preset;
        let preset_label = preset.label();
        h_flex()
            .id("editor-theme-toggle")
            .w_full()
            .h(px(theme::HOST_COMPACT_ROW_HEIGHT))
            .px(px(theme::SPACE_COMPACT))
            .gap(px(theme::SPACE_4))
            .items_center()
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_bg())
            .border_1()
            .border_color(theme::soft_border())
            .cursor_pointer()
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
            .child(
                div()
                    .w(px(theme::HOST_CARD_HEIGHT))
                    .h(px(theme::HOST_CONTROL_HEIGHT))
                    .rounded(px(theme::CONTROL_RADIUS))
                    .bg(theme::terminal_bg())
                    .border_1()
                    .border_color(theme::with_alpha(theme::accent(), 0.6))
                    .flex()
                    .items_center()
                    .pl(px(theme::SPACE_3))
                    .child(
                        div()
                            .w(px(theme::HOST_EDITOR_ICON_CONTAINER))
                            .h(px(theme::SPACE_1))
                            .rounded(px(theme::SPACE_1))
                            .bg(theme::accent()),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(theme::SPACE_1))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(preset_label),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .child(editor_message(MessageId::HostEditorThemeTooltip)),
                    ),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(theme::HOST_ICON_SIZE_BODY))
                    .text_color(theme::text_muted()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let presets = ThemePreset::all();
                let current = this.saved.settings.theme_preset;
                let idx = presets.iter().position(|p| *p == current).unwrap_or(0);
                let next = presets[(idx + 1) % presets.len()];
                this.saved.settings.theme_preset = next;
                theme::set_theme_preset(next);
                this.persist_runtime_state();
                cx.notify();
            }))
    }

    fn editor_protocol_row(&self, port_state: &Entity<InputState>) -> Div {
        h_flex()
            .gap(px(theme::SPACE_3))
            .items_center()
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(editor_message(MessageId::HostEditorProtocolPrefix)),
            )
            .child(
                div()
                    .w(px(theme::HOST_EDITOR_TALL_CONTROL))
                    .child(Input::new(port_state).xsmall()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(editor_message(MessageId::ConnectProtocolPort)),
            )
    }

    fn editor_auth_mode_button(
        &self,
        id: &'static str,
        label: String,
        mode: AuthMode,
        active: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_1()
            .h(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::CONTROL_RADIUS))
            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
            .font_medium()
            .cursor_pointer()
            .when(active, |this| {
                this.bg(theme::library_card())
                    .shadow_sm()
                    .text_color(theme::text_main())
            })
            .when(!active, |this| this.text_color(theme::text_muted()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_auth_mode(mode, cx);
                if mode == AuthMode::PrivateKey {
                    let _ = this.ensure_default_identity_selected(window, cx);
                }
            }))
            .child(label)
    }

    fn render_editor_panel_termius(&self, cx: &mut Context<Self>) -> Div {
        let auth_mode = self.draft_auth_mode;
        let agent_socket = self
            .inputs
            .identity_agent
            .read(cx)
            .value()
            .trim()
            .to_string();
        let agent_available = crate::ssh_auth::resolve_local_agent_socket(
            (!agent_socket.is_empty()).then_some(agent_socket.as_str()),
        )
        .is_ok();
        let address_row = self.editor_labeled_input_row(
            &editor_message(MessageId::HostAddressField),
            Some(&editor_message(MessageId::HostEditorAddressHelp)),
            Some(Icon::new(IconName::Globe)),
            &self.inputs.host,
        );

        let general_body = v_flex()
            .gap(px(theme::SPACE_3))
            .child(self.editor_labeled_input_row(
                &editor_message(MessageId::HostLabelField),
                Some(&editor_message(MessageId::HostEditorDisplayNameHelp)),
                None,
                &self.inputs.label,
            ))
            .child(self.editor_labeled_input_row(
                &editor_message(MessageId::HostEditorGroupField),
                Some(&editor_message(MessageId::HostEditorGroupHelp)),
                Some(Icon::new(IconName::Folder)),
                &self.inputs.group,
            ))
            .child(self.editor_labeled_input_row(
                &editor_message(MessageId::HostEditorTagsField),
                Some(&editor_message(MessageId::HostEditorTagsHelp)),
                Some(app_icon(ICON_TAG)),
                &self.inputs.tags,
            ));

        let ssh_body = v_flex()
            .gap(px(theme::SPACE_COMPACT))
            .child(self.editor_protocol_row(&self.inputs.port))
            .child(self.render_persistent_session_editor(cx))
            .child(div().h(px(theme::BORDER_HAIRLINE)).bg(theme::soft_border()))
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(editor_message(MessageId::HostEditorCredentialsHeading)),
            )
            .child(
                h_flex()
                    .p(px(theme::SPACE_MICRO))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::hover())
                    .child(self.editor_auth_mode_button(
                        "editor-auth-password",
                        editor_message(MessageId::HostAuthPassword),
                        AuthMode::Password,
                        auth_mode == AuthMode::Password,
                        cx,
                    ))
                    .child(self.editor_auth_mode_button(
                        "editor-auth-private-key",
                        editor_message(MessageId::HostAuthPrivateKey),
                        AuthMode::PrivateKey,
                        auth_mode == AuthMode::PrivateKey,
                        cx,
                    ))
                    .child(self.editor_auth_mode_button(
                        "editor-auth-agent",
                        editor_message(MessageId::HostAuthLocalAgent),
                        AuthMode::LocalAgent,
                        auth_mode == AuthMode::LocalAgent,
                        cx,
                    )),
            )
            .child(self.editor_labeled_input_row(
                &editor_message(MessageId::HostUsernameField),
                Some(&editor_message(MessageId::HostEditorUsernameHelp)),
                Some(Icon::new(IconName::User)),
                &self.inputs.username,
            ))
            .when(auth_mode == AuthMode::Password, |this| {
                this.child(self.editor_labeled_input_row(
                    &editor_message(MessageId::HostPasswordField),
                    Some(&editor_message(MessageId::HostEditorPasswordHelp)),
                    Some(app_icon(ICON_KEY)),
                    &self.inputs.password,
                ))
            })
            .when(auth_mode == AuthMode::PrivateKey, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .gap(px(theme::SPACE_FINE))
                        .child(
                            div()
                                .text_size(px(theme::TYPE_MICRO_SIZE))
                                .font_medium()
                                .text_color(theme::text_muted())
                                .child(editor_message(MessageId::HostKeyPathField)),
                        )
                        .child(
                            h_flex()
                                .gap(px(theme::SPACE_DENSE))
                                .child(Input::new(&self.inputs.key_path).flex_1())
                                .child(
                                    Button::new("editor-pick-key-file")
                                        .small()
                                        .ghost()
                                        .icon(IconName::FolderOpen)
                                        .label(editor_message(MessageId::CommonBrowse))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.set_auth_mode(AuthMode::PrivateKey, cx);
                                            this.pick_key_file(window, cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_NANO_SIZE))
                                .text_color(theme::text_muted())
                                .child(editor_message(MessageId::HostEditorKeyPickerHelp)),
                        )
                        .child(self.editor_labeled_input_row(
                            &editor_message(MessageId::HostKeyPassphraseField),
                            Some(&editor_message(MessageId::HostEditorKeyPassphraseHelp)),
                            Some(app_icon(ICON_KEY)),
                            &self.inputs.key_passphrase,
                        ))
                        .child(
                            v_flex()
                                .w_full()
                                .gap(px(theme::SPACE_FINE))
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_MICRO_SIZE))
                                        .font_medium()
                                        .text_color(theme::text_muted())
                                        .child(editor_message(
                                            MessageId::HostEditorCertificateField,
                                        )),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(theme::SPACE_DENSE))
                                        .child(Input::new(&self.inputs.certificate_path).flex_1())
                                        .child(
                                            Button::new("editor-pick-certificate-file")
                                                .small()
                                                .ghost()
                                                .icon(IconName::FolderOpen)
                                                .label(editor_message(MessageId::CommonBrowse))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.pick_certificate_file(window, cx);
                                                })),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_NANO_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(editor_message(
                                            MessageId::HostEditorCertificateHelp,
                                        )),
                                ),
                        ),
                )
            })
            .when(auth_mode == AuthMode::LocalAgent, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .gap(px(theme::SPACE_DENSE))
                        .child(self.editor_labeled_input_row(
                            &editor_message(MessageId::HostAgentSocketField),
                            Some(&editor_message(MessageId::HostEditorAgentHelp)),
                            Some(app_icon(ICON_KEY)),
                            &self.inputs.identity_agent,
                        ))
                        .child(
                            div()
                                .text_size(px(theme::TYPE_NANO_SIZE))
                                .text_color(if agent_available {
                                    theme::success()
                                } else {
                                    theme::warning()
                                })
                                .child(if agent_available {
                                    editor_message(MessageId::HostEditorAgentDetected)
                                } else {
                                    editor_message(MessageId::HostEditorAgentMissing)
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_NANO_SIZE))
                                .text_color(theme::text_muted())
                                .child(editor_message(MessageId::HostEditorAgentForwardingHelp)),
                        ),
                )
            })
            .child(
                div().id("editor-credentials-add").cursor_pointer().child(
                    h_flex()
                        .gap(px(theme::SPACE_DENSE))
                        .items_center()
                        .pt(px(theme::SPACE_1))
                        .child(
                            Icon::new(IconName::Plus)
                                .size(px(theme::SPACE_4))
                                .text_color(theme::text_muted()),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(editor_message(MessageId::HostEditorIdentityKinds)),
                        ),
                ),
            )
            .when(self.editor_advanced_expanded, |this| {
                this.child(div().h(px(theme::BORDER_HAIRLINE)).bg(theme::soft_border()))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .child(editor_message(MessageId::HostEditorStartupCommandField)),
                    )
                    .child(self.editor_input_row(
                        Some(Icon::new(IconName::SquareTerminal)),
                        &self.inputs.startup_command,
                        None,
                    ))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .pt(px(theme::SPACE_2))
                            .child(editor_message(MessageId::HostEditorJumpHostField)),
                    )
                    .child(self.editor_input_row(
                        Some(Icon::new(IconName::Globe)),
                        &self.inputs.jump_host,
                        None,
                    ))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .pt(px(theme::SPACE_2))
                            .child(editor_message(MessageId::HostEditorEnvironmentField)),
                    )
                    .child(self.editor_input_row(
                        Some(app_icon(ICON_TAG)),
                        &self.inputs.environment,
                        None,
                    ))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .pt(px(theme::SPACE_2))
                            .child(editor_message(MessageId::HostEditorTerminalThemeField)),
                    )
                    .child(self.editor_theme_row(cx))
            })
            .child(
                div()
                    .id("editor-show-more")
                    .cursor_pointer()
                    .pt(px(theme::SPACE_1))
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(if self.editor_advanced_expanded {
                                        editor_message(MessageId::HostEditorShowLess)
                                    } else {
                                        editor_message(MessageId::HostEditorShowMore)
                                    }),
                            )
                            .child(
                                Icon::new(if self.editor_advanced_expanded {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(theme::SPACE_4))
                                .text_color(theme::text_muted()),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor_advanced_expanded = !this.editor_advanced_expanded;
                        cx.notify();
                    })),
            );

        v_flex()
            .w_full()
            .gap(px(theme::SPACE_4))
            .child(self.editor_section_card(
                Some(&editor_message(MessageId::HostEditorAddressSection)),
                address_row,
            ))
            .child(self.editor_section_card(
                Some(&editor_message(MessageId::HostEditorGeneralSection)),
                general_body,
            ))
            .child(self.editor_section_card(None, ssh_body))
    }

    pub(super) fn render_editor_actions(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                Button::new("editor-connect")
                    .w_full()
                    .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                    .label(localization::common_connect())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.connect_from_editor(window, cx);
                    })),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new("editor-save")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .icon(IconName::Check)
                            .label(localization::common_save())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_profile(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .when(self.selected_profile_id.is_some(), |this| {
                        this.child(
                            Button::new("editor-delete")
                                .small()
                                .ghost()
                                .icon(IconName::Delete)
                                .label(localization::common_delete())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.remove_selected_profile(window, cx);
                                    this.close_editor_dialog(window, cx);
                                })),
                        )
                    }),
            )
    }

    fn render_editor_side_header(
        &self,
        title: &str,
        vault_label: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let vault_open = self.open_editor_menu == Some(EditorMenu::Vault);
        let overflow_open = self.open_editor_menu == Some(EditorMenu::Overflow);
        h_flex()
            .flex_none()
            .h(px(theme::CONNECT_PORT_WIDTH))
            .px(px(theme::ICON_SIZE_DEFAULT))
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .gap(px(theme::SPACE_1))
                    .child(
                        div()
                            .text_size(px(theme::ICON_SIZE_COMPACT))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .id("editor-vault-trigger")
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_2))
                                    .items_center()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::host_editor_vault_label(
                                                vault_label,
                                            )),
                                    )
                                    .child(
                                        Icon::new(if vault_open {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .size(px(theme::SPACE_COMPACT))
                                        .text_color(theme::text_muted()),
                                    ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_editor_menu =
                                    if this.open_editor_menu == Some(EditorMenu::Vault) {
                                        None
                                    } else {
                                        Some(EditorMenu::Vault)
                                    };
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(theme::SPACE_DENSE))
                    .items_center()
                    .child(
                        div()
                            .id("editor-side-overflow")
                            .size(px(theme::STATUS_HEIGHT))
                            .rounded(px(theme::CONTROL_RADIUS))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(overflow_open, |this| {
                                this.bg(theme::with_alpha(theme::hover(), 0.7))
                            })
                            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                Icon::new(IconName::Ellipsis)
                                    .size(px(theme::ICON_SIZE_COMPACT))
                                    .text_color(theme::text_main()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_editor_menu =
                                    if this.open_editor_menu == Some(EditorMenu::Overflow) {
                                        None
                                    } else {
                                        Some(EditorMenu::Overflow)
                                    };
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("editor-side-collapse")
                            .size(px(theme::STATUS_HEIGHT))
                            .rounded(px(theme::CONTROL_RADIUS))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                app_icon(ICON_PANEL_COLLAPSE_RIGHT)
                                    .size(px(theme::ICON_SIZE_COMPACT))
                                    .text_color(theme::text_main()),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor_dialog(window, cx);
                            })),
                    ),
            )
    }

    pub(super) fn render_editor_side_panel(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let title = if self.selected_profile_id.is_some() {
            editor_message(MessageId::HostEditorExistingTitle)
        } else {
            editor_message(MessageId::HostEditorNewTitle)
        };
        let vault_label = self.effective_vault_name(self.draft_vault_id.as_deref());

        v_flex()
            .id("editor-side-panel")
            .flex_none()
            .w(px(theme::HOST_EDITOR_WIDTH))
            .h_full()
            .bg(theme::library_card())
            .border_l_1()
            .border_color(theme::border())
            .relative()
            .overflow_hidden()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.submit_create_host_from_empty_state(window, cx) {
                    return;
                }
                this.close_editor_dialog(window, cx);
            }))
            .child(self.render_editor_side_header(&title, &vault_label, cx))
            .child(
                v_flex().flex_1().min_h_0().child(
                    v_flex()
                        .id("editor-side-scroll")
                        .debug_selector(|| "editor-side-scroll".to_string())
                        .flex_1()
                        .min_h_0()
                        .px(px(theme::ICON_SIZE_MEDIUM))
                        .py(px(theme::ICON_SIZE_DEFAULT))
                        .track_scroll(&self.host_editor_scroll)
                        .child(self.render_editor_panel_termius(cx))
                        .overflow_y_scrollbar(),
                ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .px(px(theme::ICON_SIZE_MEDIUM))
                    .py(px(theme::HOST_ICON_SIZE_BODY))
                    .gap(px(theme::SPACE_COMPACT))
                    .border_t_1()
                    .border_color(theme::soft_border())
                    .bg(theme::library_card())
                    .when(!self.error_message.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .gap(px(theme::SPACE_3))
                                .items_start()
                                .px(px(theme::SPACE_COMPACT))
                                .py(px(theme::SPACE_3))
                                .rounded(px(theme::CONTROL_RADIUS))
                                .bg(theme::with_alpha(theme::danger(), 0.12))
                                .border_1()
                                .border_color(theme::with_alpha(theme::danger(), 0.4))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(theme::HOST_ICON_SIZE_DENSE))
                                        .text_color(theme::danger()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(theme::TYPE_MICRO_SIZE))
                                        .text_color(theme::danger())
                                        .child(self.error_message.clone()),
                                ),
                        )
                    })
                    .child(self.render_editor_actions(cx)),
            )
    }
}
