//! Command palette + autocomplete candidate collection. The actual UI render
//! lives in `app::overlay`. This module owns the data-side types and pure
//! collect_* helpers that scan history, snippets, the active pane's recent
//! output, etc.

use std::collections::HashSet;
use termirust_ui_contract::PaletteResultId;

use crate::models::{SavedCommandHistoryEntry, SavedSnippet};
use crate::sftp::RemoteFileEntry;
use crate::ui::app::SessionPane;
use crate::ui::autocomplete::{
    AutocompleteCandidate, AutocompleteMatchKind, AutocompleteSource, autocomplete_match_kind,
    builtin_command_templates, context_detail, context_target_rank, current_path_hint,
    extract_docker_targets, extract_git_branch_targets, extract_kubernetes_pod_targets,
    extract_path_tokens, extract_systemd_unit_targets, matches_command_prefix, palette_match_kind,
    path_match_kind, path_query_context,
};
use crate::ui::localization;
use crate::ui::path::remote_parent_path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaletteCategory {
    Attention,
    Sessions,
    Projects,
    Groups,
    Presets,
    Actions,
    Archive,
    Commands,
}

impl PaletteCategory {
    pub(super) const fn rank(self) -> u8 {
        match self {
            Self::Attention => 0,
            Self::Sessions => 1,
            Self::Projects => 2,
            Self::Groups => 3,
            Self::Presets => 4,
            Self::Actions => 5,
            Self::Archive => 6,
            Self::Commands => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaletteAction {
    Search(termirust_domain::SearchAction),
    RunCommand,
}

#[derive(Clone)]
pub(super) struct CommandPaletteCandidate {
    pub(super) id: PaletteResultId,
    pub(super) command: String,
    pub(super) title: String,
    pub(super) detail: String,
    pub(super) source: AutocompleteSource,
    pub(super) pinned: bool,
    pub(super) category: PaletteCategory,
    pub(super) action: PaletteAction,
    pub(super) status: Option<termirust_domain::SearchStatus>,
    pub(super) highlights: Vec<termirust_domain::TextHighlight>,
}

pub(super) fn command_palette_result_id(namespace: u8, value: &str) -> PaletteResultId {
    let mut hash = 0xcbf29ce484222325u64 ^ u64::from(namespace);
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PaletteResultId::new(hash.max(1)).expect("non-zero palette result hash")
}

#[derive(Clone, Default)]
pub(super) struct PathSuggestionContext {
    pub(super) current_path: Option<String>,
    pub(super) startup_directory: Option<String>,
    pub(super) entries: Vec<RemoteFileEntry>,
}

#[derive(Clone, Default)]
pub(super) struct OutputSuggestionContext {
    pub(super) current_path: Option<String>,
    pub(super) recent_lines: Vec<String>,
}

#[derive(Clone)]
pub(super) struct ContextCommandTemplate {
    pub(super) command: String,
    pub(super) detail: String,
    pub(super) rank: u8,
    pub(super) ordinal: usize,
}

pub(super) fn collect_autocomplete_candidates(
    input: &str,
    command_history: &[String],
    scoped_command_history: &[SavedCommandHistoryEntry],
    scope_key: &str,
    snippets: &[SavedSnippet],
    path_context: Option<&PathSuggestionContext>,
    output_context: Option<&OutputSuggestionContext>,
) -> Vec<AutocompleteCandidate> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    #[derive(Clone)]
    struct ScoredAutocompleteCandidate {
        candidate: AutocompleteCandidate,
        match_kind: AutocompleteMatchKind,
        snippet_priority: u8,
        ordinal: usize,
    }

    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path_suggestions) =
        collect_path_autocomplete_candidates(input, path_context, command_history, snippets)
    {
        for (ordinal, candidate) in path_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal,
                });
            }
        }
    }

    if let Some(context_suggestions) =
        collect_context_autocomplete_candidates(input, output_context)
    {
        for (ordinal, candidate) in context_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal: ordinal + 100,
                });
            }
        }
    }

    if let Some(argument_suggestions) = collect_argument_autocomplete_candidates(input) {
        for (ordinal, candidate) in argument_suggestions.into_iter().enumerate() {
            if seen.insert(candidate.command.to_ascii_lowercase()) {
                suggestions.push(ScoredAutocompleteCandidate {
                    candidate,
                    match_kind: AutocompleteMatchKind::Prefix,
                    snippet_priority: 0,
                    ordinal: ordinal + 200,
                });
            }
        }
    }

    for (ordinal, entry) in scoped_command_history
        .iter()
        .rev()
        .filter(|entry| entry.scope_key == scope_key)
        .enumerate()
    {
        let command = entry.command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::History,
                    scope_label: Some(if entry.scope_label.trim().is_empty() {
                        localization::palette_this_target()
                    } else {
                        entry.scope_label.clone()
                    }),
                },
                match_kind,
                snippet_priority: 1,
                ordinal,
            });
        }
    }

    for (ordinal, command) in command_history.iter().rev().enumerate() {
        let command = command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::History,
                    scope_label: None,
                },
                match_kind,
                snippet_priority: 1,
                ordinal: ordinal + scoped_command_history.len(),
            });
        }
    }

    for (ordinal, snippet) in snippets.iter().enumerate() {
        let command = snippet.command.trim();
        let key = command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: command.to_string(),
                    source: AutocompleteSource::Snippet,
                    scope_label: None,
                },
                match_kind,
                snippet_priority: if snippet.pinned { 0 } else { 1 },
                ordinal: ordinal + scoped_command_history.len() + command_history.len(),
            });
        }
    }

    for (ordinal, template) in builtin_command_templates().iter().enumerate() {
        let key = template.command.to_ascii_lowercase();
        let Some(match_kind) = autocomplete_match_kind(&query, &key) else {
            continue;
        };
        if seen.insert(key) {
            suggestions.push(ScoredAutocompleteCandidate {
                candidate: AutocompleteCandidate {
                    command: template.command.to_string(),
                    source: template.source,
                    scope_label: Some(template.detail.to_string()),
                },
                match_kind,
                snippet_priority: 1,
                ordinal: ordinal
                    + scoped_command_history.len()
                    + command_history.len()
                    + snippets.len(),
            });
        }
    }

    suggestions.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| {
                left.candidate
                    .source
                    .priority()
                    .cmp(&right.candidate.source.priority())
            })
            .then_with(|| left.snippet_priority.cmp(&right.snippet_priority))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
            .then_with(|| {
                left.candidate
                    .command
                    .to_ascii_lowercase()
                    .cmp(&right.candidate.command.to_ascii_lowercase())
            })
    });

    suggestions
        .into_iter()
        .take(6)
        .map(|candidate| candidate.candidate)
        .collect()
}

