use std::collections::BTreeSet;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use termirust_browser::ApprovedOrigin;
use termirust_cli::CliPaths;
use termirust_domain::{CommandId, HostedSessionId, ProjectId};
use termirust_mcp::{ActionPolicy, ActionPolicyStore, ApprovedAction};

const USAGE: &str = "usage:\n  termirust-mcp-authorize grant --actions ACTION,... [--projects UUID,...] [--sessions UUID,...] [--browser-origins ORIGIN,...] --minutes 1..1440\n  termirust-mcp-authorize revoke";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: Vec<String>) -> Result<String, &'static str> {
    let paths = CliPaths::discover().map_err(|_| "TermiRust configuration is unavailable")?;
    let store = ActionPolicyStore::new(paths.config_root().join("mcp"));
    match arguments.as_slice() {
        [command] if command == "revoke" => {
            store
                .revoke()
                .map_err(|_| "unable to revoke MCP approval")?;
            Ok("MCP action approval revoked".to_string())
        }
        [command, rest @ ..] if command == "grant" => grant(&store, rest),
        _ => Err("invalid authorization command"),
    }
}

fn grant(store: &ActionPolicyStore, arguments: &[String]) -> Result<String, &'static str> {
    let mut actions = None;
    let mut projects = Vec::new();
    let mut sessions = Vec::new();
    let mut browser_origins = Vec::new();
    let mut minutes = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or("authorization option is missing a value")?;
        match flag {
            "--actions" if actions.is_none() => actions = Some(parse_actions(value)?),
            "--projects" if projects.is_empty() => projects = parse_ids::<ProjectId>(value)?,
            "--sessions" if sessions.is_empty() => sessions = parse_ids::<HostedSessionId>(value)?,
            "--browser-origins" if browser_origins.is_empty() => {
                browser_origins = parse_browser_origins(value)?
            }
            "--minutes" if minutes.is_none() => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "minutes must be an integer")?;
                if !(1..=1_440).contains(&parsed) {
                    return Err("minutes must be between 1 and 1440");
                }
                minutes = Some(parsed);
            }
            _ => return Err("unknown or duplicate authorization option"),
        }
        index += 2;
    }
    let actions = actions.ok_or("--actions is required")?;
    let minutes = minutes.ok_or("--minutes is required")?;
    if projects.is_empty() && sessions.is_empty() {
        return Err("at least one Project or Session scope is required");
    }
    if actions.contains(&ApprovedAction::Launch) && projects.is_empty() {
        return Err("launch approval requires a Project scope");
    }
    if actions
        .iter()
        .any(|action| *action != ApprovedAction::Launch)
        && sessions.is_empty()
    {
        return Err("non-launch actions require a Session scope");
    }
    let has_browser_action = actions.iter().any(|action| {
        matches!(
            action,
            ApprovedAction::BrowserText
                | ApprovedAction::BrowserScreenshot
                | ApprovedAction::BrowserDownload
        )
    });
    if has_browser_action != !browser_origins.is_empty() {
        return Err(
            "browser actions require --browser-origins, which is valid only for browser actions",
        );
    }
    let expires_at_unix_ms = now_millis().saturating_add(minutes.saturating_mul(60_000));
    let policy = ActionPolicy {
        schema_version: 1,
        grant_id: CommandId::new().to_string(),
        expires_at_unix_ms,
        actions: actions.into_iter().collect(),
        project_ids: projects,
        session_ids: sessions,
        browser_origins,
    };
    store
        .write_policy(&policy)
        .map_err(|_| "unable to write MCP approval")?;
    Ok(format!(
        "MCP action approval granted until {expires_at_unix_ms}; revoke with termirust-mcp-authorize revoke"
    ))
}

fn parse_browser_origins(value: &str) -> Result<Vec<String>, &'static str> {
    let origins = value
        .split(',')
        .map(|value| {
            ApprovedOrigin::parse(value)
                .map(|origin| origin.as_string())
                .map_err(|_| "browser origins must be exact HTTP(S) origins without paths")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if origins.is_empty() || origins.len() > 32 {
        Err("browser origin scope must contain between 1 and 32 origins")
    } else {
        Ok(origins.into_iter().collect())
    }
}

fn parse_actions(value: &str) -> Result<BTreeSet<ApprovedAction>, &'static str> {
    let mut actions = BTreeSet::new();
    for value in value.split(',') {
        let action = ApprovedAction::parse(value).ok_or("unknown action name")?;
        actions.insert(action);
    }
    if actions.is_empty() {
        Err("at least one action is required")
    } else {
        Ok(actions)
    }
}

fn parse_ids<T>(value: &str) -> Result<Vec<String>, &'static str>
where
    T: std::str::FromStr + ToString,
{
    let ids = value
        .split(',')
        .map(|value| {
            value
                .parse::<T>()
                .map(|id| id.to_string())
                .map_err(|_| "scope IDs must be UUIDs")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() || ids.len() > 256 {
        Err("scope must contain between 1 and 256 IDs")
    } else {
        Ok(ids)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
