//! Pure shell-string helpers — pasting commands, building startup hooks,
//! detecting incomplete commands.

use crate::models::ConnectRequest;

pub fn shell_command_requires_continuation(command: &str) -> bool {
    let trimmed = command.trim_end();
    if trimmed.is_empty() {
        return false;
    }

    let trailing_backslashes = trimmed.chars().rev().take_while(|ch| *ch == '\\').count();
    if trailing_backslashes % 2 == 1 {
        return true;
    }

    if trimmed.ends_with("&&") || trimmed.ends_with("||") {
        return true;
    }

    if trimmed.ends_with('|')
        || trimmed.ends_with('(')
        || trimmed.ends_with('{')
        || trimmed.ends_with('[')
    {
        return true;
    }

    let mut single_quote = false;
    let mut double_quote = false;
    let mut backtick = false;
    let mut escaped = false;
    let mut paren_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;

    for ch in trimmed.chars() {
        if escaped {
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !single_quote => {
                escaped = true;
            }
            '\'' if !double_quote && !backtick => {
                single_quote = !single_quote;
            }
            '"' if !single_quote && !backtick => {
                double_quote = !double_quote;
            }
            '`' if !single_quote && !double_quote => {
                backtick = !backtick;
            }
            '(' if !single_quote && !double_quote && !backtick => {
                paren_depth += 1;
            }
            ')' if !single_quote && !double_quote && !backtick => {
                paren_depth = (paren_depth - 1).max(0);
            }
            '{' if !single_quote && !double_quote && !backtick => {
                brace_depth += 1;
            }
            '}' if !single_quote && !double_quote && !backtick => {
                brace_depth = (brace_depth - 1).max(0);
            }
            '[' if !single_quote && !double_quote && !backtick => {
                bracket_depth += 1;
            }
            ']' if !single_quote && !double_quote && !backtick => {
                bracket_depth = (bracket_depth - 1).max(0);
            }
            _ => {}
        }
    }

    single_quote
        || double_quote
        || backtick
        || paren_depth > 0
        || brace_depth > 0
        || bracket_depth > 0
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn startup_environment_lines(request: &ConnectRequest) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, value) in &request.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        lines.push(format!("export {key}={}", shell_single_quote(value)));
    }
    lines
}

pub fn startup_bytes_for_request(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
) -> Option<Vec<u8>> {
    if request.is_local_shell() {
        return None;
    }

    let mut lines = startup_environment_lines(request);

    let effective_dir = request.startup_directory.as_deref().or(default_startup_dir);
    if let Some(directory) = effective_dir {
        let directory = directory.trim();
        if !directory.is_empty() {
            lines.push(format!("cd -- {}", shell_single_quote(directory)));
        }
    }
    if let Some(command) = request.startup_command.as_deref() {
        let command = command.trim();
        if !command.is_empty() {
            lines.push(command.to_string());
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("{}\n", lines.join("\n")).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::startup_bytes_for_request;
    use crate::models::{AuthConfig, ConnectRequest, ConnectionKind};

    fn request() -> ConnectRequest {
        ConnectRequest {
            session_id: 1,
            title: "prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: Some(AuthConfig::PrivateKey {
                key_path: "/tmp/id_ed25519".to_string(),
                passphrase: None,
            }),
            jump_host: None,
            outbound_proxy: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    fn startup_text(request: &ConnectRequest, default_dir: Option<&str>) -> String {
        String::from_utf8(
            startup_bytes_for_request(request, default_dir)
                .expect("startup bytes should be generated"),
        )
        .expect("startup bytes should be utf8")
    }

    #[test]
    fn non_persistent_startup_output_is_preserved() {
        let mut request = request();
        request.environment = vec![("APP_ENV".to_string(), "prod".to_string())];
        request.startup_directory = Some("/srv/app".to_string());
        request.startup_command = Some("uptime".to_string());

        assert_eq!(
            startup_text(&request, None),
            "export APP_ENV='prod'\ncd -- '/srv/app'\nuptime\n"
        );
    }

    #[test]
    fn legacy_tmux_flags_do_not_wrap_ssh_startup() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("old-session".into());
        request.persistent_session_detach_others = true;
        request.startup_command = Some("echo ready".into());
        assert_eq!(
            startup_text(&request, Some("/srv/app")),
            "cd -- '/srv/app'\necho ready\n"
        );
    }
}