pub(super) fn collect_command_palette_candidates(
    query: &str,
    command_history: &[String],
    scoped_command_history: &[SavedCommandHistoryEntry],
    scope_key: &str,
    snippets: &[SavedSnippet],
    output_context: Option<&OutputSuggestionContext>,
) -> Vec<CommandPaletteCandidate> {
    let query = query.trim().to_ascii_lowercase();

    #[derive(Clone)]
    struct ScoredPaletteCandidate {
        candidate: CommandPaletteCandidate,
        match_kind: AutocompleteMatchKind,
        ordinal: usize,
        source_priority: u8,
    }

    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    for (ordinal, entry) in scoped_command_history
        .iter()
        .rev()
        .filter(|entry| entry.scope_key == scope_key)
        .enumerate()
    {
        let command = entry.command.trim();
        let Some(match_kind) = palette_match_kind(&query, &[command, &entry.scope_label]) else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            let scope = if entry.scope_label.trim().is_empty() {
                localization::palette_this_target()
            } else {
                entry.scope_label.clone()
            };
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    id: command_palette_result_id(1, command),
                    command: command.to_string(),
                    title: command.to_string(),
                    detail: localization::palette_history_detail(scope),
                    source: AutocompleteSource::History,
                    pinned: false,
                    category: PaletteCategory::Commands,
                    action: PaletteAction::RunCommand,
                    status: None,
                    highlights: Vec::new(),
                },
                match_kind,
                ordinal,
                source_priority: 0,
            });
        }
    }

    for (ordinal, command) in command_history.iter().rev().enumerate() {
        let command = command.trim();
        let Some(match_kind) = palette_match_kind(&query, &[command]) else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    id: command_palette_result_id(1, command),
                    command: command.to_string(),
                    title: command.to_string(),
                    detail: localization::palette_recent_command(),
                    source: AutocompleteSource::History,
                    pinned: false,
                    category: PaletteCategory::Commands,
                    action: PaletteAction::RunCommand,
                    status: None,
                    highlights: Vec::new(),
                },
                match_kind,
                ordinal,
                source_priority: 1,
            });
        }
    }

    for (ordinal, snippet) in snippets.iter().enumerate() {
        let command = snippet.command.trim();
        let title = snippet.display_name();
        let Some(match_kind) = palette_match_kind(&query, &[command, &title, &snippet.group])
        else {
            continue;
        };
        let key = command.to_ascii_lowercase();
        if seen.insert(key) {
            let mut detail = if snippet.pinned {
                localization::palette_pinned_snippet_detail(command)
            } else {
                localization::palette_snippet_detail(command)
            };
            if !snippet.group.trim().is_empty() {
                detail = if snippet.pinned {
                    localization::palette_pinned_group_snippet_detail(snippet.group.trim(), command)
                } else {
                    localization::palette_group_snippet_detail(snippet.group.trim(), command)
                };
            }
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    id: command_palette_result_id(1, command),
                    command: command.to_string(),
                    title,
                    detail,
                    source: AutocompleteSource::Snippet,
                    pinned: snippet.pinned,
                    category: PaletteCategory::Commands,
                    action: PaletteAction::RunCommand,
                    status: None,
                    highlights: Vec::new(),
                },
                match_kind,
                ordinal,
                source_priority: if snippet.pinned { 1 } else { 2 },
            });
        }
    }

    if let Some(context_suggestions) =
        collect_context_command_templates(query.as_str(), output_context)
    {
        for template in context_suggestions {
            let Some(match_kind) =
                palette_match_kind(&query, &[&template.command, &template.detail])
            else {
                continue;
            };
            let key = template.command.to_ascii_lowercase();
            if seen.insert(key) {
                suggestions.push(ScoredPaletteCandidate {
                    candidate: CommandPaletteCandidate {
                        id: command_palette_result_id(1, &template.command),
                        command: template.command.clone(),
                        title: template.command,
                        detail: template.detail,
                        source: AutocompleteSource::Context,
                        pinned: false,
                        category: PaletteCategory::Commands,
                        action: PaletteAction::RunCommand,
                        status: None,
                        highlights: Vec::new(),
                    },
                    match_kind,
                    ordinal: template.ordinal,
                    source_priority: 2u8.saturating_add(template.rank),
                });
            }
        }
    }

    for (ordinal, template) in builtin_command_templates().iter().enumerate() {
        let Some(match_kind) = palette_match_kind(&query, &[template.command, template.detail])
        else {
            continue;
        };
        let key = template.command.to_ascii_lowercase();
        if seen.insert(key) {
            suggestions.push(ScoredPaletteCandidate {
                candidate: CommandPaletteCandidate {
                    id: command_palette_result_id(1, template.command),
                    command: template.command.to_string(),
                    title: template.command.to_string(),
                    detail: template.detail.to_string(),
                    source: template.source,
                    pinned: false,
                    category: PaletteCategory::Commands,
                    action: PaletteAction::RunCommand,
                    status: None,
                    highlights: Vec::new(),
                },
                match_kind,
                ordinal,
                source_priority: match template.source {
                    AutocompleteSource::Argument => 3,
                    _ => 4,
                },
            });
        }
    }

    suggestions.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| left.source_priority.cmp(&right.source_priority))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
            .then_with(|| {
                left.candidate
                    .title
                    .to_ascii_lowercase()
                    .cmp(&right.candidate.title.to_ascii_lowercase())
            })
    });

    suggestions
        .into_iter()
        .take(10)
        .map(|candidate| candidate.candidate)
        .collect()
}

