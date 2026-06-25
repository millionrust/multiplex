use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MOBILE_VAULT_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileVaultExport {
    pub schema_version: u16,
    pub export_id: String,
    pub created_at_millis: u128,
    pub updated_at_millis: u128,
    pub source_device_id: String,
    #[serde(default)]
    pub vaults: Vec<MobileVault>,
    #[serde(default)]
    pub hosts: Vec<MobileHost>,
    #[serde(default)]
    pub groups: Vec<MobileGroup>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub identities: Vec<MobileIdentityMetadata>,
    #[serde(default)]
    pub known_hosts: Vec<MobileKnownHost>,
    #[serde(default)]
    pub sync: MobileSyncMetadata,
    #[serde(default)]
    pub devices: Vec<MobileDeviceRecord>,
}

impl MobileVaultExport {
    pub fn empty(export_id: impl Into<String>, source_device_id: impl Into<String>) -> Self {
        Self {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: export_id.into(),
            created_at_millis: 0,
            updated_at_millis: 0,
            source_device_id: source_device_id.into(),
            vaults: Vec::new(),
            hosts: Vec::new(),
            groups: Vec::new(),
            tags: Vec::new(),
            identities: Vec::new(),
            known_hosts: Vec::new(),
            sync: MobileSyncMetadata::default(),
            devices: Vec::new(),
        }
    }

    pub fn from_desktop_portable_json(
        input: &str,
        export_id: impl Into<String>,
        source_device_id: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        let desktop: DesktopPortableBundle = serde_json::from_str(input)?;
        Ok(Self::from_desktop_portable_bundle(
            desktop,
            export_id,
            source_device_id,
        ))
    }

