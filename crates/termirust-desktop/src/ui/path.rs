//! Path + file-size utility helpers.

pub fn remote_parent_path(path: &str) -> Option<String> {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() || path == "." || path == "/" {
        return None;
    }

    if let Some((parent, _)) = path.rsplit_once('/') {
        if parent.is_empty() {
            Some("/".to_string())
        } else {
            Some(parent.to_string())
        }
    } else {
        Some(".".to_string())
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
