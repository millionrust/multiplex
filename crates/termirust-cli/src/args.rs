use std::collections::HashSet;
use std::fmt;

use termirust_client::{
    SshControllerTarget, SshControllerTargetId, ValidatedDnsOrIp, ValidatedUser,
};
use termirust_domain::{
    GroupId, HostedSessionId, HostedSessionState, OccupantGeneration, OutputSequence, PresetId,
    ProjectId, Revision,
};
use uuid::Uuid;

use crate::{CliError, ErrorCode};

pub const MAX_ARG_COUNT: usize = 128;
pub const MAX_ARG_BYTES: usize = 4 * 1024;
pub const MAX_TOTAL_ARG_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub json: bool,
    pub command: CliCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    Help,
    Status,
    ProjectList,
    PresetList {
        project_id: ProjectId,
    },
    SessionList(SessionListFilter),
    SessionShow {
        session_id: HostedSessionId,
    },
    SessionLaunch {
        project_id: ProjectId,
        preset_id: PresetId,
        group_id: Option<GroupId>,
    },
    SessionStop {
        session_id: HostedSessionId,
        expected_revision: Option<Revision>,
        confirmed: bool,
    },
    SessionArchive {
        session_id: HostedSessionId,
        expected_revision: Option<Revision>,
    },
    SessionRestore {
        session_id: HostedSessionId,
        expected_revision: Option<Revision>,
    },
    SessionRemove {
        session_id: HostedSessionId,
        expected_revision: Option<Revision>,
        preview_token: Option<String>,
        confirmed: bool,
        confirmation_stdin: bool,
        confirmation: Option<RemovalConfirmation>,
    },
    ControllerSsh(ControllerSshCommand),
}

