use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{PositionKey, ProjectId, Revision};

pub const MAX_PROJECTS: usize = 1_000;
pub const MAX_LABEL_SCALARS: usize = 256;
pub const MAX_PATH_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileIdentity {
    Unix { device: u64, inode: u64 },
    CanonicalPath { comparison_key: String },
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
    pub fn resolve(input: &Path) -> Result<Self, ProjectError> {
        validate_path_input(input)?;
        let selected_metadata = fs::symlink_metadata(input).map_err(map_path_error)?;
        if !selected_metadata.file_type().is_dir() && !selected_metadata.file_type().is_symlink() {
            return Err(ProjectError::NotDirectory);
        }

        let path = fs::canonicalize(input).map_err(map_path_error)?;
        let encoded = path.to_str().ok_or(ProjectError::NonUnicodePath)?;
        if encoded.len() > MAX_PATH_BYTES {
            return Err(ProjectError::PathTooLong);
        }

        let metadata = fs::metadata(&path).map_err(map_path_error)?;
        if !metadata.is_dir() {
            return Err(ProjectError::NotDirectory);
        }
        let mut entries = fs::read_dir(&path).map_err(map_path_error)?;
        let _ = entries.next().transpose().map_err(map_path_error)?;
        let identity = identity_for(&path, &metadata)?;

        let metadata_after = fs::metadata(&path).map_err(map_path_error)?;
        if identity_for(&path, &metadata_after)? != identity {
            return Err(ProjectError::PathChanged);
        }

        Ok(Self { path, identity })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    pub fn display_name(&self) -> Result<LocalizedUserText, ProjectError> {
        let candidate = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.path.to_str().unwrap_or("Project"));
        LocalizedUserText::new(candidate)
    }

    pub fn status(&self) -> ProjectStatus {
        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() => match identity_for(&self.path, &metadata) {
                Ok(identity) if identity == self.identity => match fs::read_dir(&self.path) {
                    Ok(_) => ProjectStatus::Available,
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        ProjectStatus::PermissionDenied
                    }
                    Err(_) => ProjectStatus::Unavailable,
                },
                _ => ProjectStatus::Unavailable,
            },
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                ProjectStatus::PermissionDenied
            }
            _ => ProjectStatus::Unavailable,
        }
    }
}

fn validate_path_input(path: &Path) -> Result<(), ProjectError> {
    let encoded = path.to_str().ok_or(ProjectError::NonUnicodePath)?;
    if encoded.trim().is_empty() {
        return Err(ProjectError::EmptyPath);
    }
    if encoded.contains('\0') {
        return Err(ProjectError::PathContainsNul);
    }
    if encoded.len() > MAX_PATH_BYTES {
        return Err(ProjectError::PathTooLong);
    }
    Ok(())
}