pub(super) fn collect_path_autocomplete_candidates(
    input: &str,
    path_context: Option<&PathSuggestionContext>,
    command_history: &[String],
    snippets: &[SavedSnippet],
) -> Option<Vec<AutocompleteCandidate>> {
    let query = path_query_context(input)?;
    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();

    let mut push_candidate =
        |path_value: String, scope_label: Option<String>, is_dir: bool, ordinal: usize| {
            let mut candidate_path = path_value;
            if is_dir && !candidate_path.ends_with('/') {
                candidate_path.push('/');
            }
            let Some(match_kind) = path_match_kind(&query.fragment, &candidate_path) else {
                return None;
            };
            let full_command = format!("{}{}", query.prefix, candidate_path);
            if seen.insert(full_command.to_ascii_lowercase()) {
                suggestions.push((
                    AutocompleteCandidate {
                        command: full_command,
                        source: AutocompleteSource::Path,
                        scope_label,
                    },
                    match_kind,
                    ordinal,
                ));
            }
            Some(())
        };

    let mut ordinal = 0usize;
    if let Some(context) = path_context {
        let current_path = context
            .current_path
            .clone()
            .unwrap_or_else(|| ".".to_string());
        let scope = Some(localization::palette_files_scope(current_path));

        for entry in &context.entries {
            let candidate_path = if query.fragment.starts_with('/') {
                entry.path.clone()
            } else if query.fragment.starts_with("./") {
                format!("./{}", entry.name)
            } else {
                entry.name.clone()
            };
            let _ = push_candidate(candidate_path, scope.clone(), entry.is_dir, ordinal);
            ordinal += 1;
        }

        if let Some(startup_directory) = context.startup_directory.clone() {
            let _ = push_candidate(
                startup_directory,
                Some(localization::palette_startup_path()),
                true,
                ordinal,
            );
            ordinal += 1;
        }
        if let Some(current_path) = context.current_path.clone() {
            let _ = push_candidate(
                current_path.clone(),
                Some(localization::palette_current_directory()),
                true,
                ordinal,
            );
            ordinal += 1;
            if let Some(parent) = remote_parent_path(&current_path) {
                let _ = push_candidate(
                    parent,
                    Some(localization::palette_parent_directory()),
                    true,
                    ordinal,
                );
                ordinal += 1;
            }
        }
    }

    for path in command_history
        .iter()
        .flat_map(|command| extract_path_tokens(command))
        .chain(
            snippets
                .iter()
                .flat_map(|snippet| extract_path_tokens(&snippet.command)),
        )
    {
        let is_dir = path.ends_with('/');
        let _ = push_candidate(
            path,
            Some(localization::palette_recent_path()),
            is_dir,
            ordinal,
        );
        ordinal += 1;
    }

    suggestions.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| {
                left.0
                    .command
                    .to_ascii_lowercase()
                    .cmp(&right.0.command.to_ascii_lowercase())
            })
    });

    if suggestions.is_empty() {
        None
    } else {
        Some(
            suggestions
                .into_iter()
                .take(6)
                .map(|(candidate, _, _)| candidate)
                .collect(),
        )
    }
}

