use unicode_width::UnicodeWidthChar as _;

use crate::{
    CLI_JSON_SCHEMA_VERSION, CliData, CliError, ErrorCode, JsonFailure, JsonSuccess,
    MAX_RESPONSE_BYTES, RemovalConfirmationKind, SessionWaitConditionData,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    pub json: bool,
    pub terminal_width: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            json: false,
            terminal_width: 80,
        }
    }
}

pub fn render_success(
    data: &CliData,
    warnings: &[String],
    options: RenderOptions,
) -> Result<Vec<u8>, CliError> {
    let bytes = if options.json {
        serde_json::to_vec_pretty(&JsonSuccess {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            ok: true,
            data,
            warnings,
        })
        .map_err(|_| serialization_error())?
    } else {
        render_human(data, warnings, options.terminal_width.max(40)).into_bytes()
    };
    bounded(bytes)
}

pub fn render_failure(error: &CliError, json: bool, terminal_width: usize) -> Vec<u8> {
    let rendered = if json {
        serde_json::to_vec_pretty(&JsonFailure {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            ok: false,
            error,
            warnings: &[],
        })
        .unwrap_or_else(|_| b"{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"operation_failed\",\"message\":\"Unable to serialize the response.\",\"hint\":\"Try the command again.\"},\"warnings\":[]}".to_vec())
    } else {
        let mut output = format!(
            "error[{}]: {}\nhint: {}",
            error.code.as_str(),
            error.message,
            error.hint
        );
        if let Some(revision) = error.current_revision {
            output.push_str(&format!("\ncurrent revision: {revision}"));
        }
        output
            .lines()
            .flat_map(|line| wrap_line(line, terminal_width.max(40)))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    };
    let mut rendered = rendered;
    rendered.push(b'\n');
    rendered
}

