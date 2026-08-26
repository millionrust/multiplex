use std::collections::HashSet;

use termirust_domain::{
    GroupId, HostedSessionId, HostedSessionState, PresetId, ProjectId, Revision,
};

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
        _ => return Err(usage("unknown or incomplete command")),
    };
    Ok(Invocation { json, command })
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
}