pub(super) fn collect_argument_autocomplete_candidates(
    input: &str,
) -> Option<Vec<AutocompleteCandidate>> {
    let query = input.trim();
    if query.is_empty() || query.contains('\n') {
        return None;
    }

    let first = query.split_whitespace().next()?;
    let has_family_templates = builtin_command_templates().iter().any(|template| {
        template.source == AutocompleteSource::Argument
            && template.command.starts_with(first)
            && (template.command == first || template.command.starts_with(&format!("{first} ")))
    });
    if !has_family_templates {
        return None;
    }

    let query_lower = query.to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut suggestions = builtin_command_templates()
        .iter()
        .filter(|template| template.source == AutocompleteSource::Argument)
        .filter_map(|template| {
            let command_lower = template.command.to_ascii_lowercase();
            let match_kind = autocomplete_match_kind(&query_lower, &command_lower)?;
            if !command_lower.starts_with(&first.to_ascii_lowercase())
                || command_lower == query_lower
            {
                return None;
            }
            if !seen.insert(command_lower.clone()) {
                return None;
            }
            Some((
                AutocompleteCandidate {
                    command: template.command.to_string(),
                    source: AutocompleteSource::Argument,
                    scope_label: Some(template.detail.to_string()),
                },
                match_kind,
            ))
        })
        .collect::<Vec<_>>();

    suggestions.sort_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| {
            left.0
                .command
                .to_ascii_lowercase()
                .cmp(&right.0.command.to_ascii_lowercase())
        })
    });

    if suggestions.is_empty() {
        None
    } else {
        Some(
            suggestions
                .into_iter()
                .take(6)
                .map(|(candidate, _)| candidate)
                .collect(),
        )
    }
}