fn render_human(data: &CliData, warnings: &[String], width: usize) -> String {
    let mut output = match data {
        CliData::Status(status) => format!(
            "TermiRust CLI status\nCLI version: {}\nJSON schema: {}\nHost protocol: {} to {}\nStore: {}\nHost control: {}",
            status.cli_version,
            status.json_schema_version,
            status.protocol_minimum,
            status.protocol_maximum,
            status.store,
            status.host_control
        ),
        CliData::Projects(data) => render_records(
            "Projects",
            data.projects.iter().map(|project| {
                vec![
                    ("ID", project.id.clone()),
                    ("Name", project.name.clone()),
                    ("Status", project.status.clone()),
                    ("Revision", project.revision.to_string()),
                ]
            }),
        ),
        CliData::Presets(data) => {
            let mut text = format!("Presets for project {}", data.project_id);
            text.push_str(&render_records(
                "",
                data.presets.iter().map(|preset| {
                    vec![
                        ("ID", preset.id.clone()),
                        ("Label", preset.label.clone()),
                        ("Enabled", if preset.enabled { "yes" } else { "no" }.into()),
                        (
                            "Favorite",
                            if preset.favorite { "yes" } else { "no" }.into(),
                        ),
                        ("Policy", preset.permission_policy.clone()),
                        ("Risk", preset.risk.clone()),
                        ("Revision", preset.revision.to_string()),
                    ]
                }),
            ));
            text
        }
        CliData::Sessions(data) => {
            render_records("Sessions", data.sessions.iter().map(session_fields))
        }
        CliData::Session(data) => {
            render_records("Session", std::iter::once(session_fields(&data.session)))
        }
        CliData::Wait(data) => {
            let condition = match &data.condition {
                SessionWaitConditionData::Lifecycle { state } => {
                    format!("lifecycle={state}")
                }
                SessionWaitConditionData::Activity { state } => format!("activity={state}"),
            };
            let mut text = format!("Wait condition matched: {condition}");
            text.push_str(&render_records(
                "\nSession",
                std::iter::once(session_fields(&data.session)),
            ));
            text
        }
        CliData::Input(data) => render_records(
            "Session input",
            std::iter::once(vec![
                ("Session", data.session_id.clone()),
                ("Accepted bytes", data.accepted_bytes.to_string()),
                ("Applied", if data.applied { "yes" } else { "no" }.into()),
            ]),
        ),
        CliData::Resize(data) => render_records(
            "Session resize",
            std::iter::once(vec![
                ("Session", data.session_id.clone()),
                ("Columns", data.columns.to_string()),
                ("Rows", data.rows.to_string()),
                ("Applied", if data.applied { "yes" } else { "no" }.into()),
            ]),
        ),
        CliData::Mutation(data) => {
            let mut text = format!("Outcome: {}", data.outcome);
            text.push_str(&render_records(
                "\nSession",
                std::iter::once(session_fields(&data.session)),
            ));
            text
        }
        CliData::RemovalPreview(data) => {
            let confirmation = match data.confirmation {
                RemovalConfirmationKind::SessionTitle => "the exact Session title",
                RemovalConfirmationKind::Remove => "REMOVE",
            };
            let mut text = render_records(
                "Session removal preview",
                std::iter::once(vec![
                    ("ID", data.session.id.clone()),
                    ("Title", data.session.title.clone()),
                    ("State", data.session.state.clone()),
                    (
                        "Archived",
                        if data.session.archived { "yes" } else { "no" }.into(),
                    ),
                    ("Session revision", data.session.revision.to_string()),
                    ("Repository revision", data.repository_revision.to_string()),
                    ("Metadata bytes", data.metadata_bytes.to_string()),
                    ("Journal bytes", data.journal_bytes.to_string()),
                    ("Transcript bytes", data.transcript_bytes.to_string()),
                    ("Artifact bytes", data.artifact_bytes.to_string()),
                    ("Total bytes", data.total_bytes.to_string()),
                    ("Files", data.file_count.to_string()),
                    ("Confirmation", confirmation.into()),
                    ("Preview token", data.preview_token.clone()),
                ]),
            );
            text.push_str(
                "\n\nNo data was changed. To commit, pipe the requested confirmation to stdin and rerun with this preview token, --yes, and --confirmation-stdin.",
            );
            text
        }
        CliData::ControllerSsh(data) => {
            let capabilities = if data.capabilities.is_empty() {
                "none".to_string()
            } else {
                data.capabilities.join(", ")
            };
            let mut fields = vec![
                ("Operation", data.operation.clone()),
                ("Route", data.route_state.clone()),
                ("Target", data.target_label.clone()),
                ("SSH host key", data.ssh_host_key.clone()),
                ("Capabilities", capabilities),
            ];
            if let Some(value) = &data.host_fingerprint_suffix {
                fields.push(("Host fingerprint suffix", value.clone()));
            }
            if let Some(value) = data.session_generation {
                fields.push(("Session generation", value.to_string()));
            }
            if let Some(value) = &data.writer_lease {
                fields.push(("Writer lease", value.clone()));
            }
            if let Some(value) = data.reconnect_attempt {
                fields.push(("Reconnect attempt", value.to_string()));
            }
            if let Some(value) = data.reconnect_deadline_millis {
                fields.push(("Reconnect deadline", value.to_string()));
            }
            let mut text = render_records("Remote Controller", std::iter::once(fields));
            if !data.sessions.is_empty() {
                text.push_str(&render_records(
                    "\nSessions",
                    data.sessions.iter().map(|session| {
                        vec![
                            ("ID", session.id.clone()),
                            ("Title", session.title.clone()),
                            ("Lifecycle", session.lifecycle.clone()),
                            ("Activity", session.activity.clone()),
                            (
                                "Generation",
                                session
                                    .occupant_generation
                                    .map_or_else(|| "-".into(), |value| value.to_string()),
                            ),
                            ("Last output", session.last_output_sequence.to_string()),
                            (
                                "Writer active",
                                if session.has_writer { "yes" } else { "no" }.into(),
                            ),
                            ("Unread", if session.unread { "yes" } else { "no" }.into()),
                        ]
                    }),
                ));
            }
            text
        }
        CliData::Help(data) => {
            let mut text = "TermiRust one-shot local CLI\n\nCommands:".to_string();
            for command in &data.commands {
                text.push_str("\n  ");
                text.push_str(command);
            }
            text.push_str("\n\nSafety:\n  ");
            text.push_str(&data.safety);
            text.push_str("\n\nExit codes:");
            for exit in &data.exit_codes {
                text.push_str("\n  ");
                text.push_str(exit);
            }
            text
        }
    };
    for warning in warnings {
        output.push_str("\nwarning: ");
        output.push_str(warning);
    }
    let mut wrapped = output
        .lines()
        .flat_map(|line| wrap_line(line, width))
        .collect::<Vec<_>>()
        .join("\n");
    wrapped.push('\n');
    wrapped
}

