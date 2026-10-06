//! Settings → About: which Multiplex this is, for a person checking for an update or filing a
//! bug.

use gpui::{
    ClipboardItem, Context, Div, InteractiveElement as _, ParentElement, Styled, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::Button;
use gpui_component::{Sizable as _, h_flex, v_flex};

use super::MultiplexApp;
use crate::ui::{localization, theme};

const SOURCE_URL: &str = "https://github.com/millionrust/multiplex";
const LICENSE: &str = "MIT OR Apache-2.0";

/// The commit the build script found, when it was built from a Git checkout.
fn build_commit() -> &'static str {
    option_env!("MULTIPLEX_BUILD_COMMIT").unwrap_or("unknown")
}

fn platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// One line with everything a bug report needs.
fn details() -> String {
    format!(
        "Multiplex {} ({}) on {}",
        env!("CARGO_PKG_VERSION"),
        build_commit(),
        platform()
    )
}

impl MultiplexApp {
    pub(super) fn render_about_settings_card(&self, cx: &Context<Self>) -> Div {
        let row = |id: &'static str, label: String, value: String, mono: bool| {
            h_flex()
                .w_full()
                .min_w_0()
                .flex_none()
                .items_start()
                .gap_3()
                .py_1()
                .border_t_1()
                .border_color(theme::soft_border())
                .child(
                    div()
                        .flex_none()
                        .w(px(theme::SPACE_9 + theme::SPACE_8))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(label),
                )
                .child(
                    div()
                        .debug_selector(move || id.to_owned())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_right()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_main())
                        .when(mono, |this| {
                            this.font_family(theme::current_design_tokens().font_mono_family().0)
                        })
                        .child(value),
                )
        };
        self.settings_section_card(
            "",
            "",
            v_flex()
                .debug_selector(|| "about-details".to_owned())
                .w_full()
                .min_w_0()
                .gap_1()
                .child(row(
                    "about-version-value",
                    localization::about_version_label(),
                    env!("CARGO_PKG_VERSION").to_owned(),
                    false,
                ))
                .child(row(
                    "about-build-value",
                    localization::about_build_label(),
                    build_commit().to_owned(),
                    true,
                ))
                .child(row(
                    "about-platform-value",
                    localization::about_platform_label(),
                    platform(),
                    false,
                ))
                .child(row(
                    "about-license-value",
                    localization::about_license_label(),
                    LICENSE.to_owned(),
                    false,
                ))
                .child(row(
                    "about-source-value",
                    localization::about_source_label(),
                    SOURCE_URL.to_owned(),
                    false,
                ))
                .child(self.render_update_settings(cx))
                .child(
                    h_flex().pt_2().gap_2().child(
                        Button::new("about-copy-details")
                            .debug_selector(|| "about-copy-details".to_string())
                            .small()
                            .label(localization::about_copy_action())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(details()));
                                this.status_message = localization::about_copied();
                                cx.notify();
                            })),
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_details_name_the_version_and_platform() {
        let details = details();
        assert!(details.starts_with(&format!("Multiplex {}", env!("CARGO_PKG_VERSION"))));
        assert!(details.ends_with(&platform()));
    }
}
