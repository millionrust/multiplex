use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A path in its one true form, written the way every tool that receives it can use. Windows
/// canonical paths start with the `\\?\` verbatim prefix: Git reads that back as `//?/C:/...` and
/// refuses to create directories under it, and a path written the ordinary way never compares
/// equal to one written with it, so containment checks on a canonical root fail.
pub fn canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = canonical.as_os_str().to_string_lossy();
        if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{share}")));
        }
        if let Some(drive) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(drive));
        }
    }
    Ok(canonical)
}

pub const MAX_LABEL_SCALARS: usize = 256;
pub const MAX_PATH_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileIdentity {
    Unix { device: u64, inode: u64 },
    CanonicalPath { comparison_key: String },
}

/// Whether a folder is still there, and still ours to read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathStatus {
    Available,
    Unavailable,
    PermissionDenied,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct CanonicalPath {
    path: PathBuf,
    identity: FileIdentity,
}

impl fmt::Debug for CanonicalPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalPath")
            .field("path", &"<redacted>")
            .field("identity", &"<redacted>")
            .finish()
    }
}

impl CanonicalPath {
    pub fn resolve(input: &Path) -> Result<Self, PathError> {
        validate_path_input(input)?;
        let selected_metadata = fs::symlink_metadata(input).map_err(map_path_error)?;
        if !selected_metadata.file_type().is_dir() && !selected_metadata.file_type().is_symlink() {
            return Err(PathError::NotDirectory);
        }

        let path = canonical_path(input).map_err(map_path_error)?;
        let encoded = path.to_str().ok_or(PathError::NonUnicodePath)?;
        if encoded.len() > MAX_PATH_BYTES {
            return Err(PathError::PathTooLong);
        }

        let metadata = fs::metadata(&path).map_err(map_path_error)?;
        if !metadata.is_dir() {
            return Err(PathError::NotDirectory);
        }
        let mut entries = fs::read_dir(&path).map_err(map_path_error)?;
        let _ = entries.next().transpose().map_err(map_path_error)?;
        let identity = identity_for(&path, &metadata)?;

        let metadata_after = fs::metadata(&path).map_err(map_path_error)?;
        if identity_for(&path, &metadata_after)? != identity {
            return Err(PathError::PathChanged);
        }

        Ok(Self { path, identity })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    pub fn display_name(&self) -> Result<LocalizedUserText, PathError> {
        let candidate = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.path.to_str().unwrap_or("Folder"));
        LocalizedUserText::new(candidate)
    }

    pub fn status(&self) -> PathStatus {
        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() => match identity_for(&self.path, &metadata) {
                Ok(identity) if identity == self.identity => match fs::read_dir(&self.path) {
                    Ok(_) => PathStatus::Available,
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        PathStatus::PermissionDenied
                    }
                    Err(_) => PathStatus::Unavailable,
                },
                _ => PathStatus::Unavailable,
            },
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                PathStatus::PermissionDenied
            }
            _ => PathStatus::Unavailable,
        }
    }
}

fn validate_path_input(path: &Path) -> Result<(), PathError> {
    let encoded = path.to_str().ok_or(PathError::NonUnicodePath)?;
    if encoded.trim().is_empty() {
        return Err(PathError::EmptyPath);
    }
    if encoded.contains('\0') {
        return Err(PathError::PathContainsNul);
    }
    if encoded.len() > MAX_PATH_BYTES {
        return Err(PathError::PathTooLong);
    }
    Ok(())
}

#[cfg(unix)]
fn identity_for(_path: &Path, metadata: &fs::Metadata) -> Result<FileIdentity, PathError> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(FileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn identity_for(path: &Path, _metadata: &fs::Metadata) -> Result<FileIdentity, PathError> {
    let encoded = path.to_str().ok_or(PathError::NonUnicodePath)?;
    #[cfg(target_os = "windows")]
    let comparison_key = encoded.to_lowercase();
    #[cfg(not(target_os = "windows"))]
    let comparison_key = encoded.to_string();
    Ok(FileIdentity::CanonicalPath { comparison_key })
}

fn map_path_error(error: std::io::Error) -> PathError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => PathError::PermissionDenied,
        std::io::ErrorKind::NotFound => PathError::Unavailable,
        _ => PathError::PathValidation,
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LocalizedUserText(String);

impl LocalizedUserText {
    pub fn new(value: &str) -> Result<Self, PathError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(PathError::EmptyLabel);
        }
        if value.contains('\0') {
            return Err(PathError::LabelContainsNul);
        }
        if value.chars().count() > MAX_LABEL_SCALARS {
            return Err(PathError::LabelTooLong);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocalizedUserText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for LocalizedUserText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LocalizedUserText(<redacted>)")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathError {
    EmptyPath,
    PathContainsNul,
    PathTooLong,
    NonUnicodePath,
    NotDirectory,
    PermissionDenied,
    Unavailable,
    PathChanged,
    PathValidation,
    EmptyLabel,
    LabelContainsNul,
    LabelTooLong,
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("path is empty"),
            Self::PathContainsNul => formatter.write_str("path contains NUL"),
            Self::PathTooLong => formatter.write_str("path exceeds the platform limit"),
            Self::NonUnicodePath => formatter.write_str("path cannot be represented safely"),
            Self::NotDirectory => formatter.write_str("path is not a directory"),
            Self::PermissionDenied => formatter.write_str("folder permission was denied"),
            Self::Unavailable => formatter.write_str("folder is unavailable"),
            Self::PathChanged => formatter.write_str("folder changed during validation"),
            Self::PathValidation => formatter.write_str("folder validation failed"),
            Self::EmptyLabel => formatter.write_str("label is empty"),
            Self::LabelContainsNul => formatter.write_str("label contains NUL"),
            Self::LabelTooLong => formatter.write_str("label exceeds 256 characters"),
        }
    }
}

