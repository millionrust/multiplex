use std::fs;
use std::path::{Path, PathBuf};

const MARKER_PREFIX: &str = "// termirust-ui-surface:";

#[derive(Clone, Copy)]
pub struct SurfaceFile {
    pub path: &'static str,
    pub marked: bool,
}

pub fn files_for_surface(surface: &str) -> Option<&'static [SurfaceFile]> {
    match surface {
        "vault-keys-snippets" => Some(&[
            SurfaceFile {
                path: "src/ui/app/key_lifecycle.rs",
                marked: false,
            },
            SurfaceFile {
                path: "src/ui/app/vault_key_snippet.rs",
                marked: false,
            },
            SurfaceFile {
                path: "src/ui/snippet.rs",
                marked: false,
            },
            SurfaceFile {
                path: "src/ui/app/library.rs",
                marked: true,
            },
            SurfaceFile {
                path: "src/ui/app/mod.rs",
                marked: true,
            },
        ]),
        _ => None,
    }
}

pub fn read_surface_sources(root: &Path, surface: &str) -> Result<Vec<(String, String)>, String> {
    let files = files_for_surface(surface)
        .ok_or_else(|| format!("unknown scoped UI surface {surface:?}"))?;
    let mut sources = Vec::with_capacity(files.len());
    for file in files {
        let relative = PathBuf::from(file.path);
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("unable to inspect {}: {error}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!("{} must be a regular source file", path.display()));
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("unable to read {}: {error}", path.display()))?;
        let source = if file.marked {
            extract_marked_source(&source, surface)?
        } else {
            source
        };
        sources.push((file.path.to_string(), source));
    }
    Ok(sources)
}

fn extract_marked_source(source: &str, surface: &str) -> Result<String, String> {
    let start = format!("{MARKER_PREFIX}{surface}:start");
    let end = format!("{MARKER_PREFIX}{surface}:end");
    let mut active = false;
    let mut ranges = 0usize;
    let mut output = String::with_capacity(source.len());
    for line in source.lines() {
        if line.trim() == start {
            if active {
                return Err(format!("nested surface marker {start:?}"));
            }
            active = true;
            ranges += 1;
            output.push('\n');
        } else if line.trim() == end {
            if !active {
                return Err(format!("surface marker {end:?} has no start"));
            }
            active = false;
            output.push('\n');
        } else if active {
            output.push_str(line);
            output.push('\n');
        } else {
            // Preserve line numbers so diagnostics still point to the real file.
            output.push('\n');
        }
    }
    if active {
        return Err(format!("surface marker {start:?} has no end"));
    }
    if ranges == 0 {
        return Err(format!("no {surface:?} markers found in shared source"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::extract_marked_source;

    #[test]
    fn marked_source_preserves_owned_lines_and_line_numbers() {
        let source = "ignored\n// termirust-ui-surface:demo:start\nowned\n// termirust-ui-surface:demo:end\nignored";
        let extracted = extract_marked_source(source, "demo").expect("valid markers");
        assert_eq!(extracted.lines().nth(2), Some("owned"));
        assert!(!extracted.contains("ignored"));
    }
}