impl CliCommand {
    pub fn with_removal_confirmation(
        mut self,
        confirmation: RemovalConfirmation,
    ) -> Result<Self, CliError> {
        let Self::SessionRemove {
            confirmation_stdin,
            confirmation: current,
            ..
        } = &mut self
        else {
            return Err(usage("confirmation input is valid only for session remove"));
        };
        if !*confirmation_stdin || current.is_some() {
            return Err(usage(
                "session removal confirmation input was not requested",
            ));
        }
        *current = Some(confirmation);
        Ok(self)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RemovalConfirmation(String);

impl RemovalConfirmation {
    pub fn new(value: String) -> Result<Self, CliError> {
        if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control) {
            return Err(CliError::new(
                ErrorCode::Validation,
                "session removal confirmation from stdin is invalid",
                "Provide exactly one non-empty line of at most 256 Unicode characters.",
            ));
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RemovalConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerSshCommand {
    pub target: SshControllerTarget,
    pub action: ControllerSshAction,
    pub allow_interaction: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControllerSshAction {
    Pair,
    Sessions,
    Attach {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        from_sequence: OutputSequence,
        columns: u16,
        rows: u16,
        request_control: bool,
    },
    Input {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
    },
    Resize {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        columns: u16,
        rows: u16,
    },
    Approval {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        approval_id: Uuid,
        decision: ApprovalDecision,
    },
    Detach {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionListFilter {
    pub project_id: Option<ProjectId>,
    pub group_id: Option<GroupId>,
    pub state: Option<HostedSessionState>,
    pub archived_only: bool,
}

pub fn parse_args(arguments: Vec<String>) -> Result<Invocation, CliError> {
    validate_bounds(&arguments)?;
    let mut json = false;
    let mut filtered = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if argument == "--json" {
            if json {
                return Err(usage("duplicate option --json"));
            }
            json = true;
        } else {
            filtered.push(argument);
        }
    }
    if filtered.is_empty() || filtered == ["--help"] || filtered == ["-h"] {
        return Ok(Invocation {
            json,
            command: CliCommand::Help,
        });
    }

    let command = match filtered.as_slice() {
        [command] if command == "status" => CliCommand::Status,
        [scope, action] if scope == "project" && action == "list" => CliCommand::ProjectList,
        [scope, action, rest @ ..] if scope == "preset" && action == "list" => {
            let options = parse_options(rest, &["--project"], &[])?;
            CliCommand::PresetList {
                project_id: parse_id(required(&options, "--project")?, "project")?,
            }
        }
        [scope, action, rest @ ..] if scope == "session" && action == "list" => {
            let options =
                parse_options(rest, &["--project", "--group", "--state"], &["--archived"])?;
            CliCommand::SessionList(SessionListFilter {
                project_id: optional_id(&options, "--project", "project")?,
                group_id: optional_id(&options, "--group", "group")?,
                state: options.value("--state").map(parse_state).transpose()?,
                archived_only: options.flag("--archived"),
            })
        }
        [scope, action, session] if scope == "session" && action == "show" => {
            CliCommand::SessionShow {
                session_id: parse_id(session, "session")?,
            }
        }
        [scope, action, rest @ ..] if scope == "session" && action == "launch" => {
            let options = parse_options(rest, &["--project", "--preset", "--group"], &[])?;
            CliCommand::SessionLaunch {
                project_id: parse_id(required(&options, "--project")?, "project")?,
                preset_id: parse_id(required(&options, "--preset")?, "preset")?,
                group_id: optional_id(&options, "--group", "group")?,
            }
        }
        [scope, action, session, rest @ ..] if scope == "session" && action == "stop" => {
            let options = parse_options(rest, &["--expected-revision"], &["--yes"])?;
            CliCommand::SessionStop {
                session_id: parse_id(session, "session")?,
                expected_revision: optional_revision(&options)?,
                confirmed: options.flag("--yes"),
            }
        }
        [scope, action, session, rest @ ..] if scope == "session" && action == "archive" => {
            let options = parse_options(rest, &["--expected-revision"], &[])?;
            CliCommand::SessionArchive {
                session_id: parse_id(session, "session")?,
                expected_revision: optional_revision(&options)?,
            }
        }
        [scope, action, session, rest @ ..] if scope == "session" && action == "restore" => {
            let options = parse_options(rest, &["--expected-revision"], &[])?;
            CliCommand::SessionRestore {
                session_id: parse_id(session, "session")?,
                expected_revision: optional_revision(&options)?,
            }
        }
        [scope, action, session, rest @ ..] if scope == "session" && action == "remove" => {
            let options = parse_options(
                rest,
                &["--expected-revision", "--preview-token"],
                &["--yes", "--confirmation-stdin"],
            )?;
            let preview_token = options.value("--preview-token").map(str::to_string);
            let confirmed = options.flag("--yes");
            let confirmation_stdin = options.flag("--confirmation-stdin");
            let commit_options = usize::from(preview_token.is_some())
                + usize::from(confirmed)
                + usize::from(confirmation_stdin);
            if commit_options != 0 && commit_options != 3 {
                return Err(usage(
                    "session removal commit requires --preview-token, --yes, and --confirmation-stdin together",
                ));
            }
            CliCommand::SessionRemove {
                session_id: parse_id(session, "session")?,
                expected_revision: optional_revision(&options)?,
                preview_token,
                confirmed,
                confirmation_stdin,
                confirmation: None,
            }
        }
        [scope, route, rest @ ..] if scope == "controller" && route == "ssh" => {
            CliCommand::ControllerSsh(parse_controller_ssh(rest, !json)?)
        }
        _ => return Err(usage("unknown or incomplete command")),
    };
    Ok(Invocation { json, command })
}

fn parse_controller_ssh(
    arguments: &[String],
    allow_interaction: bool,
) -> Result<ControllerSshCommand, CliError> {
    const ACTIONS: &[&str] = &[
        "pair", "sessions", "attach", "input", "resize", "approval", "detach",
    ];
    let action_index = arguments
        .iter()
        .position(|argument| ACTIONS.contains(&argument.as_str()))
        .ok_or_else(|| usage("controller SSH action is missing"))?;
    let target_options = parse_options(
        &arguments[..action_index],
        &["--host", "--user", "--port"],
        &[],
    )?;
    let host = ValidatedDnsOrIp::parse(required(&target_options, "--host")?)
        .map_err(|_| usage("SSH host must be a DNS name or IP address"))?;
    let user = target_options
        .value("--user")
        .map(ValidatedUser::parse)
        .transpose()
        .map_err(|_| usage("SSH user contains unsupported characters"))?;
    let port = target_options
        .value("--port")
        .map(|value| {
            value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| usage("SSH port must be an integer from 1 to 65535"))
        })
        .transpose()?;
    let target = SshControllerTarget::new(
        SshControllerTargetId::new("cli-target").expect("constant target ID is valid"),
        host,
        user,
        port,
    )
    .map_err(|_| usage("SSH target is invalid"))?;
    let action_name = arguments[action_index].as_str();
    let action_arguments = &arguments[action_index + 1..];
    let action = match action_name {
        "pair" | "sessions" => {
            parse_options(action_arguments, &[], &[])?;
            if action_name == "pair" {
                ControllerSshAction::Pair
            } else {
                ControllerSshAction::Sessions
            }
        }
        "attach" => {
            let options = parse_options(
                action_arguments,
                &[
                    "--session",
                    "--generation",
                    "--from-sequence",
                    "--columns",
                    "--rows",
                ],
                &["--write"],
            )?;
            ControllerSshAction::Attach {
                session_id: required_session(&options)?,
                occupant_generation: required_generation(&options)?,
                from_sequence: options
                    .value("--from-sequence")
                    .map(parse_sequence)
                    .transpose()?
                    .unwrap_or(OutputSequence::ZERO),
                columns: optional_dimension(&options, "--columns", 80)?,
                rows: optional_dimension(&options, "--rows", 24)?,
                request_control: options.flag("--write"),
            }
        }
        "input" => {
            let options = parse_options(action_arguments, &["--session", "--generation"], &[])?;
            ControllerSshAction::Input {
                session_id: required_session(&options)?,
                occupant_generation: required_generation(&options)?,
            }
        }
        "resize" => {
            let options = parse_options(
                action_arguments,
                &["--session", "--generation", "--columns", "--rows"],
                &[],
            )?;
            ControllerSshAction::Resize {
                session_id: required_session(&options)?,
                occupant_generation: required_generation(&options)?,
                columns: required_dimension(&options, "--columns")?,
                rows: required_dimension(&options, "--rows")?,
            }
        }
        "approval" => {
            let options = parse_options(
                action_arguments,
                &["--session", "--generation", "--approval", "--decision"],
                &[],
            )?;
            let approval_id = required(&options, "--approval")?
                .parse::<Uuid>()
                .map_err(|_| usage("approval ID must be a canonical UUID"))?;
            let decision = match required(&options, "--decision")? {
                "allow" => ApprovalDecision::Allow,
                "deny" => ApprovalDecision::Deny,
                _ => return Err(usage("approval decision must be allow or deny")),
            };
            ControllerSshAction::Approval {
                session_id: required_session(&options)?,
                occupant_generation: required_generation(&options)?,
                approval_id,
                decision,
            }
        }
        "detach" => {
            let options = parse_options(action_arguments, &["--session", "--generation"], &[])?;
            ControllerSshAction::Detach {
                session_id: required_session(&options)?,
                occupant_generation: required_generation(&options)?,
            }
        }
        _ => unreachable!("action was selected from the fixed action set"),
    };
    Ok(ControllerSshCommand {
        target,
        action,
        allow_interaction,
    })
}

fn required_session(options: &ParsedOptions<'_>) -> Result<HostedSessionId, CliError> {
    parse_id(required(options, "--session")?, "session")
}

fn required_generation(options: &ParsedOptions<'_>) -> Result<OccupantGeneration, CliError> {
    let value = required(options, "--generation")?
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| usage("occupant generation must be a positive integer"))?;
    Ok(OccupantGeneration::new(value))
}

fn parse_sequence(value: &str) -> Result<OutputSequence, CliError> {
    value
        .parse::<u64>()
        .map(OutputSequence::new)
        .map_err(|_| usage("output sequence must be an unsigned integer"))
}

fn required_dimension(options: &ParsedOptions<'_>, name: &str) -> Result<u16, CliError> {
    parse_dimension(required(options, name)?)
}

fn optional_dimension(
    options: &ParsedOptions<'_>,
    name: &str,
    default: u16,
) -> Result<u16, CliError> {
    options
        .value(name)
        .map(parse_dimension)
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_dimension(value: &str) -> Result<u16, CliError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|value| (1..=1_000).contains(value))
        .ok_or_else(|| usage("terminal dimensions must be integers from 1 to 1000"))
}

struct ParsedOptions<'a> {
    values: Vec<(&'a str, &'a str)>,
    flags: HashSet<&'a str>,
}

impl ParsedOptions<'_> {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn parse_options<'a>(
    arguments: &'a [String],
    value_options: &[&str],
    flag_options: &[&str],
) -> Result<ParsedOptions<'a>, CliError> {
    let mut values = Vec::new();
    let mut flags = HashSet::new();
    let mut seen = HashSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].as_str();
        if !seen.insert(name) {
            return Err(usage("duplicate command option"));
        }
        if value_options.contains(&name) {
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| usage("command option requires a value"))?;
            if value.starts_with("--") {
                return Err(usage("command option requires a value"));
            }
            values.push((name, value.as_str()));
            index += 2;
        } else if flag_options.contains(&name) {
            flags.insert(name);
            index += 1;
        } else {
            return Err(usage("unknown command option"));
        }
    }
    Ok(ParsedOptions { values, flags })
}