impl std::error::Error for PathError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_trimmed_bounded_user_data() {
        assert_eq!(
            LocalizedUserText::new("  Console  ").unwrap().as_str(),
            "Console"
        );
        assert_eq!(LocalizedUserText::new("  "), Err(PathError::EmptyLabel));
        assert_eq!(
            LocalizedUserText::new(&"x".repeat(257)),
            Err(PathError::LabelTooLong)
        );
    }

    #[test]
    fn canonical_path_resolves_directory_and_stable_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let canonical = CanonicalPath::resolve(fixture.path()).unwrap();
        // The same directory, written the way every tool can use: Windows canonical paths carry
        // a `\\?\` prefix that Git refuses and that no other spelling compares equal to.
        assert_eq!(
            fs::canonicalize(canonical.as_path()).unwrap(),
            fs::canonicalize(fixture.path()).unwrap()
        );
        assert!(!canonical.as_path().to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(canonical.status(), PathStatus::Available);
    }

    #[test]
    fn canonical_path_rejects_regular_files_and_missing_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("file");
        fs::write(&file, b"sentinel").unwrap();
        assert_eq!(CanonicalPath::resolve(&file), Err(PathError::NotDirectory));
        assert_eq!(
            CanonicalPath::resolve(&fixture.path().join("missing")),
            Err(PathError::Unavailable)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_filesystem_identity() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let real = fixture.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = fixture.path().join("alias");
        symlink(&real, &alias).unwrap();
        assert_eq!(
            CanonicalPath::resolve(&real).unwrap().identity(),
            CanonicalPath::resolve(&alias).unwrap().identity()
        );
    }

    #[test]
    fn unavailable_status_does_not_mutate_the_record() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("project");
        fs::create_dir(&path).unwrap();
        let canonical = CanonicalPath::resolve(&path).unwrap();
        fs::remove_dir(&path).unwrap();
        assert_eq!(canonical.status(), PathStatus::Unavailable);
    }

    #[test]
    fn a_folder_and_its_label_are_redacted_from_debug() {
        let fixture = tempfile::tempdir().unwrap();
        let secret_path = fixture.path().join("customer-secret-folder");
        fs::create_dir(&secret_path).unwrap();
        let canonical = CanonicalPath::resolve(&secret_path).unwrap();
        let label = LocalizedUserText::new("Confidential Client").unwrap();
        assert!(!format!("{canonical:?}").contains("customer-secret-folder"));
        assert!(!format!("{label:?}").contains("Confidential Client"));
    }

    #[test]
    fn permission_errors_map_to_stable_code_without_path() {
        assert_eq!(
            map_path_error(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            PathError::PermissionDenied
        );
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_directory_is_retained_as_permission_denied() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("private-project");
        fs::create_dir(&project_root).unwrap();
        let canonical = CanonicalPath::resolve(&project_root).unwrap();
        fs::set_permissions(&project_root, fs::Permissions::from_mode(0o000)).unwrap();

        assert_eq!(canonical.status(), PathStatus::PermissionDenied);
        assert_eq!(
            CanonicalPath::resolve(&project_root),
            Err(PathError::PermissionDenied)
        );

        fs::set_permissions(&project_root, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_fails_without_filesystem_mutation() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let first = fixture.path().join("first");
        let second = fixture.path().join("second");
        symlink(&second, &first).unwrap();
        symlink(&first, &second).unwrap();
        assert_eq!(
            CanonicalPath::resolve(&first),
            Err(PathError::PathValidation)
        );
        assert!(
            fs::symlink_metadata(&first)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(&second)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
