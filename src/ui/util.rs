//! Pure helpers shared by the UI layer.
//!
//! Only put functions here that have no dependency on `TShellApp`,
//! window/cx state, or other UI rendering primitives. Anything that needs
//! the app, panes, or saved state stays in `app.rs` (for now).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn format_relative_time(timestamp_ms: u64) -> String {
    format_relative_time_for(timestamp_ms, current_unix_millis())
}

pub fn format_relative_time_for(timestamp_ms: u64, now_ms: u64) -> String {
    if timestamp_ms == 0 || timestamp_ms > now_ms {
        return "just now".to_string();
    }
    let delta_secs = (now_ms - timestamp_ms) / 1000;
    if delta_secs < 60 {
        return "just now".to_string();
    }
    let minutes = delta_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{days}d ago");
    }
    if days < 30 {
        let weeks = days / 7;
        return if weeks == 1 {
            "1w ago".to_string()
        } else {
            format!("{weeks}w ago")
        };
    }
    if days < 365 {
        let months = days / 30;
        return if months == 1 {
            "1mo ago".to_string()
        } else {
            format!("{months}mo ago")
        };
    }
    let years = days / 365;
    if years == 1 {
        "1y ago".to_string()
    } else {
        format!("{years}y ago")
    }
}

pub fn format_modified_time(secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(secs);
    let diff = now.saturating_sub(secs);
    let days = diff / 86_400;
    if days < 7 {
        if days == 0 {
            "Today".to_string()
        } else if days == 1 {
            "Yesterday".to_string()
        } else {
            format!("{days} days ago")
        }
    } else {
        let weeks = days / 7;
        if weeks < 4 {
            format!("{weeks}w ago")
        } else {
            format!("{}mo ago", days / 30)
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

#[allow(dead_code)]
pub fn format_count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

pub fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn parse_tag_values(raw: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for raw in raw.split(',') {
        let tag = raw.trim().trim_start_matches('#');
        if tag.is_empty() {
            continue;
        }
        if !tags
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            tags.push(tag.to_string());
        }
    }
    tags
}

pub fn format_tag_values(tags: &[String]) -> String {
    tags.join(", ")
}

pub fn merge_tag_values(current: &[String], inherited: &[String]) -> Vec<String> {
    let mut merged = current.to_vec();
    for tag in inherited {
        if !merged
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            merged.push(tag.clone());
        }
    }
    merged
}

pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')
}

pub fn short_host_key(key: &str) -> String {
    if key.len() <= 40 {
        return key.to_string();
    }
    format!("{}…{}", &key[..18], &key[key.len() - 18..])
}
