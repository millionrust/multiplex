//! SFTP page renderers (local file browser + connect-host empty state and
//! host picker). All methods are part of the `TermiRustApp` impl.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::ui::app::TermiRustApp;
use crate::ui::sftp_local::read_local_dir;
use crate::ui::theme;
use crate::ui::util::{format_modified_time, format_size};

impl TermiRustApp {
    pub(super) fn render_sftp_view(&self, cx: &mut Context<Self>) -> Div {
        gpui::div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .size_full()
            .bg(theme::library_bg())
            .child(self.render_sftp_local_pane(cx))
            .child(gpui::div().w(px(1.)).h_full().bg(theme::soft_border()))
            .child(if self.sftp_show_host_picker {
                self.render_sftp_host_picker(cx)
            } else {
                self.render_sftp_remote_empty(cx)
            })
    }

    fn render_sftp_local_pane(&self, cx: &mut Context<Self>) -> Div {
        let mut entries = read_local_dir(&self.sftp_local_path);
        let filter_value = self
            .shell_inputs
            .sftp_local_filter
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        if !filter_value.is_empty() {
            entries.retain(|e| e.name.to_ascii_lowercase().contains(&filter_value));
        }
        let path_segments: Vec<String> = self
            .sftp_local_path
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(48.))
                    .px(px(16.))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                gpui::div()
                                    .size(px(22.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::with_alpha(theme::accent(), 0.2))
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .size(px(13.))
                                            .text_color(theme::accent()),
                                    ),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(13.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child("Local"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .items_center()
                            .child(
                                h_flex()
                                    .id("sftp-filter-toggle")
                                    .gap(px(4.))
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme::text_main()))
                                    .child(
                                        Icon::new(IconName::Search)
                                            .size(px(12.))
                                            .text_color(theme::text_muted()),
                                    )
                                    .child(
                                        gpui::div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child("Filter"),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.sftp_local_filter_visible =
                                            !this.sftp_local_filter_visible;
                                        if this.sftp_local_filter_visible {
                                            this.shell_inputs
                                                .sftp_local_filter
                                                .update(cx, |state, cx| state.focus(window, cx));
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                h_flex()
                                    .id("sftp-actions-open")
                                    .gap(px(4.))
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme::text_main()))
                                    .child(
                                        gpui::div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child("Actions"),
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(px(11.))
                                            .text_color(theme::text_muted()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = this.sftp_local_path.clone();
                                        let _ =
                                            std::process::Command::new("open").arg(&path).spawn();
                                        this.status_message =
                                            format!("Opened {} in Finder.", path.display());
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(36.))
                    .px(px(16.))
                    .items_center()
                    .gap(px(6.))
                    .child(
                        gpui::div()
                            .id("sftp-back")
                            .size(px(22.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                Icon::new(IconName::ArrowLeft)
                                    .size(px(13.))
                                    .text_color(theme::text_muted()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(parent) = this.sftp_local_path.parent() {
                                    this.sftp_local_path = parent.to_path_buf();
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size(px(13.))
                            .text_color(theme::with_alpha(theme::text_muted(), 0.5)),
                    )
                    .children(path_segments.iter().enumerate().flat_map(|(idx, seg)| {
                        let is_last = idx == path_segments.len() - 1;
                        let mut items: Vec<AnyElement> = Vec::new();
                        items.push(
                            h_flex()
                                .gap(px(4.))
                                .items_center()
                                .child(
                                    Icon::new(IconName::Folder)
                                        .size(px(12.))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    gpui::div()
                                        .text_size(px(12.))
                                        .text_color(theme::text_main())
                                        .child(seg.clone()),
                                )
                                .into_any_element(),
                        );
                        if !is_last {
                            items.push(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(11.))
                                    .text_color(theme::text_muted())
                                    .into_any_element(),
                            );
                        }
                        items
                    })),
            )
            .when(self.sftp_local_filter_visible, |this| {
                this.child(
                    gpui::div()
                        .px(px(16.))
                        .pb(px(8.))
                        .child(Input::new(&self.shell_inputs.sftp_local_filter).xsmall()),
                )
            })
            .child(
                h_flex()
                    .h(px(32.))
                    .px(px(16.))
                    .items_center()
                    .border_t_1()
                    .border_color(theme::soft_border())
                    .border_b_1()
                    .child(
                        gpui::div()
                            .w(px(280.))
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Name"),
                    )
                    .child(
                        gpui::div()
                            .w(px(160.))
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Date Modified"),
                    )
                    .child(
                        gpui::div()
                            .w(px(80.))
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Size"),
                    )
                    .child(
                        gpui::div()
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child("Kind"),
                    ),
            )
            .child(
                v_flex()
                    .id("sftp-local-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(entries.into_iter().enumerate().map(|(idx, entry)| {
                        let path = self.sftp_local_path.join(&entry.name);
                        let is_dir = entry.is_dir;
                        let date_str = entry
                            .modified
                            .map(format_modified_time)
                            .unwrap_or_else(|| "--".to_string());
                        let size_str = if is_dir {
                            "--".to_string()
                        } else {
                            format_size(entry.size)
                        };
                        let kind_str = if is_dir { "folder" } else { "file" };
                        let entry_clone = entry.name.clone();
                        h_flex()
                            .id(("sftp-row", idx))
                            .h(px(36.))
                            .px(px(16.))
                            .items_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.4)))
                            .child(
                                h_flex()
                                    .w(px(280.))
                                    .gap(px(8.))
                                    .items_center()
                                    .child(
                                        Icon::new(if is_dir {
                                            IconName::Folder
                                        } else {
                                            IconName::File
                                        })
                                        .size(px(14.))
                                        .text_color(theme::accent()),
                                    )
                                    .child(
                                        gpui::div()
                                            .text_size(px(12.))
                                            .text_color(theme::text_main())
                                            .child(entry_clone),
                                    ),
                            )
                            .child(
                                gpui::div()
                                    .w(px(160.))
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(date_str),
                            )
                            .child(
                                gpui::div()
                                    .w(px(80.))
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(size_str),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(kind_str),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_dir {
                                    this.sftp_local_path = path.clone();
                                    cx.notify();
                                }
                            }))
                    })),
            )
    }

    fn render_sftp_remote_empty(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(
                gpui::div()
                    .size(px(64.))
                    .rounded(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::with_alpha(theme::hover(), 0.7))
                    .child(
                        Icon::new(IconName::Folder)
                            .size(px(28.))
                            .text_color(theme::text_main()),
                    ),
            )
            .child(
                gpui::div()
                    .text_size(px(15.))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child("Connect to host"),
            )
            .child(
                gpui::div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .max_w(px(240.))
                    .child("Start by connecting to a saved host to manage your files with SFTP."),
            )
            .child(
                Button::new("sftp-select-host")
                    .small()
                    .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                    .label("Select host")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sftp_show_host_picker = true;
                        cx.notify();
                    })),
            )
    }

    fn render_sftp_host_picker(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(px(48.))
                    .px(px(16.))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(
                                gpui::div()
                                    .id("sftp-picker-back")
                                    .size(px(22.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.7)))
                                    .child(
                                        Icon::new(IconName::ArrowLeft)
                                            .size(px(13.))
                                            .text_color(theme::text_main()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sftp_show_host_picker = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(
                                        gpui::div()
                                            .text_size(px(13.))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child("Select Host"),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(px(4.))
                                            .items_center()
                                            .child(
                                                gpui::div()
                                                    .text_size(px(11.))
                                                    .text_color(theme::text_muted())
                                                    .child("Vaults"),
                                            )
                                            .child(
                                                Icon::new(IconName::ChevronDown)
                                                    .size(px(10.))
                                                    .text_color(theme::text_muted()),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .h(px(26.))
                            .px(px(8.))
                            .gap(px(6.))
                            .items_center()
                            .rounded(px(6.))
                            .bg(theme::with_alpha(theme::accent(), 0.15))
                            .border_1()
                            .border_color(theme::accent())
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(11.))
                                    .text_color(theme::accent()),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(11.))
                                    .font_semibold()
                                    .text_color(theme::accent())
                                    .child("Local"),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(36.))
                    .mx(px(16.))
                    .px(px(10.))
                    .gap(px(6.))
                    .items_center()
                    .rounded(px(6.))
                    .bg(theme::library_bg())
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(13.))
                            .text_color(theme::text_muted()),
                    )
                    .child(
                        gpui::div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child("Search"),
                    ),
            )
            .child(
                v_flex()
                    .px(px(16.))
                    .pt(px(14.))
                    .gap(px(6.))
                    .child(
                        gpui::div()
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(theme::text_muted())
                            .child("Hosts"),
                    )
                    .children(
                        self.saved
                            .profiles
                            .iter()
                            .enumerate()
                            .map(|(idx, profile)| {
                                let profile_id = profile.id.clone();
                                let display_name = profile.display_name();
                                let proto_summary =
                                    format!("ssh, {}@{}", profile.username, profile.endpoint());
                                h_flex()
                                    .id(("sftp-host", idx))
                                    .h(px(46.))
                                    .gap(px(10.))
                                    .items_center()
                                    .px(px(8.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
                                    .child(
                                        gpui::div()
                                            .size(px(34.))
                                            .rounded(px(6.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(theme::library_card())
                                            .child(
                                                Icon::new(IconName::SquareTerminal)
                                                    .size(px(15.))
                                                    .text_color(theme::accent()),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap(px(2.))
                                            .child(
                                                gpui::div()
                                                    .text_size(px(12.))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(display_name),
                                            )
                                            .child(
                                                gpui::div()
                                                    .text_size(px(10.))
                                                    .text_color(theme::text_muted())
                                                    .child(proto_summary),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.sftp_show_host_picker = false;
                                        this.open_connect_dialog_tab(&profile_id, window, cx);
                                    }))
                            }),
                    ),
            )
    }
}
