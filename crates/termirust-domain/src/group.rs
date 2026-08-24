use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{GroupId, PositionKey, ProjectId, Revision};

pub const MAX_GROUPS_PER_PROJECT: usize = 256;
pub const MAX_GROUP_NAME_SCALARS: usize = 256;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GroupName(String);

impl GroupName {
    pub fn new(value: &str) -> Result<Self, GroupError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(GroupError::EmptyName);
        }
        if value.contains('\0') {
            return Err(GroupError::NameContainsNul);
        }
        if value.chars().count() > MAX_GROUP_NAME_SCALARS {
            return Err(GroupError::NameTooLong);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn comparison_key(&self) -> String {
        self.0.to_lowercase()
    }
}

impl fmt::Debug for GroupName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GroupName(<redacted>)")
    }
}

impl fmt::Display for GroupName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Group {
    pub id: GroupId,
    pub project_id: ProjectId,
    pub name: GroupName,
    pub position: PositionKey,
    pub collapsed: bool,
    pub revision: Revision,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "group_id", rename_all = "snake_case")]
pub enum GroupDestination {
    #[default]
    ProjectRoot,
    Group(GroupId),
}

impl GroupDestination {
    pub const fn group_id(self) -> Option<GroupId> {
        match self {
            Self::ProjectRoot => None,
            Self::Group(id) => Some(id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupInverseCommand {
    RemoveCreated {
        group_id: GroupId,
    },
    Rename {
        group_id: GroupId,
        name: GroupName,
    },
    SetCollapsed {
        group_id: GroupId,
        collapsed: bool,
    },
    MoveBefore {
        group_id: GroupId,
        before: Option<GroupId>,
    },
    RestoreRemoved {
        group: Group,
        moved_sessions_to: GroupDestination,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupMutation<T> {
    pub value: T,
    pub inverse: GroupInverseCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupError {
    EmptyName,
    NameContainsNul,
    NameTooLong,
    DuplicateName,
    NotFound,
    ProjectNotFound,
    DestinationNotFound,
    DestinationIsSource,
    NonEmptyDestinationRequired,
    WrongProject,
    ResourceLimit {
        limit: usize,
    },
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    RevisionOverflow,
    PositionOverflow,
    Store {
        code: &'static str,
    },
}

impl fmt::Display for GroupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("group name is empty"),
            Self::NameContainsNul => formatter.write_str("group name contains NUL"),
            Self::NameTooLong => formatter.write_str("group name exceeds 256 characters"),
            Self::DuplicateName => formatter.write_str("a group with this name already exists"),
            Self::NotFound => formatter.write_str("group no longer exists"),
            Self::ProjectNotFound => formatter.write_str("project no longer exists"),
            Self::DestinationNotFound => formatter.write_str("destination group no longer exists"),
            Self::DestinationIsSource => formatter.write_str("a group cannot move into itself"),
            Self::NonEmptyDestinationRequired => {
                formatter.write_str("choose where this group's sessions should move")
            }
            Self::WrongProject => formatter.write_str("group belongs to another project"),
            Self::ResourceLimit { limit } => write!(formatter, "group limit of {limit} reached"),
            Self::StaleRevision { .. } => {
                formatter.write_str("group organization changed; reload required")
            }
            Self::RevisionOverflow => formatter.write_str("group revision exhausted"),
            Self::PositionOverflow => formatter.write_str("group position exhausted"),
            Self::Store { code } => write!(formatter, "group store error ({code})"),
        }
    }
}

impl std::error::Error for GroupError {}

pub fn validate_group_set(groups: &[Group], projects: &[ProjectId]) -> Result<(), GroupError> {
    use std::collections::{HashMap, HashSet};

    let project_ids = projects.iter().copied().collect::<HashSet<_>>();
    let mut ids = HashSet::with_capacity(groups.len());
    let mut counts = HashMap::<ProjectId, usize>::new();
    let mut names = HashSet::<(ProjectId, String)>::new();
    for group in groups {
        if !project_ids.contains(&group.project_id) {
            return Err(GroupError::ProjectNotFound);
        }
        if !ids.insert(group.id) {
            return Err(GroupError::Store {
                code: "duplicate-group-id",
            });
        }
        if !names.insert((group.project_id, group.name.comparison_key())) {
            return Err(GroupError::DuplicateName);
        }
        let count = counts.entry(group.project_id).or_default();
        *count += 1;
        if *count > MAX_GROUPS_PER_PROJECT {
            return Err(GroupError::ResourceLimit {
                limit: MAX_GROUPS_PER_PROJECT,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(project_id: ProjectId, name: &str, index: usize) -> Group {
        Group {
            id: GroupId::new(),
            project_id,
            name: GroupName::new(name).unwrap(),
            position: PositionKey::rebalanced(index).unwrap(),
            collapsed: false,
            revision: Revision::ZERO,
        }
    }

    #[test]
    fn group_names_are_trimmed_bounded_and_redacted() {
        let name = GroupName::new("  Review  ").unwrap();
        assert_eq!(name.as_str(), "Review");
        assert_eq!(format!("{name:?}"), "GroupName(<redacted>)");
        assert_eq!(GroupName::new(" "), Err(GroupError::EmptyName));
        assert_eq!(
            GroupName::new(&"x".repeat(257)),
            Err(GroupError::NameTooLong)
        );
    }

    #[test]
    fn group_names_are_unique_case_insensitively_per_project() {
        let project_id = ProjectId::new();
        let groups = vec![
            group(project_id, "Review", 0),
            group(project_id, "review", 1),
        ];
        assert_eq!(
            validate_group_set(&groups, &[project_id]),
            Err(GroupError::DuplicateName)
        );
    }

    #[test]
    fn destination_exposes_only_organization_identity() {
        let id = GroupId::new();
        assert_eq!(GroupDestination::ProjectRoot.group_id(), None);
        assert_eq!(GroupDestination::Group(id).group_id(), Some(id));
    }
}