pub(super) fn collect_context_autocomplete_candidates(
    input: &str,
    output_context: Option<&OutputSuggestionContext>,
) -> Option<Vec<AutocompleteCandidate>> {
    let suggestions = collect_context_command_templates(input, output_context)?;
    let mut candidates = suggestions
        .into_iter()
        .filter_map(|template| {
            let command = template.command;
            let command_lower = command.to_ascii_lowercase();
            let match_kind =
                autocomplete_match_kind(&input.trim().to_ascii_lowercase(), &command_lower)?;
            Some((
                AutocompleteCandidate {
                    command,
                    source: AutocompleteSource::Context,
                    scope_label: Some(template.detail),
                },
                match_kind,
                template.rank,
                template.ordinal,
            ))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| {
            left.2
                .cmp(&right.2)
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| {
                    left.0
                        .command
                        .to_ascii_lowercase()
                        .cmp(&right.0.command.to_ascii_lowercase())
                })
        })
    });

    if candidates.is_empty() {
        None
    } else {
        Some(
            candidates
                .into_iter()
                .take(6)
                .map(|(candidate, _, _, _)| candidate)
                .collect(),
        )
    }
}

pub(super) fn collect_context_command_templates(
    input: &str,
    output_context: Option<&OutputSuggestionContext>,
) -> Option<Vec<ContextCommandTemplate>> {
    let output_context = output_context?;
    let raw_query = input.trim_end_matches(['\r', '\n']);
    let query = raw_query.trim();
    if query.is_empty() || raw_query.contains('\n') || output_context.recent_lines.is_empty() {
        return None;
    }

    let query_lower = query.to_ascii_lowercase();
    let path_hint = current_path_hint(output_context.current_path.as_deref());
    let mut templates = Vec::new();
    let mut push_templates = |prefix: &str, targets: Vec<String>, kind: &str| {
        for (ordinal, target) in targets.into_iter().enumerate() {
            templates.push(ContextCommandTemplate {
                command: format!("{prefix}{target}"),
                detail: context_detail(kind, output_context.current_path.as_deref()),
                rank: context_target_rank(&target, path_hint.as_deref()),
                ordinal,
            });
        }
    };

    if matches_command_prefix(&query_lower, "git checkout")
        || matches_command_prefix(&query_lower, "git switch")
    {
        let prefix = if matches_command_prefix(&query_lower, "git switch") {
            "git switch "
        } else {
            "git checkout "
        };
        push_templates(
            prefix,
            extract_git_branch_targets(&output_context.recent_lines),
            &localization::palette_git_branch(),
        );
    } else if matches_command_prefix(&query_lower, "git diff")
        || matches_command_prefix(&query_lower, "git log")
    {
        let prefix = if matches_command_prefix(&query_lower, "git log") {
            "git log "
        } else {
            "git diff "
        };
        push_templates(
            prefix,
            extract_git_branch_targets(&output_context.recent_lines),
            &localization::palette_git_branch(),
        );
    } else if matches_command_prefix(&query_lower, "docker logs")
        || matches_command_prefix(&query_lower, "docker inspect")
        || matches_command_prefix(&query_lower, "docker stop")
        || matches_command_prefix(&query_lower, "docker restart")
        || matches_command_prefix(&query_lower, "docker rm")
        || matches_command_prefix(&query_lower, "docker exec -it")
    {
        let prefix = if matches_command_prefix(&query_lower, "docker exec -it") {
            "docker exec -it "
        } else if matches_command_prefix(&query_lower, "docker inspect") {
            "docker inspect "
        } else if matches_command_prefix(&query_lower, "docker stop") {
            "docker stop "
        } else if matches_command_prefix(&query_lower, "docker restart") {
            "docker restart "
        } else if matches_command_prefix(&query_lower, "docker rm") {
            "docker rm "
        } else {
            "docker logs "
        };
        push_templates(
            prefix,
            extract_docker_targets(&output_context.recent_lines),
            &localization::palette_docker_target(),
        );
    } else if matches_command_prefix(&query_lower, "kubectl logs")
        || matches_command_prefix(&query_lower, "kubectl describe pod")
        || matches_command_prefix(&query_lower, "kubectl exec -it")
    {
        let prefix = if matches_command_prefix(&query_lower, "kubectl describe pod") {
            "kubectl describe pod "
        } else if matches_command_prefix(&query_lower, "kubectl exec -it") {
            "kubectl exec -it "
        } else {
            "kubectl logs "
        };
        push_templates(
            prefix,
            extract_kubernetes_pod_targets(&output_context.recent_lines),
            &localization::palette_kubernetes_pod(),
        );
    } else if matches_command_prefix(&query_lower, "systemctl status")
        || matches_command_prefix(&query_lower, "systemctl restart")
        || matches_command_prefix(&query_lower, "systemctl reload")
        || matches_command_prefix(&query_lower, "journalctl -u")
        || matches_command_prefix(&query_lower, "journalctl -f -u")
    {
        let prefix = if matches_command_prefix(&query_lower, "systemctl restart") {
            "systemctl restart "
        } else if matches_command_prefix(&query_lower, "systemctl reload") {
            "systemctl reload "
        } else if matches_command_prefix(&query_lower, "journalctl -f -u") {
            "journalctl -f -u "
        } else if matches_command_prefix(&query_lower, "journalctl -u") {
            "journalctl -u "
        } else {
            "systemctl status "
        };
        push_templates(
            prefix,
            extract_systemd_unit_targets(&output_context.recent_lines),
            &localization::palette_systemd_unit(),
        );
    }

    if templates.is_empty() {
        None
    } else {
        let mut seen = HashSet::new();
        let mut deduped = templates
            .into_iter()
            .filter(|template| seen.insert(template.command.to_ascii_lowercase()))
            .collect::<Vec<_>>();
        deduped.sort_by(|left, right| {
            left.rank
                .cmp(&right.rank)
                .then_with(|| left.ordinal.cmp(&right.ordinal))
                .then_with(|| {
                    left.command
                        .to_ascii_lowercase()
                        .cmp(&right.command.to_ascii_lowercase())
                })
        });
        Some(deduped)
    }
}