fn required<'a>(options: &'a ParsedOptions<'a>, name: &str) -> Result<&'a str, CliError> {
    options
        .value(name)
        .ok_or_else(|| usage("required command option is missing"))
}

fn optional_id<T: std::str::FromStr>(
    options: &ParsedOptions<'_>,
    name: &str,
    kind: &'static str,
) -> Result<Option<T>, CliError> {
    options
        .value(name)
        .map(|value| parse_id(value, kind))
        .transpose()
}

fn parse_id<T: std::str::FromStr>(value: &str, kind: &'static str) -> Result<T, CliError> {
    value.parse().map_err(|_| {
        usage(match kind {
            "project" => "project ID must be a canonical UUID",
            "preset" => "preset ID must be a canonical UUID",
            "group" => "group ID must be a canonical UUID",
            _ => "session ID must be a canonical UUID",
        })
    })
}

fn optional_revision(options: &ParsedOptions<'_>) -> Result<Option<Revision>, CliError> {
    options
        .value("--expected-revision")
        .map(|value| {
            value
                .parse::<u64>()
                .map(Revision::new)
                .map_err(|_| usage("expected revision must be an unsigned integer"))
        })
        .transpose()
}

fn parse_state(value: &str) -> Result<HostedSessionState, CliError> {
    use HostedSessionState as State;
    match value {
        "draft" => Ok(State::Draft),
        "validating" => Ok(State::Validating),
        "starting" => Ok(State::Starting),
        "provisioning" => Ok(State::Provisioning),
        "attaching" => Ok(State::Attaching),
        "replaying" => Ok(State::Replaying),
        "live" => Ok(State::Live),
        "recording_paused" => Ok(State::RecordingPaused),
        "stopping" => Ok(State::Stopping),
        "offline" => Ok(State::Offline),
        "orphaned" => Ok(State::Orphaned),
        "gap" => Ok(State::Gap),
        "permission_denied" => Ok(State::PermissionDenied),
        "incompatible" => Ok(State::Incompatible),
        "running_app_attached" => Ok(State::RunningAppAttached),
        "failed" => Ok(State::Failed),
        "cancelled" => Ok(State::Cancelled),
        "exited" => Ok(State::Exited),
        _ => Err(usage("unknown session state")),
    }
}