fn session_fields(session: &crate::SessionView) -> Vec<(&'static str, String)> {
    vec![
        ("ID", session.id.clone()),
        ("Project", session.project_id.clone()),
        (
            "Group",
            session.group_id.clone().unwrap_or_else(|| "-".into()),
        ),
        (
            "Preset",
            session.preset_id.clone().unwrap_or_else(|| "-".into()),
        ),
        ("Title", session.title.clone()),
        ("State", session.state.clone()),
        ("Activity", session.activity.clone()),
        ("Unread", if session.unread { "yes" } else { "no" }.into()),
        (
            "Archived",
            if session.archived { "yes" } else { "no" }.into(),
        ),
        ("Revision", session.revision.to_string()),
    ]
}

fn render_records(
    heading: &str,
    records: impl Iterator<Item = Vec<(&'static str, String)>>,
) -> String {
    let records = records.collect::<Vec<_>>();
    let mut output = heading.to_string();
    if records.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("No records.");
        return output;
    }
    for (index, record) in records.into_iter().enumerate() {
        if !output.is_empty() || index > 0 {
            output.push('\n');
        }
        if index > 0 {
            output.push('\n');
        }
        for (field, value) in record {
            output.push_str(field);
            output.push_str(": ");
            output.push_str(&value);
            output.push('\n');
        }
        output.pop();
    }
    output
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if display_width(line) <= width || line.trim().is_empty() {
        return vec![line.to_string()];
    }
    let indent = line
        .chars()
        .take_while(|character| character.is_whitespace())
        .count();
    let continuation = " ".repeat(indent.min(width.saturating_sub(1)));
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in line.split_whitespace() {
        let word_width = display_width(word);
        let separator = usize::from(!current.is_empty());
        if current_width + separator + word_width > width && !current.is_empty() {
            lines.push(current);
            current = continuation.clone();
            current_width = display_width(&current);
        }
        if !current.is_empty() && !current.ends_with(' ') {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
        .into_iter()
        .flat_map(|line| hard_wrap_line(&line, width, &continuation))
        .collect()
}

fn hard_wrap_line(line: &str, width: usize, continuation: &str) -> Vec<String> {
    if display_width(line) <= width {
        return vec![line.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in line.chars() {
        let character_width = character.width().unwrap_or(0);
        if current_width + character_width > width && !current.is_empty() {
            lines.push(current);
            current = continuation.to_string();
            current_width = display_width(&current);
        }
        current.push(character);
        current_width += character_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, CliError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        Err(CliError::new(
            ErrorCode::ResourceLimit,
            "command response exceeds the one MiB limit",
            "Narrow the query with project, group, state, or archived filters.",
        ))
    } else {
        Ok(bytes)
    }
}

fn serialization_error() -> CliError {
    CliError::new(
        ErrorCode::OperationFailed,
        "unable to serialize the command response",
        "Try the command again. No mutation was rolled back.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliData, HelpData};

    #[test]
    fn human_help_wraps_to_narrow_plain_text_without_ansi() {
        let rendered = render_success(
            &CliData::Help(HelpData {
                commands: vec!["session launch --project <id> --preset <id> [--group <id>]".into()],
                safety: "Mutations never retry a conflict and stop requires --yes.".into(),
                exit_codes: vec!["0 success".into()],
            }),
            &[],
            RenderOptions {
                json: false,
                terminal_width: 40,
            },
        )
        .unwrap();
        let text = String::from_utf8(rendered).unwrap();
        assert!(text.lines().all(|line| display_width(line) <= 40));
        assert!(!text.contains("\u{1b}["));
    }

    #[test]
    fn human_output_preserves_and_wraps_long_unbroken_unicode_text() {
        let long = "界".repeat(80);
        let lines = wrap_line(&format!("Title: {long}"), 40);
        assert!(lines.iter().all(|line| display_width(line) <= 40));
        assert_eq!(lines.join("").replace(' ', ""), format!("Title:{long}"));
    }

    #[test]
    fn errors_wrap_and_serialized_responses_enforce_the_byte_cap() {
        let error = CliError::new(
            ErrorCode::Validation,
            "x".repeat(200),
            "Use the desktop application to inspect the current local state.",
        );
        let human = String::from_utf8(render_failure(&error, false, 40)).unwrap();
        assert!(human.lines().all(|line| display_width(line) <= 40));

        let huge = CliData::Help(HelpData {
            commands: vec!["x".repeat(MAX_RESPONSE_BYTES + 1)],
            safety: String::new(),
            exit_codes: Vec::new(),
        });
        assert_eq!(
            render_success(
                &huge,
                &[],
                RenderOptions {
                    json: true,
                    terminal_width: 80,
                },
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimit
        );
    }
}
