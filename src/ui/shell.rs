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

pub fn startup_bytes_for_request(
    request: &ConnectRequest,
    default_startup_dir: Option<&str>,
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

    if lines.is_empty() {
        None
    } else {
        Some(format!("{}\n", lines.join("\n")).into_bytes())
    }
}