#[cfg(unix)]
fn identity_for(_path: &Path, metadata: &fs::Metadata) -> Result<FileIdentity, ProjectError> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(FileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn identity_for(path: &Path, _metadata: &fs::Metadata) -> Result<FileIdentity, ProjectError> {
    let encoded = path.to_str().ok_or(ProjectError::NonUnicodePath)?;
    #[cfg(target_os = "windows")]
    let comparison_key = encoded.to_lowercase();
    #[cfg(not(target_os = "windows"))]
    let comparison_key = encoded.to_string();
    Ok(FileIdentity::CanonicalPath { comparison_key })
}

fn map_path_error(error: std::io::Error) -> ProjectError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => ProjectError::PermissionDenied,
        std::io::ErrorKind::NotFound => ProjectError::Unavailable,
        _ => ProjectError::PathValidation,
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LocalizedUserText(String);

impl LocalizedUserText {
    pub fn new(value: &str) -> Result<Self, ProjectError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ProjectError::EmptyLabel);
        }
        if value.contains('\0') {
            return Err(ProjectError::LabelContainsNul);
        }
        if value.chars().count() > MAX_LABEL_SCALARS {
            return Err(ProjectError::LabelTooLong);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectStatus {
    Available,
    Unavailable,
    PermissionDenied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Project {
    pub id: ProjectId,
    pub display_name: LocalizedUserText,
    pub canonical_root: CanonicalPath,
    pub position: PositionKey,
    pub revision: Revision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSummary {
    pub project: Project,
    pub status: ProjectStatus,
}

impl From<Project> for ProjectSummary {
    fn from(project: Project) -> Self {
        let status = project.canonical_root.status();
        Self { project, status }
    }
}

#[derive(Clone)]
pub struct AddProject {
    pub id: ProjectId,
    pub root: PathBuf,
    pub display_name: Option<String>,
    pub expected: Revision,
}

impl fmt::Debug for AddProject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AddProject")
            .field("id", &self.id)
            .field("root", &"<redacted>")
            .field("display_name", &"<redacted>")
            .field("expected", &self.expected)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectError {
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
    AlreadyPresent {
        id: ProjectId,
    },
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    ResourceLimit {
        limit: usize,
    },
    RevisionOverflow,
    Store {
        code: &'static str,
    },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("project path is empty"),
            Self::PathContainsNul => formatter.write_str("project path contains NUL"),
            Self::PathTooLong => formatter.write_str("project path exceeds the platform limit"),
            Self::NonUnicodePath => {
                formatter.write_str("project path cannot be represented safely")
            }
            Self::NotDirectory => formatter.write_str("project path is not a directory"),
            Self::PermissionDenied => formatter.write_str("project folder permission was denied"),
            Self::Unavailable => formatter.write_str("project folder is unavailable"),
            Self::PathChanged => formatter.write_str("project folder changed during validation"),
            Self::PathValidation => formatter.write_str("project folder validation failed"),
            Self::EmptyLabel => formatter.write_str("project label is empty"),
            Self::LabelContainsNul => formatter.write_str("project label contains NUL"),
            Self::LabelTooLong => formatter.write_str("project label exceeds 256 characters"),
            Self::AlreadyPresent { .. } => formatter.write_str("project folder is already present"),
            Self::StaleRevision { .. } => {
                formatter.write_str("project library changed; reload required")
            }
            Self::ResourceLimit { limit } => write!(formatter, "project limit of {limit} reached"),
            Self::RevisionOverflow => formatter.write_str("project revision exhausted"),
            Self::Store { code } => write!(formatter, "project store error ({code})"),
        }
    }
}

impl std::error::Error for ProjectError {}

pub trait ProjectService {
    fn list(&self) -> Result<Vec<ProjectSummary>, ProjectError>;
    fn add(&self, request: AddProject) -> Result<Project, ProjectError>;
    fn remove(&self, id: ProjectId, expected: Revision) -> Result<(), ProjectError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_labels_are_trimmed_bounded_user_data() {
        assert_eq!(
            LocalizedUserText::new("  Console  ").unwrap().as_str(),
            "Console"
        );
        assert_eq!(LocalizedUserText::new("  "), Err(ProjectError::EmptyLabel));
        assert_eq!(
            LocalizedUserText::new(&"x".repeat(257)),
            Err(ProjectError::LabelTooLong)
        );
    }

    #[test]
    fn canonical_path_resolves_directory_and_stable_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let canonical = CanonicalPath::resolve(fixture.path()).unwrap();
        assert_eq!(
            canonical.as_path(),
            fs::canonicalize(fixture.path()).unwrap()
        );
        assert_eq!(canonical.status(), ProjectStatus::Available);
    }

    #[test]
    fn canonical_path_rejects_regular_files_and_missing_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("file");
        fs::write(&file, b"sentinel").unwrap();
        assert_eq!(
            CanonicalPath::resolve(&file),
            Err(ProjectError::NotDirectory)
        );
        assert_eq!(
            CanonicalPath::resolve(&fixture.path().join("missing")),
            Err(ProjectError::Unavailable)
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
        assert_eq!(canonical.status(), ProjectStatus::Unavailable);
    }

    #[test]
    fn sensitive_project_values_are_redacted_from_debug() {
        let fixture = tempfile::tempdir().unwrap();
        let secret_path = fixture.path().join("customer-secret-project");
        fs::create_dir(&secret_path).unwrap();
        let canonical = CanonicalPath::resolve(&secret_path).unwrap();
        let label = LocalizedUserText::new("Confidential Client").unwrap();
        assert!(!format!("{canonical:?}").contains("customer-secret-project"));
        assert!(!format!("{label:?}").contains("Confidential Client"));
        let request = AddProject {
            id: ProjectId::new(),
            root: secret_path,
            display_name: Some("Confidential Client".to_string()),
            expected: Revision::ZERO,
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("customer-secret-project"));
        assert!(!debug.contains("Confidential Client"));
    }

    #[test]
    fn permission_errors_map_to_stable_code_without_path() {
        assert_eq!(
            map_path_error(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            ProjectError::PermissionDenied
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

        assert_eq!(canonical.status(), ProjectStatus::PermissionDenied);
        assert_eq!(
            CanonicalPath::resolve(&project_root),
            Err(ProjectError::PermissionDenied)
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
            Err(ProjectError::PathValidation)
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