fn validate_bounds(arguments: &[String]) -> Result<(), CliError> {
    if arguments.len() > MAX_ARG_COUNT
        || arguments
            .iter()
            .any(|argument| argument.len() > MAX_ARG_BYTES)
        || arguments.iter().map(String::len).sum::<usize>() > MAX_TOTAL_ARG_BYTES
    {
        return Err(CliError::new(
            ErrorCode::ResourceLimit,
            "command arguments exceed the supported limit",
            "Reduce the number or size of command arguments.",
        ));
    }
    Ok(())
}

fn usage(message: &'static str) -> CliError {
    CliError::new(
        ErrorCode::Usage,
        message,
        "Run termirust-cli --help for the frozen command syntax.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_duplicates_unknowns_and_invalid_ids_before_execution() {
        for args in [
            vec!["status", "--json", "--json"],
            vec!["session", "list", "--unknown"],
            vec!["session", "show", "not-an-id"],
            vec!["session", "list", "--state", "mystery"],
        ] {
            assert!(parse_args(args.into_iter().map(str::to_string).collect()).is_err());
        }
        let oversized = vec!["x".repeat(MAX_ARG_BYTES + 1)];
        assert_eq!(
            parse_args(oversized).unwrap_err().code,
            ErrorCode::ResourceLimit
        );
    }

    #[test]
    fn json_pairing_is_marked_noninteractive_before_execution() {
        let base = ["controller", "ssh", "--host", "host.example", "pair"];
        let human = parse_args(base.into_iter().map(str::to_string).collect()).unwrap();
        let json = parse_args(
            base.into_iter()
                .chain(["--json"])
                .map(str::to_string)
                .collect(),
        )
        .unwrap();
        let CliCommand::ControllerSsh(human) = human.command else {
            panic!("expected SSH Controller command");
        };
        let CliCommand::ControllerSsh(json) = json.command else {
            panic!("expected SSH Controller command");
        };
        assert!(human.allow_interaction);
        assert!(!json.allow_interaction);
    }

    #[test]
    fn session_remove_parser_separates_preview_from_complete_commit() {
        let id = HostedSessionId::new().to_string();
        let preview = parse_args(
            ["session", "remove", &id, "--expected-revision", "7"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
        .unwrap();
        assert!(matches!(
            preview.command,
            CliCommand::SessionRemove {
                expected_revision: Some(revision),
                preview_token: None,
                confirmed: false,
                confirmation_stdin: false,
                confirmation: None,
                ..
            } if revision == Revision::new(7)
        ));

        let commit = parse_args(
            [
                "session",
                "remove",
                &id,
                "--preview-token",
                "tr-remove-v1:1:2:3:4:5:6",
                "--yes",
                "--confirmation-stdin",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        )
        .unwrap();
        assert!(matches!(
            commit.command,
            CliCommand::SessionRemove {
                preview_token: Some(_),
                confirmed: true,
                confirmation_stdin: true,
                confirmation: None,
                ..
            }
        ));

        for partial in [
            vec!["session", "remove", &id, "--yes"],
            vec!["session", "remove", &id, "--preview-token", "token"],
            vec!["session", "remove", &id, "--confirmation-stdin"],
        ] {
            assert_eq!(
                parse_args(partial.into_iter().map(str::to_string).collect())
                    .unwrap_err()
                    .code,
                ErrorCode::Usage
            );
        }
    }
}