pub(super) fn pane_recent_output_lines(pane: &SessionPane, limit: usize) -> Vec<String> {
    let mut lines = pane
        .terminal
        .all_rows_text()
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let keep_from = lines.len().saturating_sub(limit);
    lines.drain(0..keep_from);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{
        HostedSessionId, PositionKey, ScoreTuple, SearchAction, SearchCategory, SearchDocumentId,
        SearchResult, SearchStatus, TextHighlight,
    };

    #[test]
    fn command_candidates_remain_in_the_commands_category() {
        let candidates = collect_command_palette_candidates(
            "git status",
            &["git status".to_string()],
            &[],
            "local:test",
            &[],
            None,
        );
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|candidate| {
            candidate.category == PaletteCategory::Commands
                && candidate.action == PaletteAction::RunCommand
                && candidate.status.is_none()
                && candidate.highlights.is_empty()
        }));
    }

    #[test]
    fn search_candidates_preserve_category_status_and_highlights() {
        let id = HostedSessionId::new();
        let document_id = SearchDocumentId::Session(id);
        let highlight = TextHighlight {
            field: termirust_domain::HighlightField::Title,
            start: 0,
            end: 4,
        };
        let result = SearchResult {
            id: document_id,
            category: SearchCategory::Archive,
            title: "Build retained".to_string(),
            project_label: Some("Console".to_string()),
            group_label: Some("Auth".to_string()),
            preset_label: None,
            runtime_label: Some("codex".to_string()),
            status: SearchStatus::Done,
            pinned: true,
            archived: true,
            highlights: vec![highlight],
            action: SearchAction::OpenSession(id),
            score: ScoreTuple {
                match_quality: 3,
                current_project: 1,
                actionable_status: 1,
                pinned: 1,
                position: PositionKey::FIRST,
                meaningful_activity_at: 1,
                id: document_id,
            },
        };

        let candidate = super::super::global_search::search_result_candidate(&result);
        assert_eq!(candidate.category, PaletteCategory::Archive);
        assert_eq!(candidate.status, Some(SearchStatus::Done));
        assert_eq!(candidate.detail, "Console / Auth / codex");
        assert_eq!(candidate.highlights, vec![highlight]);
        assert!(candidate.pinned);
        assert_eq!(
            candidate.action,
            PaletteAction::Search(SearchAction::OpenSession(id))
        );
    }

    #[test]
    fn category_rank_is_total_and_stable() {
        let categories = [
            PaletteCategory::Attention,
            PaletteCategory::Sessions,
            PaletteCategory::Projects,
            PaletteCategory::Groups,
            PaletteCategory::Presets,
            PaletteCategory::Actions,
            PaletteCategory::Archive,
            PaletteCategory::Commands,
        ];
        assert_eq!(
            categories.map(PaletteCategory::rank),
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
    }
}
