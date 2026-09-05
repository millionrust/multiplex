//! Autocomplete domain types + pure helpers (no `&self`, no UI primitives).

use std::collections::HashSet;

#[derive(Clone)]
pub struct AutocompleteCandidate {
    pub command: String,
    pub source: AutocompleteSource,
    pub scope_label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutocompleteSource {
    Path,
    Context,
    Argument,
    History,
    Snippet,
    Builtin,
}

impl AutocompleteSource {
    pub fn priority(self) -> u8 {
        match self {
            Self::Path => 0,
            Self::Context => 1,
            Self::Argument => 2,
            Self::History => 3,
            Self::Snippet => 4,
            Self::Builtin => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Context => "context",
            Self::Argument => "argument",
            Self::History => "history",
            Self::Snippet => "snippet",
            Self::Builtin => "builtin",
        }
    }
}

#[derive(Clone, Copy)]
pub struct BuiltinCommandTemplate {
    pub command: &'static str,
    pub detail: &'static str,
    pub source: AutocompleteSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AutocompleteMatchKind {
    Prefix,
    TokenPrefix,
    Substring,
}

pub struct PathAutocompleteQuery {
    pub prefix: String,
    pub fragment: String,
}

pub fn context_detail(kind: &str, current_path: Option<&str>) -> String {
    let Some(current_path) = current_path.map(str::trim).filter(|path| !path.is_empty()) else {
        return format!("Recent output • {kind}");
    };
    format!("Recent output • {kind} • {current_path}")
}

pub fn matches_command_prefix(query: &str, prefix: &str) -> bool {
    query == prefix || query.starts_with(&format!("{prefix} "))
}

pub fn current_path_hint(current_path: Option<&str>) -> Option<String> {
    let generic_segments = [
        "current", "releases", "release", "shared", "srv", "var", "www", "opt", "home", "users",
        "user", "app", "apps", "service", "services", "project", "projects",
    ];

    let mut segments = current_path_segments(current_path);
    segments.reverse();
    for segment in segments {
        if !generic_segments.contains(&segment.as_str()) {
            return Some(segment);
        }
    }

    current_path_segments(current_path).into_iter().last()
}

pub fn current_path_segments(current_path: Option<&str>) -> Vec<String> {
    current_path
        .unwrap_or_default()
        .split(['/', '\\'])
        .map(|segment| segment.trim().to_ascii_lowercase())
        .filter(|segment| !segment.is_empty())
        .collect()
}

pub fn context_target_rank(target: &str, path_hint: Option<&str>) -> u8 {
    let Some(path_hint) = path_hint.filter(|hint| !hint.is_empty()) else {
        return 1;
    };
    let target = target.to_ascii_lowercase();
    if target == path_hint
        || target.starts_with(path_hint)
        || target.contains(&format!("-{path_hint}"))
        || target.contains(&format!("{path_hint}-"))
        || target.contains(&format!("/{path_hint}"))
        || target.contains(&format!("{path_hint}."))
    {
        0
    } else {
        1
    }
}

pub fn extract_git_branch_targets(lines: &[String]) -> Vec<String> {
    let mut branches = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if let Some(branch) = trimmed.strip_prefix("On branch ") {
            if let Some(branch) =
                clean_context_token(branch.split_whitespace().next().unwrap_or_default())
            {
                if seen.insert(branch.clone()) {
                    branches.push(branch);
                }
            }
            continue;
        }

        if let Some(rest) = trimmed
            .strip_prefix("* ")
            .or_else(|| trimmed.strip_prefix("+ "))
            .or_else(|| trimmed.strip_prefix("  "))
        {
            if let Some(branch) =
                clean_context_token(rest.split_whitespace().next().unwrap_or_default())
            {
                if branch != "HEAD" && seen.insert(branch.clone()) {
                    branches.push(branch);
                }
            }
        }
    }

    branches
}

pub fn extract_docker_targets(lines: &[String]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("CONTAINER ID") {
            continue;
        }

        let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
        if tokens.len() < 2 || !looks_like_hex_id(tokens[0]) {
            continue;
        }
        if let Some(target) = clean_context_token(tokens.last().copied().unwrap_or_default()) {
            if seen.insert(target.clone()) {
                targets.push(target);
            }
        }
    }

    targets
}

pub fn extract_kubernetes_pod_targets(lines: &[String]) -> Vec<String> {
    let mut pods = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("NAME ")
            || trimmed.starts_with("No resources found")
        {
            continue;
        }

        let first = trimmed.split_whitespace().next().unwrap_or_default();
        let Some(pod) = clean_context_token(first) else {
            continue;
        };
        if !(pod.contains('-')
            || trimmed.contains("Running")
            || trimmed.contains("Pending")
            || trimmed.contains("Completed")
            || trimmed.contains("CrashLoopBackOff"))
        {
            continue;
        }
        if seen.insert(pod.clone()) {
            pods.push(pod);
        }
    }

    pods
}

pub fn extract_systemd_unit_targets(lines: &[String]) -> Vec<String> {
    let mut units = Vec::new();
    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        for token in line.split_whitespace() {
            let Some(unit) = clean_context_token(token) else {
                continue;
            };
            if !unit.ends_with(".service") {
                continue;
            }
            if seen.insert(unit.clone()) {
                units.push(unit);
            }
        }
    }

    units
}

pub fn clean_context_token(token: &str) -> Option<String> {
    let token = token.trim_matches(|ch: char| {
        !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_' | '.' | '/' | ':' | '@')
    });
    if token.is_empty() || token.eq_ignore_ascii_case("name") {
        None
    } else {
        Some(token.to_string())
    }
}

