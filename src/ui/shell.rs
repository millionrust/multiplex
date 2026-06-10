//! Pure shell-string helpers — pasting commands, building startup hooks,
//! detecting incomplete commands.

use crate::models::{ConnectRequest, ConnectionKind};

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

pub fn persistent_session_name(request: &ConnectRequest) -> String {
    if let Some(name) = request
        .persistent_session_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }

    let kind = match request.kind {
        ConnectionKind::Ssh => "ssh",
        ConnectionKind::LocalShell => "local",
    };
    let label = match request.kind {
        ConnectionKind::Ssh => format!(
            "{}-{}-{}-{}-{}",
            request.title, request.username, request.host, request.port, request.session_id
        ),
        ConnectionKind::LocalShell => {
            format!(
                "{}-{}-{}",
                request.title, request.username, request.session_id
            )
        }
    };
    let slug = slugify_tmux_name(&label);
    let hash = fnv1a64(&label);
    format!("tshell-{kind}-{slug}-{hash:016x}")
}

pub fn startup_bytes_for_request(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
    persistent_terminal_sessions: bool,
) -> Option<Vec<u8>> {
    if request.is_local_shell() {
        return None;
    }

    let mut lines = Vec::new();
    for (key, value) in &request.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        lines.push(format!("export {key}={}", shell_single_quote(value)));
    }
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

    if persistent_terminal_sessions {
        return Some(tmux_bootstrap_script(request, &lines).into_bytes());
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("{}\n", lines.join("\n")).into_bytes())
    }
}

pub fn local_tmux_wrapper_script(
    request: &ConnectRequest,
    program: &str,
    args: &[String],
    bundled_tmux_path: Option<&str>,
) -> String {
    let session_name = shell_single_quote(&persistent_session_name(request));
    let command = shell_exec_command(program, args);
    let tmux_command = shell_single_quote(&command);
    let bundled_tmux = bundled_tmux_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(shell_single_quote);
    let bundled_branch = bundled_tmux
        .map(|path| {
            format!(
                "elif [ -x {path} ]; then\n  exec {path} new-session -A -s {session_name} {tmux_command}\n"
            )
        })
        .unwrap_or_default();

    format!(
        "if command -v tmux >/dev/null 2>&1; then\n  exec tmux new-session -A -s {session_name} {tmux_command}\n{bundled_branch}else\n  printf '%s\\n' 'tmux not found; continuing without persistent terminal.'\n  {command}\nfi\n"
    )
}

fn tmux_bootstrap_script(request: &ConnectRequest, startup_lines: &[String]) -> String {
    let session_name = shell_single_quote(&persistent_session_name(request));
    let new_session_command = shell_single_quote(&tmux_new_session_command(startup_lines));
    let fallback = if startup_lines.is_empty() {
        String::new()
    } else {
        format!("  {}\n", startup_lines.join("\n  "))
    };

    format!(
        "if command -v tmux >/dev/null 2>&1; then\n  exec tmux new-session -A -s {session_name} {new_session_command}\nelse\n  printf '%s\\n' 'tmux not found; continuing without persistent terminal.'\n{fallback}fi\n"
    )
}

fn tmux_new_session_command(startup_lines: &[String]) -> String {
    let mut lines = startup_lines.to_vec();
    lines.push("exec \"${SHELL:-sh}\" -l".to_string());
    lines.join("\n")
}

fn shell_exec_command(program: &str, args: &[String]) -> String {
    let mut parts = vec!["exec".to_string(), shell_single_quote(program)];
    parts.extend(args.iter().map(|arg| shell_single_quote(arg)));
    parts.join(" ")
}

fn slugify_tmux_name(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if (ch == '-' || ch == '_' || ch == '.') && !slug.ends_with('-') {
            slug.push('-');
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug.to_string()
    }
}

fn fnv1a64(value: &str) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    value.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectRequest, ConnectionKind};

    fn ssh_request() -> ConnectRequest {
        ConnectRequest {
            session_id: 1,
            title: "Prod API".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: None,
            jump_host: None,
            startup_directory: Some("/srv/app's".to_string()),
            startup_command: Some("docker compose logs -f".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: vec![("AWS_PROFILE".to_string(), "prod".to_string())],
            persistent_session_name: None,
        }
    }

    #[test]
    fn persistent_ssh_startup_uses_tmux_attach_with_new_session_bootstrap() {
        let request = ssh_request();
        let script = String::from_utf8(startup_bytes_for_request(&request, None, true).unwrap())
            .expect("startup script should be utf8");

        assert!(script.contains("exec tmux new-session -A -s 'tshell-ssh-"));
        assert!(script.contains("export AWS_PROFILE='prod'"));
        assert!(script.contains("cd -- '/srv/app'\"'\"'s'"));
        assert!(script.contains("docker compose logs -f"));
        assert!(script.contains("exec \"${SHELL:-sh}\" -l"));
        assert!(script.contains("tmux not found; continuing without persistent terminal."));
    }

    #[test]
    fn persistent_session_names_are_stable_and_tmux_safe() {
        let request = ssh_request();
        let name = persistent_session_name(&request);

        assert_eq!(name, persistent_session_name(&request));
        assert!(name.starts_with("tshell-ssh-prod-api-deploy-prod-example-com-22-"));
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
    }

    #[test]
    fn local_tmux_wrapper_falls_back_to_configured_shell() {
        let mut request = ssh_request();
        request.kind = ConnectionKind::LocalShell;
        request.title = "Local Terminal".to_string();
        request.host = "local".to_string();
        request.port = 0;

        let script = local_tmux_wrapper_script(
            &request,
            "/bin/zsh",
            &["-l".to_string(), "-i".to_string()],
            None,
        );

        assert!(script.contains("exec tmux new-session -A -s 'tshell-local-"));
        assert!(script.contains("tmux not found; continuing without persistent terminal."));
        assert!(script.contains("exec '/bin/zsh' '-l' '-i'"));
    }

    #[test]
    fn local_tmux_wrapper_uses_bundled_tmux_when_system_tmux_is_missing() {
        let mut request = ssh_request();
        request.kind = ConnectionKind::LocalShell;
        request.title = "Local Terminal".to_string();

        let script = local_tmux_wrapper_script(&request, "/bin/zsh", &[], Some("/app/bin/tmux"));

        assert!(script.contains("elif [ -x '/app/bin/tmux' ]; then"));
        assert!(script.contains("exec '/app/bin/tmux' new-session -A -s 'tshell-local-"));
    }
}
