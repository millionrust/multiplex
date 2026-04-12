use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn default_ssh_port() -> u16 {
    22
}

fn default_local_forward_host() -> String {
    "127.0.0.1".to_string()
}

pub const DEFAULT_VAULT_ID: &str = "vault-personal";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    #[default]
    Password,
    PrivateKey,
}

impl AuthMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::PrivateKey => "Private Key",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    #[default]
    User,
    SshConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    #[default]
    User,
    Imported,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultKind {
    #[default]
    Personal,
    Shared,
}

impl VaultKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Personal => "Personal",
            Self::Shared => "Shared",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultMemberRole {
    Owner,
    #[default]
    Editor,
    Viewer,
}

impl VaultMemberRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Editor => "Editor",
            Self::Viewer => "Viewer",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    #[default]
    Ssh,
    LocalShell,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalShellConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl LocalShellConfig {
    pub fn display_name(&self) -> String {
        if let Some(name) = std::path::Path::new(&self.program)
            .file_name()
            .and_then(|name| name.to_str())
        {
            name.to_string()
        } else {
            self.program.clone()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostProfile {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub group: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    #[serde(default)]
    pub key_path: String,
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub local_forward: Option<LocalPortForward>,
    #[serde(default)]
    pub password_credential_id: Option<String>,
    #[serde(default)]
    pub source: ProfileSource,
}

impl HostProfile {
    pub fn display_name(&self) -> String {
        if !self.label.trim().is_empty() {
            self.label.trim().to_string()
        } else {
            format!("{}@{}", self.username.trim(), self.host.trim())
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host.trim(), self.port)
    }

    pub fn effective_vault_id(&self) -> &str {
        self.vault_id.as_deref().unwrap_or(DEFAULT_VAULT_ID)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedIdentity {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    pub key_path: String,
    pub kind: String,
    #[serde(default)]
    pub source: IdentitySource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPortForward {
    #[serde(default = "default_local_forward_host")]
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

impl LocalPortForward {
    pub fn display_name(&self) -> String {
        format!(
            "{}:{} -> {}:{}",
            self.local_host, self.local_port, self.remote_host, self.remote_port
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedSnippet {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub group: String,
    pub command: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedVault {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: VaultKind,
    #[serde(default)]
    pub members: Vec<SavedVaultMember>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedVaultMember {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub role: VaultMemberRole,
}

impl SavedVault {
    pub fn personal() -> Self {
        Self {
            id: DEFAULT_VAULT_ID.to_string(),
            label: "Personal".to_string(),
            description: "Local vault for private hosts, snippets, and identities.".to_string(),
            kind: VaultKind::Personal,
            members: vec![SavedVaultMember::you()],
        }
    }

    pub fn display_name(&self) -> String {
        if self.label.trim().is_empty() {
            self.kind.label().to_string()
        } else {
            self.label.trim().to_string()
        }
    }

    pub fn vault_id() -> String {
        format!("vault-{}", now_millis())
    }

    pub fn is_personal(&self) -> bool {
        self.id == DEFAULT_VAULT_ID || self.kind == VaultKind::Personal
    }

    pub fn ensure_members(&mut self) {
        if self.is_personal() {
            self.kind = VaultKind::Personal;
            if self.members.is_empty() {
                self.members.push(SavedVaultMember::you());
            }
        } else if self.members.is_empty()
            || !self
                .members
                .iter()
                .any(|member| member.role == VaultMemberRole::Owner)
        {
            self.members.push(SavedVaultMember::owner_you());
        }

        self.members.sort_by(|left, right| {
            role_sort_key(left.role)
                .cmp(&role_sort_key(right.role))
                .then_with(|| {
                    left.display_name()
                        .to_ascii_lowercase()
                        .cmp(&right.display_name().to_ascii_lowercase())
                })
        });
    }

    pub fn upsert_member(&mut self, member: SavedVaultMember) {
        let mut member = member;
        if member.id.trim().is_empty() {
            member.id = SavedVaultMember::member_id();
        }

        if let Some(existing) = self.members.iter_mut().find(|item| item.id == member.id) {
            *existing = member;
        } else if let Some(existing) = self
            .members
            .iter_mut()
            .find(|item| item.email.eq_ignore_ascii_case(&member.email))
        {
            *existing = member;
        } else {
            self.members.push(member);
        }

        self.ensure_members();
    }

    pub fn remove_member(&mut self, member_id: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|member| member.id != member_id);
        self.ensure_members();
        before != self.members.len()
    }
}

impl SavedVaultMember {
    pub fn member_id() -> String {
        format!("member-{}", now_millis())
    }

    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else {
            self.email.trim().to_string()
        }
    }

    pub fn you() -> Self {
        Self {
            id: "member-you".to_string(),
            name: current_username(),
            email: "local@device".to_string(),
            role: VaultMemberRole::Owner,
        }
    }

    pub fn owner_you() -> Self {
        Self {
            role: VaultMemberRole::Owner,
            ..Self::you()
        }
    }
}

impl SavedSnippet {
    pub fn display_name(&self) -> String {
        if !self.label.trim().is_empty() {
            self.label.trim().to_string()
        } else {
            self.command.trim().to_string()
        }
    }

    pub fn snippet_id() -> String {
        format!("snippet-{}", now_millis())
    }

    pub fn effective_vault_id(&self) -> &str {
        self.vault_id.as_deref().unwrap_or(DEFAULT_VAULT_ID)
    }
}

impl SavedIdentity {
    pub fn effective_vault_id(&self) -> &str {
        self.vault_id.as_deref().unwrap_or(DEFAULT_VAULT_ID)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedState {
    #[serde(default)]
    pub vaults: Vec<SavedVault>,
    #[serde(default)]
    pub profiles: Vec<HostProfile>,
    #[serde(default)]
    pub identities: Vec<SavedIdentity>,
    #[serde(default)]
    pub snippets: Vec<SavedSnippet>,
    #[serde(default)]
    pub selected_profile_id: Option<String>,
    #[serde(default)]
    pub session_logs: Vec<SessionLogEntry>,
    #[serde(default)]
    pub restored_workspaces: Vec<SavedWorkspace>,
    #[serde(default)]
    pub active_workspace_index: Option<usize>,
}

impl SavedState {
    pub fn ensure_vaults(&mut self) {
        if !self.vaults.iter().any(|vault| vault.id == DEFAULT_VAULT_ID) {
            self.vaults.push(SavedVault::personal());
        }

        if let Some(personal) = self
            .vaults
            .iter_mut()
            .find(|vault| vault.id == DEFAULT_VAULT_ID)
        {
            personal.kind = VaultKind::Personal;
            if personal.label.trim().is_empty() {
                personal.label = "Personal".to_string();
            }
        }

        for vault in &mut self.vaults {
            vault.ensure_members();
        }

        self.vaults.sort_by(|left, right| {
            left.is_personal()
                .cmp(&right.is_personal())
                .reverse()
                .then_with(|| {
                    left.display_name()
                        .to_ascii_lowercase()
                        .cmp(&right.display_name().to_ascii_lowercase())
                })
        });

        let valid_vaults = self
            .vaults
            .iter()
            .map(|vault| vault.id.as_str())
            .collect::<Vec<_>>();
        for profile in &mut self.profiles {
            if !profile
                .vault_id
                .as_deref()
                .is_some_and(|vault_id| valid_vaults.contains(&vault_id))
            {
                profile.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
        for identity in &mut self.identities {
            if !identity
                .vault_id
                .as_deref()
                .is_some_and(|vault_id| valid_vaults.contains(&vault_id))
            {
                identity.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
        for snippet in &mut self.snippets {
            if !snippet
                .vault_id
                .as_deref()
                .is_some_and(|vault_id| valid_vaults.contains(&vault_id))
            {
                snippet.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
    }

    pub fn upsert_vault(&mut self, mut vault: SavedVault) {
        if vault.id.trim().is_empty() {
            vault.id = SavedVault::vault_id();
        }
        if vault.is_personal() {
            vault.id = DEFAULT_VAULT_ID.to_string();
            vault.kind = VaultKind::Personal;
        }
        vault.ensure_members();

        if let Some(existing) = self.vaults.iter_mut().find(|item| item.id == vault.id) {
            *existing = vault;
        } else {
            self.vaults.push(vault);
        }

        self.ensure_vaults();
    }

    pub fn remove_vault(&mut self, vault_id: &str) -> bool {
        if vault_id == DEFAULT_VAULT_ID {
            return false;
        }

        let removed = self.vaults.iter().any(|vault| vault.id == vault_id);
        if !removed {
            return false;
        }

        self.vaults.retain(|vault| vault.id != vault_id);
        for profile in &mut self.profiles {
            if profile.vault_id.as_deref() == Some(vault_id) {
                profile.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
        for identity in &mut self.identities {
            if identity.vault_id.as_deref() == Some(vault_id) {
                identity.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
        for snippet in &mut self.snippets {
            if snippet.vault_id.as_deref() == Some(vault_id) {
                snippet.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
        }
        self.ensure_vaults();
        true
    }

    pub fn upsert_profile(&mut self, profile: HostProfile) {
        let mut profile = profile;
        if profile.vault_id.is_none() {
            profile.vault_id = Some(DEFAULT_VAULT_ID.to_string());
        }

        if let Some(existing) = self.profiles.iter_mut().find(|item| item.id == profile.id) {
            *existing = profile.clone();
        } else {
            self.profiles.push(profile.clone());
        }

        self.ensure_vaults();
        self.profiles
            .sort_by_key(|profile| profile.display_name().to_ascii_lowercase());
        self.selected_profile_id = Some(profile.id);
    }

    pub fn remove_profile(&mut self, profile_id: &str) {
        self.profiles.retain(|profile| profile.id != profile_id);
        if self.selected_profile_id.as_deref() == Some(profile_id) {
            self.selected_profile_id = self.profiles.first().map(|profile| profile.id.clone());
        }
    }

    pub fn merge_imported_profiles(&mut self, imported_profiles: Vec<HostProfile>) {
        for mut imported in imported_profiles {
            if imported.vault_id.is_none() {
                imported.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
            if let Some(existing) = self.profiles.iter_mut().find(|item| item.id == imported.id) {
                if existing.source == ProfileSource::User {
                    continue;
                }

                *existing = imported;
            } else {
                self.profiles.push(imported);
            }
        }

        self.ensure_vaults();
        self.profiles
            .sort_by_key(|profile| profile.display_name().to_ascii_lowercase());
    }

    pub fn upsert_identity(&mut self, identity: SavedIdentity) {
        let mut identity = identity;
        if identity.vault_id.is_none() {
            identity.vault_id = Some(DEFAULT_VAULT_ID.to_string());
        }

        if let Some(existing) = self
            .identities
            .iter_mut()
            .find(|item| item.id == identity.id)
        {
            *existing = identity;
        } else {
            self.identities.push(identity);
        }

        self.ensure_vaults();
        self.identities
            .sort_by_key(|identity| identity.label.to_ascii_lowercase());
    }

    pub fn merge_imported_identities(&mut self, imported_identities: Vec<ImportedIdentity>) {
        for imported in imported_identities {
            let mut identity = imported.into_saved();
            if identity.vault_id.is_none() {
                identity.vault_id = Some(DEFAULT_VAULT_ID.to_string());
            }
            if let Some(existing) = self
                .identities
                .iter_mut()
                .find(|item| item.id == identity.id)
            {
                if existing.source == IdentitySource::User {
                    continue;
                }
                *existing = identity;
            } else {
                self.identities.push(identity);
            }
        }

        self.ensure_vaults();
        self.identities
            .sort_by_key(|identity| identity.label.to_ascii_lowercase());
    }

    pub fn upsert_snippet(&mut self, snippet: SavedSnippet) {
        let mut snippet = snippet;
        if snippet.vault_id.is_none() {
            snippet.vault_id = Some(DEFAULT_VAULT_ID.to_string());
        }

        if let Some(existing) = self.snippets.iter_mut().find(|item| item.id == snippet.id) {
            *existing = snippet;
        } else {
            self.snippets.push(snippet);
        }

        self.ensure_vaults();
        self.snippets
            .sort_by_key(|snippet| snippet.display_name().to_ascii_lowercase());
    }

    pub fn remove_snippet(&mut self, snippet_id: &str) {
        self.snippets.retain(|snippet| snippet.id != snippet_id);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedIdentity {
    pub label: String,
    pub path: String,
    pub kind: String,
}

impl ImportedIdentity {
    pub fn into_saved(self) -> SavedIdentity {
        SavedIdentity {
            id: identity_id_for_path(&self.path),
            label: self.label,
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            key_path: self.path,
            kind: self.kind,
            source: IdentitySource::Imported,
        }
    }
}

pub fn identity_id_for_path(path: &str) -> String {
    let hash = path.bytes().fold(1469598103934665603u64, |acc, byte| {
        acc.wrapping_mul(1099511628211)
            .wrapping_add(u64::from(byte))
    });
    format!("identity-{hash:x}")
}

#[derive(Clone, Debug, Default)]
pub struct DraftProfile {
    pub label: String,
    pub vault_id: Option<String>,
    pub group: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub key_path: String,
    pub identity_id: Option<String>,
    pub jump_host_id: Option<String>,
    pub forward_local_port: String,
    pub forward_remote_host: String,
    pub forward_remote_port: String,
    pub key_passphrase: String,
    pub password_credential_id: Option<String>,
    pub auth_mode: AuthMode,
}

impl DraftProfile {
    pub fn from_profile(profile: &HostProfile) -> Self {
        Self {
            label: profile.label.clone(),
            vault_id: profile.vault_id.clone(),
            group: profile.group.clone(),
            host: profile.host.clone(),
            port: profile.port.to_string(),
            username: profile.username.clone(),
            password: String::new(),
            key_path: profile.key_path.clone(),
            identity_id: profile.identity_id.clone(),
            jump_host_id: profile.jump_host_id.clone(),
            forward_local_port: profile
                .local_forward
                .as_ref()
                .map(|forward| forward.local_port.to_string())
                .unwrap_or_default(),
            forward_remote_host: profile
                .local_forward
                .as_ref()
                .map(|forward| forward.remote_host.clone())
                .unwrap_or_default(),
            forward_remote_port: profile
                .local_forward
                .as_ref()
                .map(|forward| forward.remote_port.to_string())
                .unwrap_or_default(),
            key_passphrase: String::new(),
            password_credential_id: profile.password_credential_id.clone(),
            auth_mode: profile.auth_mode,
        }
    }

    pub fn profile_id() -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        format!("profile-{millis}")
    }

    pub fn display_name(&self) -> String {
        if !self.label.trim().is_empty() {
            self.label.trim().to_string()
        } else {
            format!("{}@{}", self.username.trim(), self.host.trim())
        }
    }

    fn parse_port(&self) -> Result<u16> {
        let port = self.port.trim();
        if port.is_empty() {
            return Ok(default_ssh_port());
        }

        port.parse::<u16>()
            .with_context(|| format!("Invalid SSH port '{port}'"))
    }

    fn parse_local_forward(&self) -> Result<Option<LocalPortForward>> {
        let local_port = self.forward_local_port.trim();
        let remote_host = self.forward_remote_host.trim();
        let remote_port = self.forward_remote_port.trim();

        if local_port.is_empty() && remote_host.is_empty() && remote_port.is_empty() {
            return Ok(None);
        }

        if local_port.is_empty() || remote_host.is_empty() || remote_port.is_empty() {
            bail!("Local forwarding requires local port, remote host, and remote port");
        }

        Ok(Some(LocalPortForward {
            local_host: default_local_forward_host(),
            local_port: local_port
                .parse::<u16>()
                .with_context(|| format!("Invalid local forward port '{local_port}'"))?,
            remote_host: remote_host.to_string(),
            remote_port: remote_port
                .parse::<u16>()
                .with_context(|| format!("Invalid remote forward port '{remote_port}'"))?,
        }))
    }

    pub fn to_profile(&self, id: String) -> Result<HostProfile> {
        let host = self.host.trim();
        let username = self.username.trim();
        let key_path = self.key_path.trim();

        if host.is_empty() {
            bail!("Host is required");
        }
        if username.is_empty() {
            bail!("Username is required");
        }
        if self.auth_mode == AuthMode::PrivateKey && key_path.is_empty() {
            bail!("A private key file is required for key authentication");
        }

        Ok(HostProfile {
            id,
            label: self.label.trim().to_string(),
            vault_id: self
                .vault_id
                .clone()
                .or_else(|| Some(DEFAULT_VAULT_ID.to_string())),
            group: self.group.trim().to_string(),
            host: host.to_string(),
            port: self.parse_port()?,
            username: username.to_string(),
            auth_mode: self.auth_mode,
            key_path: key_path.to_string(),
            identity_id: if self.auth_mode == AuthMode::PrivateKey {
                self.identity_id.clone()
            } else {
                None
            },
            jump_host_id: self.jump_host_id.clone(),
            local_forward: self.parse_local_forward()?,
            password_credential_id: if self.auth_mode == AuthMode::Password {
                self.password_credential_id.clone()
            } else {
                None
            },
            source: ProfileSource::User,
        })
    }

    pub fn to_connect_request(&self, session_id: u64) -> Result<ConnectRequest> {
        let profile = self.to_profile(Self::profile_id())?;

        let auth = match self.auth_mode {
            AuthMode::Password => {
                let password = self.password.trim().to_string();
                if !password.is_empty() {
                    AuthConfig::Password { password }
                } else if let Some(credential_id) = self.password_credential_id.clone() {
                    AuthConfig::PasswordRef { credential_id }
                } else {
                    bail!("Password is required for password authentication");
                }
            }
            AuthMode::PrivateKey => AuthConfig::PrivateKey {
                key_path: profile.key_path.clone(),
                passphrase: non_empty(self.key_passphrase.trim()),
            },
        };

        Ok(ConnectRequest {
            session_id,
            title: self.display_name(),
            kind: ConnectionKind::Ssh,
            host: profile.host,
            port: profile.port,
            username: profile.username,
            auth: Some(auth),
            jump_host: None,
            local_forward: profile.local_forward,
            local_shell: None,
        })
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local".to_string())
}

fn role_sort_key(role: VaultMemberRole) -> u8 {
    match role {
        VaultMemberRole::Owner => 0,
        VaultMemberRole::Editor => 1,
        VaultMemberRole::Viewer => 2,
    }
}

fn default_local_shell_config() -> LocalShellConfig {
    #[cfg(target_os = "windows")]
    {
        LocalShellConfig {
            program: std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string()),
            args: Vec::new(),
            cwd: std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        LocalShellConfig {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
            args: Vec::new(),
            cwd: std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AuthConfig {
    Password {
        password: String,
    },
    PasswordRef {
        credential_id: String,
    },
    PrivateKey {
        key_path: String,
        passphrase: Option<String>,
    },
}

impl AuthConfig {
    pub fn to_restorable(&self) -> Option<RestorableAuth> {
        match self {
            Self::Password { .. } => None,
            Self::PasswordRef { credential_id } => Some(RestorableAuth::PasswordKeychain {
                credential_id: credential_id.clone(),
            }),
            Self::PrivateKey { key_path, .. } => Some(RestorableAuth::PrivateKey {
                key_path: key_path.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectRequest {
    pub session_id: u64,
    pub title: String,
    #[allow(dead_code)]
    pub kind: ConnectionKind,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Option<AuthConfig>,
    pub jump_host: Option<JumpHostConnection>,
    pub local_forward: Option<LocalPortForward>,
    pub local_shell: Option<LocalShellConfig>,
}

impl ConnectRequest {
    pub fn address(&self) -> String {
        match self.kind {
            ConnectionKind::Ssh => format!("{}:{}", self.host, self.port),
            ConnectionKind::LocalShell => "local shell".to_string(),
        }
    }

    pub fn endpoint_label(&self) -> String {
        match self.kind {
            ConnectionKind::Ssh => self.address(),
            ConnectionKind::LocalShell => self
                .local_shell
                .as_ref()
                .map(LocalShellConfig::display_name)
                .unwrap_or_else(|| "Local Shell".to_string()),
        }
    }

    pub fn is_local_shell(&self) -> bool {
        self.kind == ConnectionKind::LocalShell
    }

    pub fn known_host_key(&self) -> String {
        self.address()
    }

    pub fn to_restorable(&self) -> Option<RestorableConnection> {
        match self.kind {
            ConnectionKind::Ssh => Some(RestorableConnection {
                title: self.title.clone(),
                kind: self.kind,
                host: self.host.clone(),
                port: self.port,
                username: self.username.clone(),
                auth: Some(self.auth.as_ref()?.to_restorable()?),
                jump_host: self
                    .jump_host
                    .as_ref()
                    .and_then(JumpHostConnection::to_restorable),
                local_forward: self.local_forward.clone(),
                local_shell: None,
            }),
            ConnectionKind::LocalShell => Some(RestorableConnection {
                title: self.title.clone(),
                kind: self.kind,
                host: self.host.clone(),
                port: self.port,
                username: self.username.clone(),
                auth: None,
                jump_host: None,
                local_forward: None,
                local_shell: self.local_shell.clone(),
            }),
        }
    }

    pub fn local_shell(session_id: u64) -> Self {
        let shell = default_local_shell_config();
        Self {
            session_id,
            title: "Local Terminal".to_string(),
            kind: ConnectionKind::LocalShell,
            host: "local".to_string(),
            port: 0,
            username: current_username(),
            auth: None,
            jump_host: None,
            local_forward: None,
            local_shell: Some(shell),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JumpHostConnection {
    pub title: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthConfig,
    pub jump_host: Option<Box<JumpHostConnection>>,
}

impl JumpHostConnection {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn known_host_key(&self) -> String {
        self.address()
    }

    pub fn to_restorable(&self) -> Option<RestorableJumpHostConnection> {
        Some(RestorableJumpHostConnection {
            title: self.title.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth: self.auth.to_restorable()?,
            jump_host: self
                .jump_host
                .as_deref()
                .and_then(JumpHostConnection::to_restorable)
                .map(Box::new),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorableAuth {
    PasswordKeychain { credential_id: String },
    PrivateKey { key_path: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestorableConnection {
    pub title: String,
    #[serde(default)]
    pub kind: ConnectionKind,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: Option<RestorableAuth>,
    #[serde(default)]
    pub jump_host: Option<RestorableJumpHostConnection>,
    #[serde(default)]
    pub local_forward: Option<LocalPortForward>,
    #[serde(default)]
    pub local_shell: Option<LocalShellConfig>,
}

impl RestorableConnection {
    pub fn to_connect_request(&self, session_id: u64) -> ConnectRequest {
        match self.kind {
            ConnectionKind::Ssh => {
                let auth = match self.auth.as_ref().expect("ssh connections require auth") {
                    RestorableAuth::PasswordKeychain { credential_id } => AuthConfig::PasswordRef {
                        credential_id: credential_id.clone(),
                    },
                    RestorableAuth::PrivateKey { key_path } => AuthConfig::PrivateKey {
                        key_path: key_path.clone(),
                        passphrase: None,
                    },
                };

                Some(ConnectRequest {
                    session_id,
                    title: self.title.clone(),
                    kind: self.kind,
                    host: self.host.clone(),
                    port: self.port,
                    username: self.username.clone(),
                    auth: Some(auth),
                    jump_host: self
                        .jump_host
                        .as_ref()
                        .map(RestorableJumpHostConnection::to_jump_host_connection),
                    local_forward: self.local_forward.clone(),
                    local_shell: None,
                })
            }
            ConnectionKind::LocalShell => Some(ConnectRequest {
                session_id,
                title: self.title.clone(),
                kind: self.kind,
                host: self.host.clone(),
                port: self.port,
                username: self.username.clone(),
                auth: None,
                jump_host: None,
                local_forward: None,
                local_shell: self
                    .local_shell
                    .clone()
                    .or_else(|| Some(default_local_shell_config())),
            }),
        }
        .expect("restorable connection should be valid")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestorableJumpHostConnection {
    pub title: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: RestorableAuth,
    #[serde(default)]
    pub jump_host: Option<Box<RestorableJumpHostConnection>>,
}

impl RestorableJumpHostConnection {
    pub fn to_jump_host_connection(&self) -> JumpHostConnection {
        let auth = match &self.auth {
            RestorableAuth::PasswordKeychain { credential_id } => AuthConfig::PasswordRef {
                credential_id: credential_id.clone(),
            },
            RestorableAuth::PrivateKey { key_path } => AuthConfig::PrivateKey {
                key_path: key_path.clone(),
                passphrase: None,
            },
        };

        JumpHostConnection {
            title: self.title.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth,
            jump_host: self
                .jump_host
                .as_deref()
                .map(RestorableJumpHostConnection::to_jump_host_connection)
                .map(Box::new),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub title: String,
    #[serde(default)]
    pub split_axis: SplitAxis,
    #[serde(default)]
    pub active_pane_index: usize,
    #[serde(default)]
    pub panes: Vec<RestorableConnection>,
}

impl SavedWorkspace {
    pub fn normalize(&mut self) {
        if self.panes.is_empty() {
            self.active_pane_index = 0;
        } else {
            self.active_pane_index = self.active_pane_index.min(self.panes.len() - 1);
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLogStatus {
    #[default]
    Connecting,
    Connected,
    Disconnected,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub title: String,
    pub status: SessionLogStatus,
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub error_message: Option<String>,
}

impl SessionLogEntry {
    pub fn new(request: &ConnectRequest) -> Self {
        Self {
            id: format!("log-{}", now_millis()),
            host: request.host.clone(),
            port: request.port,
            username: request.username.clone(),
            title: request.title.clone(),
            status: SessionLogStatus::Connecting,
            started_at: now_millis(),
            ended_at: None,
            error_message: None,
        }
    }

    pub fn mark_connected(&mut self) {
        self.status = SessionLogStatus::Connected;
    }

    pub fn mark_disconnected(&mut self) {
        self.status = SessionLogStatus::Disconnected;
        self.ended_at = Some(now_millis());
    }

    pub fn mark_error(&mut self, message: &str) {
        self.status = SessionLogStatus::Error;
        self.error_message = Some(message.to_string());
        self.ended_at = Some(now_millis());
    }

    pub fn duration_display(&self) -> String {
        let end = self.ended_at.unwrap_or_else(now_millis);
        let elapsed = Duration::from_millis(end.saturating_sub(self.started_at));
        let secs = elapsed.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    pub fn started_display(&self) -> String {
        format_timestamp(self.started_at)
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn format_timestamp(millis: u64) -> String {
    let duration = Duration::from_millis(millis);
    let secs = duration.as_secs();
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

#[derive(Clone, Debug)]
pub struct QuickConnect {
    pub username: String,
    pub host: String,
    pub port: u16,
}

impl QuickConnect {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let input = input
            .strip_prefix("ssh ")
            .or_else(|| input.strip_prefix("ssh://"))
            .unwrap_or(input)
            .trim();

        if input.is_empty() {
            return None;
        }

        let (user_part, host_port) = if input.contains('@') {
            let mut parts = input.splitn(2, '@');
            let user = parts.next()?.trim();
            let rest = parts.next()?.trim();
            if user.is_empty() || rest.is_empty() {
                return None;
            }
            (user.to_string(), rest.to_string())
        } else {
            return None;
        };

        let (host, port) = if host_port.contains(':') {
            let mut parts = host_port.rsplitn(2, ':');
            let port_str = parts.next()?;
            let host = parts.next()?.trim();
            let port = port_str.parse::<u16>().ok()?;
            (host.to_string(), port)
        } else {
            (host_port, 22)
        };

        if host.is_empty() {
            return None;
        }

        Some(Self {
            username: user_part,
            host,
            port,
        })
    }

    pub fn display_name(&self) -> String {
        format!("{}@{}", self.username, self.host)
    }

    pub fn to_connect_request(&self, session_id: u64, auth: AuthConfig) -> ConnectRequest {
        ConnectRequest {
            session_id,
            title: self.display_name(),
            kind: ConnectionKind::Ssh,
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth: Some(auth),
            jump_host: None,
            local_forward: None,
            local_shell: None,
        }
    }
}

const MAX_SESSION_LOGS: usize = 200;

impl SavedState {
    pub fn record_session_log(&mut self, entry: SessionLogEntry) {
        self.session_logs.push(entry);
        if self.session_logs.len() > MAX_SESSION_LOGS {
            let drain_count = self.session_logs.len() - MAX_SESSION_LOGS;
            self.session_logs.drain(..drain_count);
        }
    }

    pub fn update_session_log(&mut self, log_id: &str, updater: impl FnOnce(&mut SessionLogEntry)) {
        if let Some(entry) = self.session_logs.iter_mut().find(|e| e.id == log_id) {
            updater(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DEFAULT_VAULT_ID, DraftProfile,
        IdentitySource, ImportedIdentity, JumpHostConnection, LocalPortForward, LocalShellConfig,
        QuickConnect, RestorableAuth, RestorableConnection, SavedIdentity, SavedSnippet,
        SavedState, SavedVault, SavedVaultMember, SavedWorkspace, SplitAxis, VaultKind,
        VaultMemberRole, identity_id_for_path,
    };

    #[test]
    fn parses_user_at_host() {
        let qc = QuickConnect::parse("root@192.168.1.1").unwrap();
        assert_eq!(qc.username, "root");
        assert_eq!(qc.host, "192.168.1.1");
        assert_eq!(qc.port, 22);
    }

    #[test]
    fn parses_user_at_host_with_port() {
        let qc = QuickConnect::parse("admin@example.com:2222").unwrap();
        assert_eq!(qc.username, "admin");
        assert_eq!(qc.host, "example.com");
        assert_eq!(qc.port, 2222);
    }

    #[test]
    fn parses_ssh_prefix() {
        let qc = QuickConnect::parse("ssh user@host.io").unwrap();
        assert_eq!(qc.username, "user");
        assert_eq!(qc.host, "host.io");
    }

    #[test]
    fn rejects_bare_hostname() {
        assert!(QuickConnect::parse("example.com").is_none());
    }

    #[test]
    fn rejects_empty() {
        assert!(QuickConnect::parse("").is_none());
        assert!(QuickConnect::parse("   ").is_none());
    }

    #[test]
    fn password_sessions_are_not_restorable() {
        let request = ConnectRequest {
            session_id: 1,
            title: "app".to_string(),
            kind: ConnectionKind::Ssh,
            host: "example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: Some(AuthConfig::Password {
                password: "secret".to_string(),
            }),
            jump_host: None,
            local_forward: None,
            local_shell: None,
        };

        assert!(request.to_restorable().is_none());
    }

    #[test]
    fn keychain_password_sessions_are_restorable() {
        let request = ConnectRequest {
            session_id: 1,
            title: "app".to_string(),
            kind: ConnectionKind::Ssh,
            host: "example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: Some(AuthConfig::PasswordRef {
                credential_id: "profile:app".to_string(),
            }),
            jump_host: None,
            local_forward: None,
            local_shell: None,
        };

        let restored = request.to_restorable().unwrap();
        let request = restored.to_connect_request(2);
        match request.auth.unwrap() {
            AuthConfig::PasswordRef { credential_id } => {
                assert_eq!(credential_id, "profile:app");
            }
            _ => panic!("expected keychain-backed password auth"),
        }
    }

    #[test]
    fn private_key_sessions_round_trip_as_restorable() {
        let request = ConnectRequest {
            session_id: 7,
            title: "prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 2222,
            username: "ubuntu".to_string(),
            auth: Some(AuthConfig::PrivateKey {
                key_path: "/tmp/id_ed25519".to_string(),
                passphrase: Some("ignored".to_string()),
            }),
            jump_host: Some(JumpHostConnection {
                title: "bastion".to_string(),
                host: "bastion.example.com".to_string(),
                port: 22,
                username: "ubuntu".to_string(),
                auth: AuthConfig::PrivateKey {
                    key_path: "/tmp/jump_id_ed25519".to_string(),
                    passphrase: None,
                },
                jump_host: Some(Box::new(JumpHostConnection {
                    title: "edge".to_string(),
                    host: "edge.example.com".to_string(),
                    port: 22,
                    username: "ubuntu".to_string(),
                    auth: AuthConfig::PrivateKey {
                        key_path: "/tmp/edge_id_ed25519".to_string(),
                        passphrase: None,
                    },
                    jump_host: None,
                })),
            }),
            local_forward: Some(LocalPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port: 15432,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 5432,
            }),
            local_shell: None,
        };

        let restored = request.to_restorable().unwrap();
        assert_eq!(restored.title, "prod");
        assert_eq!(restored.port, 2222);

        let request = restored.to_connect_request(9);
        assert_eq!(request.session_id, 9);
        assert_eq!(
            request
                .local_forward
                .as_ref()
                .map(LocalPortForward::display_name),
            Some("127.0.0.1:15432 -> 127.0.0.1:5432".to_string())
        );
        assert_eq!(
            request.jump_host.as_ref().map(|jump| jump.title.clone()),
            Some("bastion".to_string())
        );
        assert_eq!(
            request
                .jump_host
                .as_ref()
                .and_then(|jump| jump.jump_host.as_ref())
                .map(|jump| jump.title.clone()),
            Some("edge".to_string())
        );
        match request.auth.unwrap() {
            AuthConfig::PrivateKey {
                key_path,
                passphrase,
            } => {
                assert_eq!(key_path, "/tmp/id_ed25519");
                assert_eq!(passphrase, None);
            }
            AuthConfig::Password { .. } | AuthConfig::PasswordRef { .. } => {
                panic!("expected private key auth")
            }
        }
    }

    #[test]
    fn saved_workspace_normalizes_active_pane() {
        let mut workspace = SavedWorkspace {
            title: "prod".to_string(),
            split_axis: SplitAxis::Vertical,
            active_pane_index: 5,
            panes: vec![RestorableConnection {
                title: "prod".to_string(),
                kind: ConnectionKind::Ssh,
                host: "prod.example.com".to_string(),
                port: 22,
                username: "ubuntu".to_string(),
                auth: Some(RestorableAuth::PasswordKeychain {
                    credential_id: "profile:prod".to_string(),
                }),
                jump_host: None,
                local_forward: None,
                local_shell: None,
            }],
        };

        workspace.normalize();
        assert_eq!(workspace.active_pane_index, 0);
    }

    #[test]
    fn local_shell_sessions_round_trip_as_restorable() {
        let request = ConnectRequest {
            session_id: 11,
            title: "Local Terminal".to_string(),
            kind: ConnectionKind::LocalShell,
            host: "local".to_string(),
            port: 0,
            username: "jacob".to_string(),
            auth: None,
            jump_host: None,
            local_forward: None,
            local_shell: Some(LocalShellConfig {
                program: "/bin/zsh".to_string(),
                args: vec!["-l".to_string()],
                cwd: Some("/tmp".to_string()),
            }),
        };

        let restored = request.to_restorable().unwrap();
        assert_eq!(restored.kind, ConnectionKind::LocalShell);
        assert!(restored.auth.is_none());

        let round_trip = restored.to_connect_request(12);
        assert!(round_trip.is_local_shell());
        assert_eq!(round_trip.session_id, 12);
        assert_eq!(
            round_trip
                .local_shell
                .as_ref()
                .map(|shell| shell.program.as_str()),
            Some("/bin/zsh")
        );
    }

    #[test]
    fn imported_identities_become_saved_identities() {
        let mut state = SavedState::default();
        state.merge_imported_identities(vec![ImportedIdentity {
            label: "id_ed25519".to_string(),
            path: "/tmp/id_ed25519".to_string(),
            kind: "OpenSSH".to_string(),
        }]);

        assert_eq!(state.identities.len(), 1);
        assert_eq!(state.identities[0].source, IdentitySource::Imported);
        assert_eq!(
            state.identities[0].id,
            identity_id_for_path("/tmp/id_ed25519")
        );
    }

    #[test]
    fn imported_identities_do_not_replace_user_identities() {
        let mut state = SavedState::default();
        state.identities.push(SavedIdentity {
            id: identity_id_for_path("/tmp/id_ed25519"),
            label: "prod-key".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            key_path: "/tmp/id_ed25519".to_string(),
            kind: "OpenSSH".to_string(),
            source: IdentitySource::User,
        });

        state.merge_imported_identities(vec![ImportedIdentity {
            label: "id_ed25519".to_string(),
            path: "/tmp/id_ed25519".to_string(),
            kind: "OpenSSH".to_string(),
        }]);

        assert_eq!(state.identities.len(), 1);
        assert_eq!(state.identities[0].label, "prod-key");
        assert_eq!(state.identities[0].source, IdentitySource::User);
    }

    #[test]
    fn draft_profile_keeps_identity_reference_for_private_key_hosts() {
        let draft = DraftProfile {
            label: "prod".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Production".to_string(),
            host: "prod.example.com".to_string(),
            port: "22".to_string(),
            username: "ubuntu".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
        };

        let profile = draft.to_profile("profile-1".to_string()).unwrap();
        assert_eq!(profile.identity_id.as_deref(), Some("identity-123"));
    }

    #[test]
    fn snippets_are_sorted_by_display_name() {
        let mut state = SavedState::default();
        state.upsert_snippet(SavedSnippet {
            id: "b".to_string(),
            label: "Restart".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Ops".to_string(),
            command: "sudo systemctl restart app".to_string(),
        });
        state.upsert_snippet(SavedSnippet {
            id: "a".to_string(),
            label: "Deploy".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Ops".to_string(),
            command: "./deploy.sh".to_string(),
        });

        assert_eq!(state.snippets.len(), 2);
        assert_eq!(state.snippets[0].label, "Deploy");
        assert_eq!(state.snippets[1].label, "Restart");
    }

    #[test]
    fn snippets_can_be_removed() {
        let mut state = SavedState::default();
        state.upsert_snippet(SavedSnippet {
            id: "snippet-1".to_string(),
            label: "Tail logs".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: String::new(),
            command: "tail -f /var/log/app.log".to_string(),
        });

        state.remove_snippet("snippet-1");
        assert!(state.snippets.is_empty());
    }

    #[test]
    fn draft_profile_parses_local_forward() {
        let draft = DraftProfile {
            label: "db".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Data".to_string(),
            host: "db.example.com".to_string(),
            port: "22".to_string(),
            username: "postgres".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            forward_local_port: "15432".to_string(),
            forward_remote_host: "127.0.0.1".to_string(),
            forward_remote_port: "5432".to_string(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
        };

        let profile = draft.to_profile("profile-2".to_string()).unwrap();
        assert_eq!(
            profile
                .local_forward
                .as_ref()
                .map(LocalPortForward::display_name),
            Some("127.0.0.1:15432 -> 127.0.0.1:5432".to_string())
        );
    }

    #[test]
    fn state_ensures_personal_vault_and_normalizes_item_references() {
        let mut state = SavedState::default();
        state.vaults.push(SavedVault {
            id: "vault-team".to_string(),
            label: "Team".to_string(),
            description: String::new(),
            kind: VaultKind::Shared,
            members: Vec::new(),
        });
        state.snippets.push(SavedSnippet {
            id: "snippet-2".to_string(),
            label: "Deploy".to_string(),
            vault_id: Some("vault-missing".to_string()),
            group: String::new(),
            command: "./deploy.sh".to_string(),
        });

        state.ensure_vaults();

        assert!(
            state
                .vaults
                .iter()
                .any(|vault| vault.id == DEFAULT_VAULT_ID)
        );
        assert_eq!(
            state.snippets[0].vault_id.as_deref(),
            Some(DEFAULT_VAULT_ID)
        );
        assert!(
            state
                .vaults
                .iter()
                .find(|vault| vault.id == "vault-team")
                .is_some_and(|vault| !vault.members.is_empty())
        );
    }

    #[test]
    fn shared_vault_members_are_upserted_and_sorted() {
        let mut vault = SavedVault {
            id: "vault-team".to_string(),
            label: "Team".to_string(),
            description: String::new(),
            kind: VaultKind::Shared,
            members: Vec::new(),
        };

        vault.upsert_member(SavedVaultMember {
            id: "member-2".to_string(),
            name: "Viewer User".to_string(),
            email: "viewer@example.com".to_string(),
            role: VaultMemberRole::Viewer,
        });
        vault.upsert_member(SavedVaultMember {
            id: "member-1".to_string(),
            name: "Editor User".to_string(),
            email: "editor@example.com".to_string(),
            role: VaultMemberRole::Editor,
        });

        assert!(
            vault
                .members
                .iter()
                .any(|member| member.role == VaultMemberRole::Owner)
        );
        assert_eq!(vault.members[1].role, VaultMemberRole::Editor);
        assert_eq!(vault.members[2].role, VaultMemberRole::Viewer);
    }
}
