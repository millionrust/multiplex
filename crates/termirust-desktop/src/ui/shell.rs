//! Pure shell-string helpers — pasting commands, building startup hooks,
//! detecting incomplete commands.

use crate::models::ConnectRequest;
use crate::ui::localization;

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

fn persistent_session_name(request: &ConnectRequest) -> String {
    request
        .persistent_session_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            crate::models::default_persistent_session_name_for_endpoint(
                &request.username,
                &request.host,
                request.port,
            )
        })
}

pub fn tmux_bootstrap_script(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
) -> String {
    let session_name = persistent_session_name(request);
    let session = shell_single_quote(&session_name);
    let attach = if request.persistent_session_detach_others {
        format!("exec tmux attach-session -d -t {session}")
    } else {
        format!("exec tmux attach-session -t {session}")
    };

    let mut new_session = format!("exec tmux new-session -s {session}");
    let effective_dir = request.startup_directory.as_deref().or(default_startup_dir);
    if let Some(directory) = effective_dir
        .map(str::trim)
        .filter(|directory| !directory.is_empty())
    {
        new_session.push_str(" -c ");
        new_session.push_str(&shell_single_quote(directory));
    }
    if let Some(command) = request
        .startup_command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        let shell_command = format!("{command}; exec \"${{SHELL:-/bin/sh}}\" -l");
        new_session.push_str(" -- \"${SHELL:-/bin/sh}\" -lc ");
        new_session.push_str(&shell_single_quote(&shell_command));
    }

    let missing = shell_single_quote(&format!("\n{}\n", localization::shell_tmux_missing()));
    let install = shell_single_quote(&localization::shell_tmux_install_guidance());
    let install_generic = shell_single_quote(&localization::shell_tmux_install_generic());
    let fallback = shell_single_quote(&format!("\n{}\n", localization::shell_tmux_fallback()));

    format!(
        "if command -v tmux >/dev/null 2>&1; then\n  if tmux has-session -t {session} 2>/dev/null; then\n    {attach}\n  else\n    {new_session}\n  fi\nelse\n  printf '\\033[2J\\033[H'\n  printf '%s\\n' {missing} >&2\n  printf '%s\\n' {install} >&2\n  if command -v brew >/dev/null 2>&1 || [ -x /opt/homebrew/bin/brew ] || [ -x /usr/local/bin/brew ]; then\n    printf '%s\\n' '  brew install tmux' >&2\n  elif command -v apt-get >/dev/null 2>&1; then\n    printf '%s\\n' '  sudo apt-get update && sudo apt-get install -y tmux' >&2\n  elif command -v dnf >/dev/null 2>&1; then\n    printf '%s\\n' '  sudo dnf install -y tmux' >&2\n  elif command -v yum >/dev/null 2>&1; then\n    printf '%s\\n' '  sudo yum install -y tmux' >&2\n  elif command -v pacman >/dev/null 2>&1; then\n    printf '%s\\n' '  sudo pacman -S tmux' >&2\n  else\n    printf '%s\\n' {install_generic} >&2\n  fi\n  printf '%s\\n' {fallback} >&2\n  exec \"${{SHELL:-/bin/sh}}\"\nfi"
    )
}

pub fn startup_bytes_for_request(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
) -> Option<Vec<u8>> {
    if request.is_local_shell() {
        return None;
    }

    let mut lines = startup_environment_lines(request);
    if request.persistent_session {
        lines.push(tmux_bootstrap_script(request, default_startup_dir));
        return Some(format!("{}\n", lines.join("\n")).into_bytes());
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
    fn persistent_session_attaches_existing_session_without_startup_command() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-prod".to_string());
        request.startup_directory = Some("/srv/app".to_string());
        request.startup_command = Some("uptime".to_string());

        let script = startup_text(&request, None);
        assert!(script.contains("tmux has-session -t 'tr-prod'"));
        assert!(script.contains("exec tmux attach-session -t 'tr-prod'"));
        assert!(script.contains("exec tmux new-session -s 'tr-prod' -c '/srv/app'"));
        assert!(
            script.contains("-- \"${SHELL:-/bin/sh}\" -lc 'uptime; exec \"${SHELL:-/bin/sh}\" -l'")
        );

        let attach_index = script.find("exec tmux attach-session").unwrap();
        let create_index = script.find("exec tmux new-session").unwrap();
        let command_index = script.find("uptime; exec").unwrap();
        assert!(attach_index < create_index);
        assert!(create_index < command_index);
    }

    #[test]
    fn persistent_session_exports_environment_before_tmux_block() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-prod".to_string());
        request.environment = vec![("TOKEN".to_string(), "a'b".to_string())];

        let script = startup_text(&request, None);
        assert!(script.starts_with("export TOKEN='a'\"'\"'b'\nif command -v tmux"));
    }

    #[test]
    fn persistent_session_uses_default_startup_directory_on_create_only() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-prod".to_string());

        let script = startup_text(&request, Some("/opt/app"));
        assert!(script.contains("exec tmux attach-session -t 'tr-prod'"));
        assert!(script.contains("exec tmux new-session -s 'tr-prod' -c '/opt/app'"));
    }

    #[test]
    fn persistent_session_detach_others_uses_attach_dash_d() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-prod".to_string());
        request.persistent_session_detach_others = true;

        let script = startup_text(&request, None);
        assert!(script.contains("exec tmux attach-session -d -t 'tr-prod'"));
    }

    #[test]
    fn persistent_session_falls_back_to_endpoint_name() {
        let mut request = request();
        request.persistent_session = true;

        let script = startup_text(&request, None);
        assert!(script.contains("tmux has-session -t 'tr-deploy-prod-example-com-22'"));
    }

    #[test]
    fn persistent_session_quotes_custom_name_directory_and_command() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("team's prod".to_string());
        request.startup_directory = Some("/srv/app's current".to_string());
        request.startup_command = Some("printf 'ready now'".to_string());

        let script = startup_text(&request, None);
        assert!(script.contains("tmux has-session -t 'team'\"'\"'s prod'"));
        assert!(script.contains("-c '/srv/app'\"'\"'s current'"));
        assert!(script.contains("'printf '\"'\"'ready now'\"'\"'; exec \"${SHELL:-/bin/sh}\" -l'"));
    }

    #[test]
    fn persistent_session_missing_tmux_falls_back_to_shell() {
        let mut request = request();
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-prod".to_string());

        let script = startup_text(&request, None);
        assert!(script.contains("TermiRust Persistent Session could not start"));
        assert!(script.contains("Install tmux on the remote machine, then reconnect:"));
        assert!(script.contains("brew install tmux"));
        assert!(script.contains("sudo apt-get update && sudo apt-get install -y tmux"));
        assert!(script.contains("TermiRust opened a normal shell for now."));
        assert!(script.contains("printf '\\033[2J\\033[H'"));
        assert!(script.contains("exec \"${SHELL:-/bin/sh}\""));
    }
}