pub fn looks_like_hex_id(token: &str) -> bool {
    token.len() >= 6 && token.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub fn autocomplete_match_kind(query: &str, command: &str) -> Option<AutocompleteMatchKind> {
    if command.starts_with(query) {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let token_prefix = command
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '/' | '\\' | ':' | '=' | '-' | '_' | '.' | '|' | '&' | ';'
                )
        })
        .filter(|token| !token.is_empty())
        .any(|token| token.starts_with(query));
    if token_prefix {
        return Some(AutocompleteMatchKind::TokenPrefix);
    }

    if query.len() >= 2 && command.contains(query) {
        return Some(AutocompleteMatchKind::Substring);
    }

    None
}

pub fn palette_match_kind(query: &str, fields: &[&str]) -> Option<AutocompleteMatchKind> {
    if query.is_empty() {
        return Some(AutocompleteMatchKind::Prefix);
    }

    fields
        .iter()
        .filter_map(|field| autocomplete_match_kind(query, &field.to_ascii_lowercase()))
        .min()
}

pub fn path_query_context(input: &str) -> Option<PathAutocompleteQuery> {
    let input = input.trim_end_matches(['\r', '\n']);
    if input.trim().is_empty() {
        return None;
    }

    let tokens = input.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    if input.ends_with(' ') {
        let last = tokens.last().copied().unwrap_or_default();
        if is_path_command(last) {
            return Some(PathAutocompleteQuery {
                prefix: input.to_string(),
                fragment: String::new(),
            });
        }
        return None;
    }

    let last = tokens.last().copied().unwrap_or_default();
    let previous = tokens
        .get(tokens.len().saturating_sub(2))
        .copied()
        .unwrap_or_default();
    if !is_path_like_token(last) && !is_path_command(previous) {
        return None;
    }

    let start = input.rfind(last)?;
    Some(PathAutocompleteQuery {
        prefix: input[..start].to_string(),
        fragment: last.to_string(),
    })
}

pub fn is_path_command(command: &str) -> bool {
    matches!(
        command,
        "cd" | "ls"
            | "cat"
            | "tail"
            | "less"
            | "more"
            | "vim"
            | "nvim"
            | "nano"
            | "rm"
            | "cp"
            | "mv"
            | "mkdir"
            | "touch"
            | "chmod"
            | "chown"
            | "source"
    )
}

pub fn is_path_like_token(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.contains('/')
        || token == "."
        || token == ".."
}

pub fn path_match_kind(fragment: &str, candidate: &str) -> Option<AutocompleteMatchKind> {
    let fragment = fragment.trim().to_ascii_lowercase();
    if fragment.is_empty() {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let candidate_lower = candidate.to_ascii_lowercase();
    if candidate_lower.starts_with(&fragment) {
        return Some(AutocompleteMatchKind::Prefix);
    }

    let basename = candidate
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(candidate)
        .to_ascii_lowercase();
    let stripped_fragment = fragment
        .trim_start_matches("./")
        .trim_start_matches("../")
        .trim_start_matches("~/");
    if !stripped_fragment.is_empty() && basename.starts_with(stripped_fragment) {
        return Some(AutocompleteMatchKind::TokenPrefix);
    }

    if fragment.len() >= 2 && candidate_lower.contains(&fragment) {
        return Some(AutocompleteMatchKind::Substring);
    }

    None
}

pub fn extract_path_tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                ch.is_ascii_punctuation() && ch != '/' && ch != '.' && ch != '_' && ch != '-'
            });
            if is_path_like_token(token) {
                Some(token.to_string())
            } else {
                None
            }
        })
        .collect()
}

pub fn builtin_command_templates() -> &'static [BuiltinCommandTemplate] {
    &[
        BuiltinCommandTemplate {
            command: "pwd",
            detail: "Print the current working directory",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "ls -la",
            detail: "List files with hidden entries and details",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "cd /var/www",
            detail: "Jump to a common web root path",
            source: AutocompleteSource::Builtin,
        },
        BuiltinCommandTemplate {
            command: "git status",
            detail: "Show working tree status",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git pull",
            detail: "Fetch and merge from the tracked remote branch",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git fetch --all",
            detail: "Fetch all remotes without changing the working tree",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git checkout main",
            detail: "Switch to a branch or restore a path",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git diff",
            detail: "Inspect uncommitted changes",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "git log --oneline --decorate -20",
            detail: "Show recent commit history in a compact view",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker ps",
            detail: "List running containers",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker logs -f",
            detail: "Stream container logs",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker exec -it",
            detail: "Open an interactive shell inside a running container",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose ps",
            detail: "List compose services and state",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose logs -f",
            detail: "Stream logs for a compose project",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "docker compose up -d",
            detail: "Start compose services in the background",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl get pods",
            detail: "List pods in the current namespace",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl logs -f",
            detail: "Stream logs from a Kubernetes pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl describe pod",
            detail: "Inspect the full state of a pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "kubectl exec -it",
            detail: "Open an interactive shell inside a pod",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl status",
            detail: "Inspect a systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl restart",
            detail: "Restart a systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "systemctl reload",
            detail: "Reload a unit without a full restart when supported",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "journalctl -u",
            detail: "View logs for a specific systemd unit",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "journalctl -f -u",
            detail: "Follow logs for a systemd unit in real time",
            source: AutocompleteSource::Argument,
        },
        BuiltinCommandTemplate {
            command: "tail -f /var/log/syslog",
            detail: "Follow a log file",
            source: AutocompleteSource::Builtin,
        },
    ]
}
