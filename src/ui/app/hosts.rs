//! Hosts library page: host tile/list cards, top toolbar (search + NEW HOST
//! split menu + Grid/Tag/Sort/Avatar dropdowns), the absolute overlay layer
//! and the page wrapper. All methods are part of `TermiRustApp`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, ElementId, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, point, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Disableable, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::{AuthMode, HostProfile};
use crate::ui::app::{
    EditorMenu, HostsSort, HostsViewMode, ICON_CALENDAR, ICON_GRID, ICON_KEY, ICON_PENCIL,
    ICON_TAG, ICON_VAULT, TermiRustApp, ToolbarMenu, app_icon,
};
use crate::ui::theme;
use crate::ui::util::format_relative_time;

impl TermiRustApp {
    fn host_card(
        &self,
        card_ix: usize,
        profile: &HostProfile,
        selected: bool,
        batch_selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let profile_id = profile.id.clone();
        let connect_profile_id = profile.id.clone();
        let favorite_profile_id = profile.id.clone();
        let batch_profile_id = profile.id.clone();
        let favorite_selected = profile.favorite;
        let accent = match profile.color_tag {
            Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
            None => theme::host_chip_color(&profile.display_name()),
        };
        let group_label = profile.group.trim().to_string();
        let tags = profile.tags.iter().take(3).cloned().collect::<Vec<_>>();
        let identity_label = profile
            .identity_id
            .as_deref()
            .and_then(|identity_id| self.identity_by_id(identity_id))
            .map(|identity| identity.label.clone());
        let jump_host_label = profile
            .jump_host_id
            .as_deref()
            .and_then(|jump_host_id| self.jump_host_display_name(jump_host_id))
            .map(|label| format!("Via {label}"));
        let startup_label = (profile.startup_directory.is_some()
            || profile.startup_command.is_some())
        .then(|| "Startup".to_string());
        let connect_view_label = profile.start_in_files.then(|| "Files First".to_string());
        let scrollback_label = profile
            .terminal_scrollback_rows
            .map(|rows| format!("{}k Scrollback", rows / 1000))
            .filter(|label| label != "10k Scrollback");
        let forward_count = profile.effective_port_forward_rules().len();
        let forward_label = (forward_count > 0).then(|| {
            if forward_count == 1 {
                "1 Forward".to_string()
            } else {
                format!("{forward_count} Forwards")
            }
        });
        let last_connected_label = self
            .last_connected_at(profile)
            .map(|ts| format!("Last {}", format_relative_time(ts)));
        let protocols = if profile.auth_mode == AuthMode::PrivateKey {
            "key auth"
        } else {
            "password"
        };
        let protocol_icon = if profile.auth_mode == AuthMode::PrivateKey {
            app_icon(ICON_KEY)
        } else {
            Icon::new(IconName::User)
        };

        let visible_tags: Vec<String> = profile.tags.iter().take(4).cloned().collect();
        let _ = (
            tags,
            identity_label,
            jump_host_label,
            startup_label,
            connect_view_label,
            scrollback_label,
            forward_label,
            last_connected_label,
            protocols,
            protocol_icon,
            selected,
        );

        let _ = (group_label, visible_tags, connect_profile_id);
        let sublabel = if profile.username.trim().is_empty() {
            "ssh".to_string()
        } else {
            format!("{}@{}", profile.username, profile.endpoint())
        };
        let icon_bg = theme::with_alpha(accent, 0.18);
        h_flex()
            .id(("host-row", card_ix))
            .debug_selector(move || format!("host-row-{card_ix}"))
            .group(format!("host-row-group-{card_ix}"))
            .w(px(340.))
            .h(px(64.))
            .gap(px(12.))
            .items_center()
            .px(px(10.))
            .rounded(px(10.))
            .border_2()
            .border_color(if batch_selected {
                theme::with_alpha(theme::accent(), 0.5)
            } else {
                gpui::transparent_black()
            })
            .bg(theme::library_card())
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(theme::with_alpha(theme::hover(), 0.6))
                    .border_color(theme::accent())
            })
            .on_click(cx.listener({
                let profile_id = profile_id.clone();
                move |this, event: &ClickEvent, window, cx| {
                    let click_count = match event {
                        ClickEvent::Mouse(e) => e.up.click_count,
                        ClickEvent::Keyboard(_) => 1,
                    };
                    if click_count >= 2 {
                        this.open_connect_dialog_tab(&profile_id, window, cx);
                    } else {
                        this.select_profile_from_library(&profile_id, window, cx);
                    }
                }
            }))
            .child(
                div()
                    .size(px(44.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(icon_bg)
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(20.))
                            .text_color(accent),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(profile.display_name()),
                            )
                            .when(profile.favorite, |this| {
                                this.child(
                                    Icon::new(IconName::Star)
                                        .size(px(11.))
                                        .text_color(theme::warning()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child(sublabel),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .debug_selector(move || format!("host-row-select-{card_ix}"))
                            .child(
                                Button::new(("host-row-select", card_ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(if batch_selected {
                                        IconName::Check
                                    } else {
                                        IconName::Plus
                                    })
                                    .tooltip(if batch_selected {
                                        "Remove from batch"
                                    } else {
                                        "Select for batch"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_host_batch_selection(&batch_profile_id, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("host-row-favorite-{card_ix}"))
                            .child(
                                Button::new(("host-row-favorite", card_ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Star)
                                    .tooltip(if favorite_selected {
                                        "Unstar host"
                                    } else {
                                        "Star host"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.set_profile_favorite(
                                            &favorite_profile_id,
                                            !favorite_selected,
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .child({
                        let edit_profile_id = profile_id.clone();
                        div()
                            .id(("host-row-edit", card_ix))
                            .debug_selector(move || format!("host-row-edit-{card_ix}"))
                            .size(px(28.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(theme::text_muted())
                            .hover(|style| {
                                style
                                    .bg(theme::with_alpha(theme::hover(), 0.85))
                                    .text_color(theme::text_main())
                            })
                            .child(app_icon(ICON_PENCIL).size(px(14.)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.load_profile_into_inputs(&edit_profile_id, window, cx);
                                this.show_editor_panel = true;
                                cx.notify();
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    }),
            )
    }

    fn host_list_row(
        &self,
        card_ix: usize,
        profile: &HostProfile,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let profile_id = profile.id.clone();
        let select_id = profile.id.clone();
        let favorite_id = profile.id.clone();
        let edit_id = profile.id.clone();
        let display = profile.display_name();
        let endpoint = format!("{}@{}", profile.username, profile.endpoint());
        let is_batch_selected = self.selected_host_ids.contains(profile.id.as_str());
        let favorite_selected = profile.favorite;
        let accent = match profile.color_tag {
            Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
            None => theme::host_chip_color(&display),
        };
        h_flex()
            .id(("host-row-list", card_ix))
            .debug_selector(move || format!("host-row-list-{card_ix}"))
            .w_full()
            .h(px(40.))
            .gap(px(10.))
            .px(px(10.))
            .items_center()
            .rounded(px(6.))
            .bg(if selected {
                theme::accent_soft()
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
            .on_click(cx.listener({
                let pid = profile_id.clone();
                move |this, event: &ClickEvent, window, cx| {
                    let click_count = match event {
                        ClickEvent::Mouse(e) => e.up.click_count,
                        ClickEvent::Keyboard(_) => 1,
                    };
                    if click_count >= 2 {
                        this.open_connect_dialog_tab(&pid, window, cx);
                    } else {
                        this.select_profile_from_library(&pid, window, cx);
                    }
                }
            }))
            .child(div().size(px(8.)).rounded(px(999.)).bg(accent))
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(13.))
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .w(px(160.))
                    .text_size(px(13.))
                    .text_color(theme::text_main())
                    .child(display),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(endpoint),
            )
            .child(
                div()
                    .debug_selector(move || format!("host-row-list-select-{card_ix}"))
                    .child(
                        Button::new(("host-row-list-select", card_ix))
                            .xsmall()
                            .ghost()
                            .icon(if is_batch_selected {
                                IconName::Check
                            } else {
                                IconName::Plus
                            })
                            .tooltip(if is_batch_selected {
                                "Remove from batch"
                            } else {
                                "Select for batch"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_host_batch_selection(&select_id, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(move || format!("host-row-list-favorite-{card_ix}"))
                    .child(
                        Button::new(("host-row-list-favorite", card_ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Star)
                            .tooltip(if favorite_selected {
                                "Unstar host"
                            } else {
                                "Star host"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.set_profile_favorite(
                                    &favorite_id,
                                    !favorite_selected,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child({
                div()
                    .id(("host-row-list-edit", card_ix))
                    .debug_selector(move || format!("host-row-list-edit-{card_ix}"))
                    .size(px(24.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(theme::text_muted())
                    .hover(|s| {
                        s.bg(theme::with_alpha(theme::hover(), 0.85))
                            .text_color(theme::text_main())
                    })
                    .child(app_icon(ICON_PENCIL).size(px(13.)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.load_profile_into_inputs(&edit_id, window, cx);
                        this.show_editor_panel = true;
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
    }

    fn render_host_grid(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let groups = self.grouped_profiles(cx);

        let mut sections = Vec::new();
        let mut card_ix = 0usize;
        for (group_name, profiles) in &groups {
            let visible_count = profiles.len();
            let total_count = self
                .saved
                .profiles
                .iter()
                .filter(|profile| Self::profile_group_name(profile) == *group_name)
                .count();
            let _ = total_count;
            let is_only_ungrouped = groups.len() == 1 && group_name == "Ungrouped";
            let is_list = self.hosts_view_mode == HostsViewMode::List;
            let cards: Div = if is_list {
                v_flex()
                    .w_full()
                    .gap(px(2.))
                    .children(profiles.iter().enumerate().map(|(group_ix, profile)| {
                        self.host_list_row(
                            card_ix + group_ix,
                            profile,
                            self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                            cx,
                        )
                        .into_any_element()
                    }))
            } else {
                h_flex().w_full().flex_wrap().gap(px(10.)).children(
                    profiles.iter().enumerate().map(|(group_ix, profile)| {
                        self.host_card(
                            card_ix + group_ix,
                            profile,
                            self.selected_profile_id.as_deref() == Some(profile.id.as_str()),
                            self.selected_host_ids.contains(profile.id.as_str()),
                            cx,
                        )
                        .into_any_element()
                    }),
                )
            };

            let mut section = v_flex().w_full().gap(px(10.));
            if !is_only_ungrouped {
                section = section.child(
                    h_flex()
                        .h(px(24.))
                        .items_end()
                        .gap(px(8.))
                        .pl(px(2.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child(group_name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::text_muted())
                                .pb(px(1.))
                                .child(format!(
                                    "{} {}",
                                    visible_count,
                                    if visible_count == 1 { "host" } else { "hosts" }
                                )),
                        ),
                );
            }
            section = section.child(cards);
            sections.push(section.into_any_element());
            card_ix += profiles.len();
        }

        v_flex()
            .w_full()
            .gap_5()
            .children(sections)
            .when(groups.is_empty(), |this| {
                let query = self.host_search_query(cx);
                let empty_state = if query.trim().is_empty() {
                    self.render_library_empty_state(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "Create host",
                        "Save your connection details as hosts to connect in one click.",
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(40.))
                            .px(px(12.))
                            .items_center()
                            .rounded(px(8.))
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .border_1()
                            .border_color(theme::soft_border())
                            .text_size(px(13.))
                            .text_color(theme::text_main())
                            .child(
                                Input::new(&self.shell_inputs.create_host_address)
                                    .appearance(false)
                                    .flex_1(),
                            ),
                    )
                    .child(
                        Button::new("hosts-empty-new")
                            .w_full()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label("Continue")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !this.submit_create_host_from_empty_state(window, cx) {
                                    this.open_editor_for_new_host(window, cx);
                                }
                            })),
                    )
                } else {
                    self.render_library_empty_state(
                        Icon::new(IconName::Search)
                            .size(px(24.))
                            .text_color(theme::accent()),
                        "No hosts match this filter",
                        "Try a different search, or save a new host to add it to the library.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("hosts-empty-clear-search")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label("Clear Search")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        Self::set_input_value(
                                            &this.shell_inputs.host_search,
                                            "",
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("hosts-empty-new-filtered")
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
                    )
                };
                this.child(empty_state)
            })
    }

    fn render_new_host_split_button(&self, cx: &mut Context<Self>) -> Div {
        let menu_open = self.show_new_host_menu;
        div()
            .relative()
            .child(
                h_flex()
                    .gap(px(0.))
                    .items_center()
                    .child(
                        Button::new("library-new-host")
                            .debug_selector(|| "library-new-host".to_string())
                            .xsmall()
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .icon(IconName::Plus)
                            .label("NEW HOST")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor_for_new_host(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("library-new-host-chevron")
                            .debug_selector(|| "library-new-host-chevron".to_string())
                            .h(px(28.))
                            .px(px(6.))
                            .ml(px(2.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(if menu_open {
                                theme::with_alpha(theme::hover(), 0.85)
                            } else {
                                gpui::transparent_black()
                            })
                            .border_1()
                            .border_color(theme::soft_border())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                Icon::new(if menu_open {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(12.))
                                .text_color(theme::text_main()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_new_host_menu(cx);
                            })),
                    ),
            )
            .when(false, |this| {
                this.child(
                    v_flex()
                        .absolute()
                        .top(px(36.))
                        .left(px(0.))
                        .w(px(220.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::border())
                        .shadow(vec![gpui::BoxShadow {
                            color: theme::card_shadow_strong_color(),
                            offset: point(px(0.), px(8.)),
                            blur_radius: px(24.),
                            spread_radius: px(-6.),
                        }])
                        .child(self.new_host_menu_item(
                            "menu-new-group-x",
                            IconName::Folder,
                            "New Group",
                            false,
                            cx,
                            |this, window, cx| {
                                this.show_new_host_menu = false;
                                this.open_editor_for_new_host(window, cx);
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-import-x",
                            IconName::PanelLeft,
                            "Import",
                            false,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    "Import from SSH config / OpenSSH already loads automatically."
                                        .to_string();
                                cx.notify();
                            },
                        ))
                        .child(div().h(px(1.)).w_full().my(px(4.)).bg(theme::soft_border()))
                        .child(self.new_host_menu_item(
                            "menu-aws-x",
                            IconName::Globe,
                            "AWS Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    "AWS integration ships in a future release.".to_string();
                                cx.notify();
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-do-x",
                            IconName::Globe,
                            "DigitalOcean Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    "DigitalOcean integration ships in a future release."
                                        .to_string();
                                cx.notify();
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-azure-x",
                            IconName::Globe,
                            "Azure Integration",
                            true,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                this.status_message =
                                    "Azure integration ships in a future release.".to_string();
                                cx.notify();
                            },
                        )),
                )
            })
    }

    fn new_host_menu_item(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        cloud: bool,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        h_flex()
            .id(id)
            .w_full()
            .h(px(32.))
            .px(px(12.))
            .gap(px(10.))
            .items_center()
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .child(
                Icon::new(icon)
                    .size(px(14.))
                    .text_color(theme::text_muted_dark()),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .text_color(theme::text_main())
                    .child(label),
            )
            .when(cloud, |this| {
                this.child(
                    Icon::new(IconName::ExternalLink)
                        .size(px(11.))
                        .text_color(theme::text_muted_dark()),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                handler(this, window, cx);
            }))
    }

    fn toolbar_chevron_svg_button(
        &self,
        id: &'static str,
        svg_path: &'static str,
        _tooltip: &'static str,
        menu: ToolbarMenu,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.open_toolbar_menu == Some(menu);
        let chevron = if open {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };
        div()
            .id(id)
            .debug_selector(|| id.to_string())
            .h(px(28.))
            .px(px(8.))
            .gap(px(4.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(open, |this| this.bg(theme::with_alpha(theme::hover(), 0.7)))
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .child(
                app_icon(svg_path)
                    .size(px(14.))
                    .text_color(theme::text_main()),
            )
            .child(
                Icon::new(chevron)
                    .size(px(11.))
                    .text_color(theme::text_muted_dark()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_toolbar_menu = if this.open_toolbar_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            }))
    }

    pub(super) fn dropdown_item(
        &self,
        id: impl Into<ElementId>,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        h_flex()
            .id(id)
            .h(px(30.))
            .px(px(10.))
            .gap(px(10.))
            .items_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::with_alpha(theme::hover(), 0.7)))
            .when_some(icon, |this, icon| {
                this.child(icon.size(px(13.)).text_color(theme::text_muted_dark()))
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(theme::text_main())
                    .child(label),
            )
            .when(selected, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .size(px(12.))
                        .text_color(theme::accent()),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
                this.open_toolbar_menu = None;
                this.open_editor_menu = None;
                cx.notify();
            }))
    }

    fn render_view_mode_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-view-mode",
            ICON_GRID,
            "View mode",
            ToolbarMenu::ViewMode,
            cx,
        )
    }

    fn render_tag_filter_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-tag-filter",
            ICON_TAG,
            "Filter by tag",
            ToolbarMenu::TagFilter,
            cx,
        )
    }

    fn render_sort_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.toolbar_chevron_svg_button(
            "library-sort",
            ICON_CALENDAR,
            "Sort hosts",
            ToolbarMenu::Sort,
            cx,
        )
    }

    fn render_avatar_dropdown(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("library-avatar-trigger")
            .debug_selector(|| "library-avatar-trigger".to_string())
            .cursor_pointer()
            .child(self.toolbar_avatar_pill(cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_toolbar_menu = if this.open_toolbar_menu == Some(ToolbarMenu::Avatar) {
                    None
                } else {
                    Some(ToolbarMenu::Avatar)
                };
                cx.notify();
            }))
    }

    fn render_hosts_overlays(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.show_editor_panel {
            if self.open_editor_menu == Some(EditorMenu::Vault) {
                let mut panel = v_flex()
                    .min_w(px(220.))
                    .p(px(6.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg();
                let active = self.draft_vault_id.clone();
                for (idx, vault) in self.saved.vaults.iter().enumerate() {
                    let id = vault.id.clone();
                    let display = vault.display_name();
                    let is_active = active.as_deref() == Some(id.as_str());
                    panel = panel.child(self.dropdown_item(
                        ("vault-pick", idx),
                        Some(app_icon(ICON_VAULT)),
                        display,
                        is_active,
                        move |this, _, _| {
                            this.draft_vault_id = Some(id.clone());
                        },
                        cx,
                    ));
                }
                return div()
                    .id("editor-vault-overlay")
                    .absolute()
                    .top(px(148.))
                    .right(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_editor_menu = None;
                        cx.notify();
                    }))
                    .child(panel)
                    .into_any_element();
            }
            if self.open_editor_menu == Some(EditorMenu::Overflow) {
                let has_profile = self.selected_profile_id.is_some();
                let mut menu = v_flex()
                    .min_w(px(180.))
                    .p(px(6.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg()
                    .child(self.dropdown_item(
                        "overflow-connect",
                        Some(Icon::new(IconName::SquareTerminal)),
                        "Connect",
                        false,
                        |this, window, cx| {
                            eprintln!(
                                "[connect] overflow Connect clicked, selected_profile_id={:?}",
                                this.selected_profile_id
                            );
                            if let Some(id) = this.selected_profile_id.clone() {
                                this.open_choose_protocol_tab(&id, window, cx);
                            } else {
                                this.open_choose_protocol_tab_from_draft(window, cx);
                            }
                        },
                        cx,
                    ))
                    .child(self.dropdown_item(
                        "overflow-add-telnet",
                        Some(Icon::new(IconName::Plus)),
                        "Add Telnet",
                        false,
                        |this, _, _| {
                            this.editor_telnet_added = true;
                        },
                        cx,
                    ))
                    .child(self.dropdown_item(
                        "overflow-duplicate",
                        Some(Icon::new(IconName::Copy)),
                        "Duplicate",
                        false,
                        |this, _, cx| {
                            if let Some(id) = this.selected_profile_id.clone() {
                                if let Some(orig) =
                                    this.saved.profiles.iter().find(|p| p.id == id).cloned()
                                {
                                    let mut copy = orig.clone();
                                    copy.id =
                                        format!("{}-copy-{}", orig.id, this.next_session_id());
                                    copy.label = format!("{} (copy)", orig.label);
                                    this.saved.upsert_profile(copy.clone());
                                    this.selected_profile_id = Some(copy.id);
                                    this.persist_runtime_state();
                                    cx.notify();
                                }
                            }
                        },
                        cx,
                    ));
                if has_profile {
                    menu = menu.child(self.dropdown_item(
                        "overflow-remove",
                        Some(Icon::new(IconName::Delete)),
                        "Remove",
                        false,
                        |this, _, cx| {
                            if let Some(id) = this.selected_profile_id.clone() {
                                this.saved.remove_profile(&id);
                                this.show_editor_panel = false;
                                this.persist_runtime_state();
                                cx.notify();
                            }
                        },
                        cx,
                    ));
                }
                return div()
                    .id("editor-overflow-overlay")
                    .absolute()
                    .top(px(148.))
                    .right(px(50.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_editor_menu = None;
                        cx.notify();
                    }))
                    .child(menu)
                    .into_any_element();
            }
        }
        if self.show_new_host_menu {
            return div()
                .id("new-host-overlay")
                .absolute()
                .top(px(94.))
                .left(px(12.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.show_new_host_menu = false;
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .w(px(220.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::border())
                        .shadow_lg()
                        .child(self.new_host_menu_item(
                            "menu-new-group",
                            IconName::Folder,
                            "New Group",
                            false,
                            cx,
                            |this, window, cx| {
                                this.show_new_host_menu = false;
                                this.open_editor_for_new_host(window, cx);
                            },
                        ))
                        .child(self.new_host_menu_item(
                            "menu-import",
                            IconName::PanelLeft,
                            "Import from ~/.ssh/config",
                            false,
                            cx,
                            |this, _, cx| {
                                this.show_new_host_menu = false;
                                match crate::storage::load_local_ssh_hosts() {
                                    Ok(hosts) => {
                                        let count = hosts.len();
                                        this.saved.merge_imported_profiles(hosts);
                                        this.persist_runtime_state();
                                        this.status_message =
                                            format!("Imported {count} hosts from ~/.ssh/config.");
                                    }
                                    Err(e) => {
                                        this.error_message =
                                            format!("Could not import SSH config: {e}");
                                    }
                                }
                                cx.notify();
                            },
                        )),
                )
                .into_any_element();
        }
        let menu = self.open_toolbar_menu;
        let email = std::env::var("USER")
            .ok()
            .map(|u| format!("{u}@local"))
            .unwrap_or_else(|| "user@local".to_string());
        let mut tags: Vec<String> = self
            .saved
            .profiles
            .iter()
            .flat_map(|p| p.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        let inner: Div = match menu {
            Some(ToolbarMenu::ViewMode) => v_flex()
                .min_w(px(140.))
                .p(px(6.))
                .gap(px(2.))
                .rounded(px(8.))
                .bg(theme::library_card())
                .border_1()
                .border_color(theme::soft_border())
                .shadow_lg()
                .child(
                    self.dropdown_item(
                        "view-mode-grid",
                        Some(app_icon(ICON_GRID)),
                        "Grid",
                        self.hosts_view_mode == HostsViewMode::Grid,
                        |this, _, _| this.hosts_view_mode = HostsViewMode::Grid,
                        cx,
                    )
                    .debug_selector(|| "view-mode-grid".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "view-mode-list",
                        Some(Icon::new(IconName::Menu)),
                        "List",
                        self.hosts_view_mode == HostsViewMode::List,
                        |this, _, _| this.hosts_view_mode = HostsViewMode::List,
                        cx,
                    )
                    .debug_selector(|| "view-mode-list".to_string()),
                )
                .into(),
            Some(ToolbarMenu::TagFilter) => {
                if tags.is_empty() {
                    v_flex()
                        .w(px(220.))
                        .p(px(20.))
                        .gap(px(10.))
                        .items_center()
                        .rounded(px(8.))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::soft_border())
                        .shadow_lg()
                        .child(
                            div()
                                .size(px(36.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(theme::with_alpha(theme::hover(), 0.7))
                                .child(
                                    app_icon(ICON_TAG)
                                        .size(px(16.))
                                        .text_color(theme::text_main()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_semibold()
                                .text_color(theme::text_main())
                                .child("Add tags"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::text_muted())
                                .child(
                                    "Tags help you filter your hosts. You can add a tag when editing a host.",
                                ),
                        )
                } else {
                    let mut panel = v_flex()
                        .min_w(px(180.))
                        .p(px(6.))
                        .gap(px(2.))
                        .rounded(px(8.))
                        .bg(theme::library_card())
                        .border_1()
                        .border_color(theme::soft_border())
                        .shadow_lg();
                    let active_filter = self.hosts_tag_filter.clone();
                    panel = panel.child(
                        self.dropdown_item(
                            "tag-filter-all",
                            Some(app_icon(ICON_TAG)),
                            "All hosts",
                            active_filter.is_none(),
                            |this, _, _| this.hosts_tag_filter = None,
                            cx,
                        )
                        .debug_selector(|| "tag-filter-all".to_string()),
                    );
                    for (idx, tag) in tags.iter().enumerate() {
                        let tag_owned = tag.clone();
                        let is_active = active_filter.as_deref() == Some(tag.as_str());
                        panel = panel.child(
                            self.dropdown_item(
                                ("tag-filter", idx),
                                Some(app_icon(ICON_TAG)),
                                tag.clone(),
                                is_active,
                                move |this, _, _| {
                                    this.hosts_tag_filter = Some(tag_owned.clone());
                                },
                                cx,
                            )
                            .debug_selector(move || format!("tag-filter-{idx}")),
                        );
                    }
                    panel
                }
            }
            Some(ToolbarMenu::Sort) => v_flex()
                .min_w(px(180.))
                .p(px(6.))
                .gap(px(2.))
                .rounded(px(8.))
                .bg(theme::library_card())
                .border_1()
                .border_color(theme::soft_border())
                .shadow_lg()
                .child(
                    self.dropdown_item(
                        "sort-az",
                        Some(Icon::new(IconName::SortAscending)),
                        "A-z",
                        self.hosts_sort == HostsSort::AZ,
                        |this, _, _| this.hosts_sort = HostsSort::AZ,
                        cx,
                    )
                    .debug_selector(|| "sort-az".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "sort-za",
                        Some(Icon::new(IconName::SortDescending)),
                        "Z-a",
                        self.hosts_sort == HostsSort::ZA,
                        |this, _, _| this.hosts_sort = HostsSort::ZA,
                        cx,
                    )
                    .debug_selector(|| "sort-za".to_string()),
                )
                .child(div().h(px(1.)).my(px(4.)).bg(theme::soft_border()))
                .child(
                    self.dropdown_item(
                        "sort-newest",
                        Some(app_icon(ICON_CALENDAR)),
                        "Newest to oldest",
                        self.hosts_sort == HostsSort::NewestFirst,
                        |this, _, _| this.hosts_sort = HostsSort::NewestFirst,
                        cx,
                    )
                    .debug_selector(|| "sort-newest".to_string()),
                )
                .child(
                    self.dropdown_item(
                        "sort-oldest",
                        Some(app_icon(ICON_CALENDAR)),
                        "Oldest to newest",
                        self.hosts_sort == HostsSort::OldestFirst,
                        |this, _, _| this.hosts_sort = HostsSort::OldestFirst,
                        cx,
                    )
                    .debug_selector(|| "sort-oldest".to_string()),
                )
                .into(),
            Some(ToolbarMenu::Avatar) => {
                let invite_email = email.clone();
                let copy_email = email.clone();
                v_flex()
                    .min_w(px(240.))
                    .p(px(6.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::soft_border())
                    .shadow_lg()
                    .child(self.dropdown_item(
                        "avatar-invite",
                        Some(Icon::new(IconName::User)),
                        "Invite team members",
                        false,
                        move |this, _, cx| {
                            let _ = std::process::Command::new("open")
                                .arg(format!(
                                    "mailto:?subject=Join%20me%20on%20TermiRust&body=I%27m%20using%20TermiRust%20at%20{invite_email}"
                                ))
                                .spawn();
                            this.status_message =
                                "Opened your email client to invite a teammate.".to_string();
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(div().h(px(1.)).my(px(4.)).bg(theme::soft_border()))
                    .child(self.dropdown_item(
                        "avatar-email",
                        None,
                        email,
                        false,
                        move |this, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_email.clone()));
                            this.status_message = "Copied email to clipboard.".to_string();
                            cx.notify();
                        },
                        cx,
                    ))
                    .into()
            }
            None => return div().into_any_element(),
        };
        // Toolbar row layout (right-aligned): chrome pr=12, then AvatarPill (52px,
        // ml=4), Sort chevron (45px), Tag (45px), View (45px), all separated by
        // gap=4 in the parent h_flex.
        let right_offset = match menu {
            Some(ToolbarMenu::ViewMode) => px(170.),
            Some(ToolbarMenu::TagFilter) => px(121.),
            Some(ToolbarMenu::Sort) => px(72.),
            Some(ToolbarMenu::Avatar) => px(12.),
            None => return div().into_any_element(),
        };
        div()
            .id("hosts-overlay")
            .absolute()
            .top(px(88.))
            .right(right_offset)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_toolbar_menu = None;
                cx.notify();
            }))
            .child(inner)
            .into_any_element()
    }

    fn toolbar_avatar_pill(&self, _cx: &mut Context<Self>) -> Div {
        h_flex()
            .ml(px(4.))
            .h(px(30.))
            .pl(px(2.))
            .pr(px(6.))
            .gap(px(4.))
            .items_center()
            .rounded(px(999.))
            .border_2()
            .border_color(theme::accent())
            .bg(theme::library_card())
            .child(self.toolbar_avatar_button(_cx))
            .child(
                Icon::new(IconName::Plus)
                    .size(px(12.))
                    .text_color(theme::text_main()),
            )
    }

    fn toolbar_avatar_button(&self, _cx: &mut Context<Self>) -> Stateful<Div> {
        let initials = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("USERNAME").ok())
            .and_then(|name| {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let mut chars = trimmed.chars();
                    let first = chars.next().map(|c| c.to_ascii_uppercase());
                    let second = trimmed
                        .split(|c: char| !c.is_alphanumeric())
                        .nth(1)
                        .and_then(|word| word.chars().next())
                        .map(|c| c.to_ascii_uppercase());
                    match (first, second) {
                        (Some(a), Some(b)) => Some(format!("{a}{b}")),
                        (Some(a), None) => Some(a.to_string()),
                        _ => None,
                    }
                }
            })
            .unwrap_or_else(|| "ME".to_string());

        div()
            .id("library-avatar")
            .size(px(24.))
            .rounded(px(999.))
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::warning())
            .text_size(px(10.))
            .font_semibold()
            .text_color(gpui::white())
            .cursor_pointer()
            .child(initials)
    }

    pub(super) fn render_hosts_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let quick_connect = self.try_quick_connect_from_search(cx);
        let has_quick_connect = quick_connect.is_some();
        let _ = self.current_quick_connect_password(cx);
        let _ = self.filtered_profile_ids(cx).len();
        let selected_host_count = self.selected_host_ids.len();

        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .h(px(44.))
                    .flex_none()
                    .w_full()
                    .px(px(12.))
                    .pt(px(8.))
                    .child(
                        h_flex()
                            .id("hosts-search-bar")
                            .w_full()
                            .h(px(36.))
                            .px(px(12.))
                            .gap(px(8.))
                            .items_center()
                            .rounded(px(8.))
                            .bg(theme::with_alpha(theme::hover(), 0.6))
                            .border_1()
                            .border_color(theme::soft_border())
                            .text_size(px(13.))
                            .text_color(theme::text_main())
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(14.))
                                    .text_color(theme::text_muted()),
                            )
                            .child(
                                div().id("hosts-search-input-wrap").flex_1().child(
                                    Input::new(&self.shell_inputs.host_search)
                                        .appearance(false)
                                        .flex_1(),
                                ),
                            )
                            .child(
                                Button::new("library-quick-connect")
                                    .debug_selector(|| "library-quick-connect".to_string())
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        if has_quick_connect {
                                            theme::ActionTone::Accent
                                        } else {
                                            theme::ActionTone::Neutral
                                        },
                                        cx,
                                    ))
                                    .disabled(!has_quick_connect)
                                    .label("CONNECT")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(qc) = this.try_quick_connect_from_search(cx) {
                                            let password = this.current_quick_connect_password(cx);
                                            this.quick_connect(
                                                qc,
                                                if password.trim().is_empty() {
                                                    None
                                                } else {
                                                    Some(password)
                                                },
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h(px(44.))
                    .px(px(12.))
                    .py(px(6.))
                    .gap(px(6.))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_center()
                            .child(self.render_new_host_split_button(cx))
                            .child(
                                Button::new("library-new-terminal")
                                    .debug_selector(|| "library-new-terminal".to_string())
                                    .xsmall()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .icon(IconName::SquareTerminal)
                                    .label("TERMINAL")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_local_terminal(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(4.))
                            .items_center()
                            .child(
                                Button::new("hosts-select-visible")
                                    .debug_selector(|| "hosts-select-visible".to_string())
                                    .xsmall()
                                    .ghost()
                                    .label("Select Visible")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_all_filtered_hosts(cx);
                                    })),
                            )
                            .when(selected_host_count > 0, |this| {
                                this.child(self.status_badge(
                                    format!("{selected_host_count} selected"),
                                    theme::with_alpha(theme::accent(), 0.16),
                                    theme::accent(),
                                ))
                                .child(
                                    Button::new("hosts-clear-selection")
                                        .debug_selector(|| "hosts-clear-selection".to_string())
                                        .xsmall()
                                        .ghost()
                                        .label("Clear")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.clear_host_batch_selection(cx);
                                        })),
                                )
                                .child(
                                    Button::new("hosts-bulk-star")
                                        .debug_selector(|| "hosts-bulk-star".to_string())
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Star)
                                        .tooltip("Star selected")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_set_selected_hosts_favorite(true, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("hosts-bulk-unstar")
                                        .debug_selector(|| "hosts-bulk-unstar".to_string())
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Star)
                                        .tooltip("Unstar selected")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_set_selected_hosts_favorite(
                                                false, window, cx,
                                            );
                                        })),
                                )
                                .child(
                                    div().id("hosts-bulk-group-input-wrap").child(
                                        Input::new(&self.shell_inputs.bulk_group).w(px(130.)),
                                    ),
                                )
                                .child(
                                    Button::new("hosts-bulk-apply-group")
                                        .debug_selector(|| "hosts-bulk-apply-group".to_string())
                                        .xsmall()
                                        .ghost()
                                        .label("Apply Group")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.bulk_assign_selected_hosts_group(window, cx);
                                        })),
                                )
                            })
                            .child(self.render_view_mode_dropdown(cx))
                            .child(self.render_tag_filter_dropdown(cx))
                            .child(self.render_sort_dropdown(cx))
                            .child(self.render_avatar_dropdown(cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .gap_0()
                    .child(
                        v_flex()
                            .id("hosts-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .gap(px(10.))
                            .px(px(12.))
                            .pt(px(8.))
                            .pb(px(16.))
                            .track_scroll(&self.hosts_list_scroll)
                            .overflow_y_scroll()
                            .when_some(
                                self.render_hosts_onboarding(window, cx),
                                |this, onboarding| this.child(onboarding),
                            )
                            .when_some(self.render_saved_group_cards(cx), |this, cards| {
                                this.child(cards)
                            })
                            .when_some(self.render_recent_hosts_row(cx), |this, row| {
                                this.child(row)
                            })
                            .when(!self.saved.profiles.is_empty(), |this| {
                                this.child(
                                    div()
                                        .pl(px(2.))
                                        .text_size(px(13.))
                                        .font_semibold()
                                        .text_color(theme::text_main())
                                        .child("Hosts"),
                                )
                            })
                            .child(self.render_host_grid(window, cx)),
                    )
                    .when(self.show_editor_panel, |this| {
                        this.child(self.render_editor_side_panel(window, cx))
                    }),
            )
            .child(self.render_hosts_overlays(cx))
    }
}
