use std::path::Path;

use gpui::{
    ClipboardItem, Context, Div, InteractiveElement as _, ParentElement as _, Styled as _, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Disableable as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use termirust_cli::{CLI_JSON_SCHEMA_VERSION, cli_installation_status};

use super::TermiRustApp;
use crate::ui::localization;
use crate::ui::theme;

const CLI_EXAMPLES: [&str; 3] = [
    "termirust-cli status --json",
    "termirust-cli project list --json",
    "termirust-cli session list --json",
];
const CLI_EXAMPLE_SELECTORS: [&str; 3] = [
    "settings-cli-copy-example-status",
    "settings-cli-copy-example-projects",
    "settings-cli-copy-example-sessions",
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct CliStatusPresentation {
    available: bool,
    host_available: bool,
    path: Option<String>,
    schema_version: u16,
    protocol_version: Option<String>,
}

impl CliStatusPresentation {
    fn inspect(current_executable: &Path) -> Self {
        let status = cli_installation_status(current_executable);
        Self {
            available: status.available,
            host_available: status.host_available,
            path: Some(status.path.display().to_string()),
            schema_version: status.json_schema_version,
            protocol_version: Some(status.protocol_version),
        }
    }

    fn unavailable() -> Self {
        Self {
            available: false,
            host_available: false,
            path: None,
            schema_version: CLI_JSON_SCHEMA_VERSION,
            protocol_version: None,
        }
    }
}

impl TermiRustApp {
    pub(super) fn render_cli_settings_card(&self, cx: &Context<Self>) -> Div {
        let status = std::env::current_exe()
            .ok()
            .as_deref()
            .map(CliStatusPresentation::inspect)
            .unwrap_or_else(CliStatusPresentation::unavailable);
        let path = status.path.clone();
        let state_label = match (status.available, status.host_available) {
            (true, true) => localization::cli_settings_state_available(),
            (true, false) => localization::cli_settings_state_host_unavailable(),
            (false, _) => localization::cli_settings_state_unavailable(),
        };
        let fully_available = status.available && status.host_available;
        let schema = localization::cli_settings_schema(status.schema_version.to_string());
        let protocol = status.protocol_version.map_or_else(
            localization::cli_settings_protocol_unavailable,
            localization::cli_settings_protocol,
        );
        let display_path = status.path.map_or_else(
            localization::cli_settings_path_unavailable,
            localization::cli_settings_path_value,
        );

        self.settings_section_card(
            localization::cli_settings_title(),
            localization::cli_settings_description(),
            v_flex()
                .gap_4()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .flex_wrap()
                        .child(self.status_badge(
                            state_label,
                            if fully_available {
                                theme::with_alpha(theme::success(), 0.12)
                            } else {
                                theme::with_alpha(theme::danger(), 0.12)
                            },
                            if fully_available {
                                theme::success()
                            } else {
                                theme::danger()
                            },
                        ))
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(schema),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(protocol),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .font_medium()
                                .text_color(theme::text_main())
                                .child(localization::cli_settings_installed_path()),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(theme::CONTROL_RADIUS))
                                        .bg(theme::hover())
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_main())
                                        .child(display_path),
                                )
                                .child(
                                    Button::new("settings-cli-copy-path")
                                        .debug_selector(|| "settings-cli-copy-path".to_string())
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .icon(IconName::Copy)
                                        .label(localization::cli_settings_copy_path())
                                        .disabled(path.is_none())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(path) = path.as_ref() {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    path.clone(),
                                                ));
                                                this.status_message =
                                                    localization::cli_settings_path_copied();
                                                this.error_message.clear();
                                                cx.notify();
                                            }
                                        })),
                                ),
                        ),
                )
                .child(self.settings_divider())
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .font_medium()
                                .text_color(theme::text_main())
                                .child(localization::cli_settings_examples()),
                        )
                        .child(
                            h_flex().gap_2().flex_wrap().children(
                                CLI_EXAMPLES
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, example)| {
                                        Button::new(("settings-cli-copy-example", index))
                                            .debug_selector(move || {
                                                CLI_EXAMPLE_SELECTORS[index].to_string()
                                            })
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Neutral,
                                                cx,
                                            ))
                                            .icon(IconName::Copy)
                                            .label(localization::cli_settings_example(example))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    example.to_string(),
                                                ));
                                                this.status_message =
                                                    localization::cli_settings_example_copied();
                                                this.error_message.clear();
                                                cx.notify();
                                            }))
                                    }),
                            ),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::cli_settings_help_hint()),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn executable_name(stem: &str) -> OsString {
        let mut name = OsString::from(stem);
        name.push(std::env::consts::EXE_SUFFIX);
        name
    }

    #[test]
    fn cli_status_reports_missing_and_installed_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join(executable_name("termirust"));
        let cli = temp.path().join(executable_name("termirust-cli"));
        let host = temp.path().join(executable_name("termirust-session-host"));
        let cli_display = cli.display().to_string();

        let missing = CliStatusPresentation::inspect(&app);
        assert!(!missing.available);
        assert!(!missing.host_available);
        assert_eq!(missing.schema_version, 1);
        assert_eq!(missing.path.as_deref(), Some(cli_display.as_str()));

        std::fs::write(&cli, b"test binary").unwrap();
        let installed = CliStatusPresentation::inspect(&app);
        assert!(installed.available);
        assert!(!installed.host_available);
        assert_eq!(installed.path.as_deref(), Some(cli_display.as_str()));
        assert_eq!(installed.protocol_version.as_deref(), Some("1.0"));

        std::fs::write(host, b"test host").unwrap();
        assert!(CliStatusPresentation::inspect(&app).host_available);
    }

    #[test]
    fn cli_examples_are_local_bounded_and_json_explicit() {
        assert_eq!(CLI_EXAMPLES.len(), 3);
        assert!(CLI_EXAMPLES.iter().all(|example| example.len() < 100));
        assert!(
            CLI_EXAMPLES
                .iter()
                .all(|example| example.contains("--json"))
        );
        assert!(
            CLI_EXAMPLES
                .iter()
                .all(|example| !example.contains("http") && !example.contains("socket"))
        );
    }
}
