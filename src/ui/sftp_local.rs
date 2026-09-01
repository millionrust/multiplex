//! Local-filesystem helpers for the SFTP page (read directory listings).

use std::cmp::Ordering;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug)]
pub struct SftpLocalEntry {
    pub name: String,
    pub is_dir: bool,
    pub modified: Option<u64>,
    pub size: u64,
}

pub fn read_local_dir(path: &Path) -> Vec<SftpLocalEntry> {
    read_local_dir_result(path).unwrap_or_default()
}

pub fn read_local_dir_result(path: &Path) -> io::Result<Vec<SftpLocalEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else {
            continue;
        };
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
            is_dir: metadata.as_ref().is_some_and(|m| m.is_dir()),
            modified,
            size: metadata.as_ref().map_or(0, |m| m.len()),
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase()),
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_preserves_read_errors_instead_of_presenting_an_empty_folder() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert_eq!(
            read_local_dir_result(&missing).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(read_local_dir(&missing).is_empty());
    }

    #[test]
    fn directories_sort_first_and_hidden_entries_remain_excluded() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("z-file"), b"z").unwrap();
        std::fs::create_dir(temp.path().join("a-dir")).unwrap();
        std::fs::write(temp.path().join(".hidden"), b"hidden").unwrap();
        let entries = read_local_dir_result(temp.path()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-dir", "z-file"]
        );
    }
}
