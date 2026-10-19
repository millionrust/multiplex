//! Command palette candidate collection. The actual UI render lives in
//! `app::overlay`. This module owns the data-side types and pure collect_*
//! helpers that scan history, snippets, recent output, etc.

use multiplex_ui_contract::PaletteResultId;
use std::collections::HashSet;

use crate::models::{SavedCommandHistoryEntry, SavedSnippet};
use crate::ui::autocomplete::{
    AutocompleteMatchKind, AutocompleteSource, builtin_command_templates, context_detail,
    context_target_rank, current_path_hint, extract_docker_targets, extract_git_branch_targets,
    extract_kubernetes_pod_targets, extract_systemd_unit_targets, matches_command_prefix,
    palette_match_kind,
};
use crate::ui::localization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaletteCategory {
    Attention,
    Sessions,
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
            Self::Groups => 2,
            Self::Presets => 3,
            Self::Actions => 4,
            Self::Archive => 5,
            Self::Commands => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaletteAction {
    Search(multiplex_domain::SearchAction),
    RunCommand,
    Layout(LayoutCommand),
}

/// A split or canvas action offered in the palette, next to its shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LayoutCommand {
    SplitRight,
    SplitDown,
    ZoomPane,
    Equalize,
    Preset(super::split_tree::SplitPreset),
    ShowCanvas,
    ShowSplit,
    FitCanvas,
    TidyCanvas,
    ToggleBroadcast,
    ToggleSnap,
}

impl LayoutCommand {
    /// The commands that make sense for a workspace in `canvas` or split view.
    pub(super) fn available(canvas: bool) -> Vec<LayoutCommand> {
        let mut commands = vec![
            LayoutCommand::SplitRight,
            LayoutCommand::SplitDown,
            LayoutCommand::ZoomPane,
            LayoutCommand::Equalize,
        ];
        commands.extend(
            super::split_tree::SplitPreset::ALL
                .into_iter()
                .map(LayoutCommand::Preset),
        );
        commands.push(if canvas {
            LayoutCommand::ShowSplit
        } else {
            LayoutCommand::ShowCanvas
        });
        commands.extend([
            LayoutCommand::FitCanvas,
            LayoutCommand::TidyCanvas,
            LayoutCommand::ToggleBroadcast,
            LayoutCommand::ToggleSnap,
        ]);
        commands
    }

    pub(super) fn title(self) -> String {
        use multiplex_ui_contract::MessageId;
        let message = |id| localization::static_message(id).to_string();
        match self {
            LayoutCommand::SplitRight => message(MessageId::PaletteSplitRight),
            LayoutCommand::SplitDown => message(MessageId::PaletteSplitDown),
            LayoutCommand::ZoomPane => message(MessageId::PaletteZoomPane),
            LayoutCommand::Equalize => message(MessageId::PaletteEqualize),
            LayoutCommand::Preset(preset) => localization::dynamic_user_data_message(
                MessageId::PaletteLayoutPreset,
                vec![message(super::workspace::preset_message(preset))],
            ),
            LayoutCommand::ShowCanvas => message(MessageId::PaletteShowCanvas),
            LayoutCommand::ShowSplit => message(MessageId::PaletteShowSplit),
            LayoutCommand::FitCanvas => message(MessageId::PaletteFitCanvas),
            LayoutCommand::TidyCanvas => message(MessageId::PaletteTidyCanvas),
            LayoutCommand::ToggleBroadcast => message(MessageId::PaletteToggleBroadcast),
            LayoutCommand::ToggleSnap => message(MessageId::PaletteToggleSnap),
        }
    }

    /// The shortcut, shown so the palette teaches it.
    pub(super) fn shortcut(self) -> &'static str {
        match self {
            LayoutCommand::SplitRight => "Cmd+D",
            LayoutCommand::SplitDown => "Cmd+Shift+D",
            LayoutCommand::ZoomPane => "Cmd+Shift+Enter",
            LayoutCommand::Equalize => "Cmd+Shift+E",
            LayoutCommand::ShowCanvas | LayoutCommand::ShowSplit => "",
            LayoutCommand::FitCanvas => "Cmd+Option+1",
            LayoutCommand::TidyCanvas => "Cmd+Option+T",
            LayoutCommand::ToggleBroadcast => "Cmd+Shift+B",
            LayoutCommand::Preset(_) | LayoutCommand::ToggleSnap => "",
        }
    }
}

/// Layout commands whose titles hold every word of `query`, as palette entries.
pub(super) fn layout_command_candidates(query: &str, canvas: bool) -> Vec<CommandPaletteCandidate> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    LayoutCommand::available(canvas)
        .into_iter()
        .filter_map(|command| {
            let title = command.title();
            let lower = title.to_lowercase();
            if !words.iter().all(|word| lower.contains(word.as_str())) {
                return None;
            }
            Some(CommandPaletteCandidate {
                id: command_palette_result_id(3, &format!("{command:?}")),
                command: String::new(),
                title,
                detail: command.shortcut().to_string(),
                source: AutocompleteSource::Builtin,
                pinned: false,
                category: PaletteCategory::Actions,
                action: PaletteAction::Layout(command),
                status: None,
                highlights: Vec::new(),
            })
        })
        .collect()
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
    pub(super) status: Option<multiplex_domain::SearchStatus>,
    pub(super) highlights: Vec<multiplex_domain::TextHighlight>,
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

/// Submit through the same Enter byte as keyboard input. In ConPTY, a bare LF can
/// be echoed without accepting the command in an interactive shell.
pub(super) fn command_submission_bytes(command: &str) -> Vec<u8> {
    let mut bytes = command.trim_end_matches(['\r', '\n']).as_bytes().to_vec();
    bytes.push(b'\r');
    bytes
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_submission_uses_enter_and_preserves_literal_command_content() {
        for ending in ["", "\n", "\r", "\r\n"] {
            assert_eq!(
                super::command_submission_bytes(&format!("echo palette-replay-ok{ending}")),
                b"echo palette-replay-ok\r"
            );
        }
        assert_eq!(
            String::from_utf8(super::command_submission_bytes("printf 'héllo\\n'\n")).unwrap(),
            "printf 'héllo\\n'\r"
        );
    }

    use super::*;
    use multiplex_domain::{
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
            field: multiplex_domain::HighlightField::Title,
            start: 0,
            end: 4,
        };
        let result = SearchResult {
            id: document_id,
            category: SearchCategory::Archive,
            title: "Build retained".to_string(),
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
        assert_eq!(candidate.detail, "Auth / codex");
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
            PaletteCategory::Groups,
            PaletteCategory::Presets,
            PaletteCategory::Actions,
            PaletteCategory::Archive,
            PaletteCategory::Commands,
        ];
        assert_eq!(categories.map(PaletteCategory::rank), [0, 1, 2, 3, 4, 5, 6]);
    }
}
