//! Local-filesystem helpers for the SFTP page (read directory listings).

use std::cmp::Ordering;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub struct SftpLocalEntry {
    pub name: String,
    pub is_dir: bool,
    pub modified: Option<u64>,
    pub size: u64,
}

pub fn read_local_dir(path: &Path) -> Vec<SftpLocalEntry> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let metadata = entry.metadata().ok();
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            out.push(SftpLocalEntry {
                name,
                is_dir: metadata.as_ref().map_or(false, |m| m.is_dir()),
                modified,
                size: metadata.as_ref().map_or(0, |m| m.len()),
            });
        }
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase()),
    });
    out
}