    fn from_desktop_portable_bundle(
        desktop: DesktopPortableBundle,
        export_id: impl Into<String>,
        source_device_id: impl Into<String>,
    ) -> Self {
        let mut groups = Vec::new();
        let mut tag_set = BTreeMap::<String, ()>::new();
        let hosts = desktop
            .profiles
            .into_iter()
            .map(|profile| {
                if !profile.group.trim().is_empty()
                    && !groups
                        .iter()
                        .any(|group: &MobileGroup| group.name == profile.group)
                {
                    groups.push(MobileGroup {
                        id: profile.group.clone(),
                        name: profile.group.clone(),
                    });
                }
                for tag in &profile.tags {
                    tag_set.insert(tag.clone(), ());
                }
                profile.into_mobile_host()
            })
            .collect();

        Self {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: export_id.into(),
            created_at_millis: desktop.exported_at_millis,
            updated_at_millis: desktop.exported_at_millis,
            source_device_id: source_device_id.into(),
            vaults: desktop
                .vaults
                .into_iter()
                .map(DesktopVault::into_mobile_vault)
                .collect(),
            hosts,
            groups,
            tags: tag_set.into_keys().collect(),
            identities: desktop
                .identities
                .into_iter()
                .map(DesktopIdentity::into_mobile_identity)
                .collect(),
            known_hosts: desktop
                .known_hosts
                .into_iter()
                .map(|(endpoint, public_key)| MobileKnownHost {
                    endpoint,
                    public_key,
                    algorithm: None,
                    fingerprint: None,
                })
                .collect(),
            sync: MobileSyncMetadata::default(),
            devices: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileVault {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileHost {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth: MobileAuthMetadata,
    #[serde(default)]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub start_in_files: bool,
    #[serde(default)]
    pub persistent_session: MobilePersistentSession,
    #[serde(default)]
    pub terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    pub color_tag: Option<String>,
    #[serde(default)]
    pub environment: Vec<MobileEnvironmentVariable>,
    #[serde(default)]
    pub known_host_endpoint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobilePersistentSession {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub detach_others: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileAuthMetadata {
    #[serde(default)]
    pub kind: MobileAuthKind,
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileAuthKind {
    #[default]
    Password,
    PrivateKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileIdentityMetadata {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileKnownHost {
    pub endpoint: String,
    pub public_key: String,
    #[serde(default)]
    pub algorithm: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGroup {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileSyncMetadata {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub previous_revision_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDeviceRecord {
    pub device_id: String,
    pub label: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub revoked_at_millis: Option<u128>,
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopPortableBundle {
    #[allow(dead_code)]
    version: u16,
    exported_at_millis: u128,
    #[serde(default)]
    vaults: Vec<DesktopVault>,
    #[serde(default)]
    profiles: Vec<DesktopHostProfile>,
    #[serde(default)]
    identities: Vec<DesktopIdentity>,
    #[serde(default)]
    known_hosts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopVault {
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: String,
}

impl DesktopVault {
    fn into_mobile_vault(self) -> MobileVault {
        MobileVault {
            id: self.id,
            label: self.label,
            description: self.description,
            kind: self.kind,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopHostProfile {
    id: String,
    label: String,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    group: String,
    #[serde(default)]
    tags: Vec<String>,
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
    username: String,
    #[serde(default)]
    auth_mode: MobileAuthKind,
    #[serde(default)]
    identity_id: Option<String>,
    #[serde(default)]
    jump_host_id: Option<String>,
    #[serde(default)]
    startup_directory: Option<String>,
    #[serde(default)]
    startup_command: Option<String>,
    #[serde(default)]
    start_in_files: bool,
    #[serde(default)]
    persistent_session: bool,
    #[serde(default)]
    persistent_session_name: Option<String>,
    #[serde(default)]
    persistent_session_detach_others: bool,
    #[serde(default)]
    terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    color_tag: Option<String>,
    #[serde(default)]
    environment: Vec<(String, String)>,
}

impl DesktopHostProfile {
    fn into_mobile_host(self) -> MobileHost {
        let known_host_endpoint = Some(format!("{}:{}", self.host, self.port));
        MobileHost {
            id: self.id,
            label: self.label,
            vault_id: self.vault_id,
            group: self.group,
            tags: self.tags,
            host: self.host,
            port: self.port,
            username: self.username,
            auth: MobileAuthMetadata {
                kind: self.auth_mode,
                identity_id: self.identity_id,
                secret_ref: None,
            },
            jump_host_id: self.jump_host_id,
            startup_directory: self.startup_directory,
            startup_command: self.startup_command,
            start_in_files: self.start_in_files,
            persistent_session: MobilePersistentSession {
                enabled: self.persistent_session,
                session_name: self.persistent_session_name,
                detach_others: self.persistent_session_detach_others,
            },
            terminal_scrollback_rows: self.terminal_scrollback_rows,
            color_tag: self.color_tag,
            environment: self
                .environment
                .into_iter()
                .map(|(name, value)| MobileEnvironmentVariable { name, value })
                .collect(),
            known_host_endpoint,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopIdentity {
    id: String,
    label: String,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    kind: String,
}

impl DesktopIdentity {
    fn into_mobile_identity(self) -> MobileIdentityMetadata {
        MobileIdentityMetadata {
            id: self.id,
            label: self.label,
            vault_id: self.vault_id,
            kind: self.kind,
            public_key: None,
            fingerprint: None,
            secret_ref: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_vault_schema_round_trips() {
        let export = MobileVaultExport {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: "export-1".to_string(),
            created_at_millis: 100,
            updated_at_millis: 101,
            source_device_id: "desktop-1".to_string(),
            vaults: vec![MobileVault {
                id: "vault-1".to_string(),
                label: "Ops".to_string(),
                description: String::new(),
                kind: "shared".to_string(),
            }],
            hosts: vec![MobileHost {
                id: "profile-1".to_string(),
                label: "Production".to_string(),
                vault_id: Some("vault-1".to_string()),
                group: "Ops".to_string(),
                tags: vec!["prod".to_string()],
                host: "prod.example.com".to_string(),
                port: 22,
                username: "deploy".to_string(),
                auth: MobileAuthMetadata {
                    kind: MobileAuthKind::PrivateKey,
                    identity_id: Some("identity-1".to_string()),
                    secret_ref: Some("keychain://identity-1".to_string()),
                },
                jump_host_id: None,
                startup_directory: Some("/srv/app".to_string()),
                startup_command: Some("uptime".to_string()),
                start_in_files: false,
                persistent_session: MobilePersistentSession {
                    enabled: true,
                    session_name: Some("tr-prod".to_string()),
                    detach_others: false,
                },
                terminal_scrollback_rows: Some(10_000),
                color_tag: Some("green".to_string()),
                environment: vec![MobileEnvironmentVariable {
                    name: "APP_ENV".to_string(),
                    value: "prod".to_string(),
                }],
                known_host_endpoint: Some("prod.example.com:22".to_string()),
            }],
            groups: vec![MobileGroup {
                id: "Ops".to_string(),
                name: "Ops".to_string(),
            }],
            tags: vec!["prod".to_string()],
            identities: vec![MobileIdentityMetadata {
                id: "identity-1".to_string(),
                label: "Deploy key".to_string(),
                vault_id: Some("vault-1".to_string()),
                kind: "ed25519".to_string(),
                public_key: None,
                fingerprint: None,
                secret_ref: None,
            }],
            known_hosts: vec![MobileKnownHost {
                endpoint: "prod.example.com:22".to_string(),
                public_key: "ssh-ed25519 AAAA".to_string(),
                algorithm: Some("ssh-ed25519".to_string()),
                fingerprint: None,
            }],
            sync: MobileSyncMetadata {
                revision: 1,
                previous_revision_id: None,
            },
            devices: vec![MobileDeviceRecord {
                device_id: "ios-1".to_string(),
                label: "Jacob iPhone".to_string(),
                platform: Some("ios".to_string()),
                public_key: None,
                revoked_at_millis: None,
            }],
        };

        let json = serde_json::to_string_pretty(&export).expect("serialize mobile vault");
        let parsed: MobileVaultExport =
            serde_json::from_str(&json).expect("deserialize mobile vault");
        assert_eq!(parsed, export);
    }

    #[test]
    fn mobile_schema_defaults_missing_collections() {
        let parsed: MobileVaultExport = serde_json::from_str(
            r#"{
              "schema_version": 1,
              "export_id": "export-1",
              "created_at_millis": 1,
              "updated_at_millis": 1,
              "source_device_id": "desktop-1"
            }"#,
        )
        .expect("deserialize minimal mobile vault");

        assert!(parsed.hosts.is_empty());
        assert!(parsed.known_hosts.is_empty());
        assert_eq!(parsed.sync, MobileSyncMetadata::default());
    }

    #[test]
    fn desktop_portable_bundle_converts_to_mobile_schema() {
        let mobile = MobileVaultExport::from_desktop_portable_json(
            r#"{
              "version": 1,
              "exported_at_millis": 1719356789123,
              "vaults": [{
                "id": "vault-ops",
                "label": "Ops",
                "description": "Operations vault",
                "kind": "shared"
              }],
              "profiles": [{
                "id": "profile-1",
                "label": "Production",
                "vault_id": "vault-ops",
                "group": "Ops",
                "tags": ["prod", "ssh"],
                "host": "prod.example.com",
                "port": 2222,
                "username": "deploy",
                "auth_mode": "private_key",
                "identity_id": "identity-1",
                "startup_directory": "/srv/app",
                "startup_command": "uptime",
                "persistent_session": true,
                "persistent_session_name": "tr-prod",
                "persistent_session_detach_others": true,
                "environment": [["APP_ENV", "prod"]]
              }],
              "identities": [{
                "id": "identity-1",
                "label": "Deploy key",
                "vault_id": "vault-ops",
                "key_path": "/Users/jacob/.ssh/id_ed25519",
                "kind": "ed25519"
              }],
              "known_hosts": {
                "prod.example.com:2222": "ssh-ed25519 AAAA"
              }
            }"#,
            "export-1",
            "desktop-1",
        )
        .expect("convert desktop bundle");

        assert_eq!(mobile.schema_version, MOBILE_VAULT_SCHEMA_VERSION);
        assert_eq!(mobile.created_at_millis, 1719356789123);
        assert_eq!(mobile.vaults[0].id, "vault-ops");
        assert_eq!(mobile.vaults[0].kind, "shared");
        assert_eq!(mobile.groups[0].name, "Ops");
        assert_eq!(mobile.tags, vec!["prod".to_string(), "ssh".to_string()]);
        assert_eq!(mobile.known_hosts[0].endpoint, "prod.example.com:2222");

        let host = &mobile.hosts[0];
        assert_eq!(host.auth.kind, MobileAuthKind::PrivateKey);
        assert_eq!(host.auth.identity_id.as_deref(), Some("identity-1"));
        assert_eq!(host.auth.secret_ref, None);
        assert!(host.persistent_session.enabled);
        assert_eq!(
            host.persistent_session.session_name.as_deref(),
            Some("tr-prod")
        );
        assert!(host.persistent_session.detach_others);
        assert_eq!(host.environment[0].name, "APP_ENV");

        let identity = &mobile.identities[0];
        assert_eq!(identity.kind, "ed25519");
        assert_eq!(identity.secret_ref, None);
    }
}
