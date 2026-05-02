use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn default_ssh_port() -> u16 {
    22
}

fn default_local_forward_host() -> String {
    "127.0.0.1".to_string()
}

fn default_terminal_font_size() -> u16 {
    14
}

fn default_session_log_limit() -> u16 {
    200
}

fn default_restore_workspaces_on_launch() -> bool {
    true
}

fn default_terminal_scrollback_rows() -> u32 {
    10_000
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
pub enum ThemePreset {
    #[default]
    Ocean,
    Daylight,
    FlexokiDark,
    FlexokiLight,
    KanagawaWave,
    KanagawaDragon,
    KanagawaLotus,
    HackerBlue,
    HackerGreen,
    HackerRed,
}

impl ThemePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ocean => "Termius Dark",
            Self::Daylight => "Termius Light",
            Self::FlexokiDark => "Flexoki Dark",
            Self::FlexokiLight => "Flexoki Light",
            Self::KanagawaWave => "Kanagawa Wave",
            Self::KanagawaDragon => "Kanagawa Dragon",
            Self::KanagawaLotus => "Kanagawa Lotus",
            Self::HackerBlue => "Hacker Blue",
            Self::HackerGreen => "Hacker Green",
            Self::HackerRed => "Hacker Red",
        }
    }

    pub fn all() -> [ThemePreset; 10] {
        [
            Self::Ocean,
            Self::Daylight,
            Self::FlexokiDark,
            Self::FlexokiLight,
            Self::KanagawaWave,
            Self::KanagawaDragon,
            Self::KanagawaLotus,
            Self::HackerBlue,
            Self::HackerGreen,
            Self::HackerRed,
        ]
    }

    pub fn preview_bg(self) -> u32 {
        match self {
            Self::Ocean => 0x07101c,
            Self::Daylight => 0xf6f1e6,
            Self::FlexokiDark => 0x100f0f,
            Self::FlexokiLight => 0xfffcf0,
            Self::KanagawaWave => 0x1f1f28,
            Self::KanagawaDragon => 0x181616,
            Self::KanagawaLotus => 0xf2ecbc,
            Self::HackerBlue => 0x0b1226,
            Self::HackerGreen => 0x06160a,
            Self::HackerRed => 0x1c0707,
        }
    }

    pub fn preview_accent(self) -> u32 {
        match self {
            Self::Ocean => 0x3ec97a,
            Self::Daylight => 0x2f9d7e,
            Self::FlexokiDark => 0xda702c,
            Self::FlexokiLight => 0xaf3029,
            Self::KanagawaWave => 0x7e9cd8,
            Self::KanagawaDragon => 0xc4746e,
            Self::KanagawaLotus => 0x4d699b,
            Self::HackerBlue => 0x4ea1ff,
            Self::HackerGreen => 0x3df36c,
            Self::HackerRed => 0xff5252,
        }
    }
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
    pub favorite: bool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<String>,
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
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub start_in_files: bool,
    #[serde(default)]
    pub terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    pub port_forward_rules: Vec<PortForwardRule>,
    #[serde(default)]
    pub local_forwards: Vec<LocalPortForward>,
    #[serde(default)]
    pub local_forward: Option<LocalPortForward>,
    #[serde(default)]
    pub password_credential_id: Option<String>,
    #[serde(default)]
    pub source: ProfileSource,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub color_tag: Option<HostColorTag>,
    #[serde(default)]
    pub environment: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostColorTag {
    Red,
    Amber,
    Green,
    Teal,
    Blue,
    Violet,
    Pink,
    Gray,
}

impl HostColorTag {
    pub fn label(self) -> &'static str {
        match self {
            HostColorTag::Red => "Red",
            HostColorTag::Amber => "Amber",
            HostColorTag::Green => "Green",
            HostColorTag::Teal => "Teal",
            HostColorTag::Blue => "Blue",
            HostColorTag::Violet => "Violet",
            HostColorTag::Pink => "Pink",
            HostColorTag::Gray => "Gray",
        }
    }

    pub fn rgb_hex(self) -> u32 {
        match self {
            HostColorTag::Red => 0xed4f4f,
            HostColorTag::Amber => 0xe39c42,
            HostColorTag::Green => 0x49b87b,
            HostColorTag::Teal => 0x2faea2,
            HostColorTag::Blue => 0x3f86eb,
            HostColorTag::Violet => 0x8c5cf2,
            HostColorTag::Pink => 0xd363a8,
            HostColorTag::Gray => 0x8794a8,
        }
    }

    pub fn all() -> [HostColorTag; 8] {
        [
            HostColorTag::Red,
            HostColorTag::Amber,
            HostColorTag::Green,
            HostColorTag::Teal,
            HostColorTag::Blue,
            HostColorTag::Violet,
            HostColorTag::Pink,
            HostColorTag::Gray,
        ]
    }
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

    pub fn normalize(&mut self) {
        self.startup_directory = self
            .startup_directory
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.startup_command = self
            .startup_command
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.terminal_scrollback_rows = self
            .terminal_scrollback_rows
            .take()
            .map(|rows| rows.clamp(500, 200_000));
        self.port_forward_rules = normalize_port_forward_rules(
            self.port_forward_rules.clone(),
            self.local_forwards.clone(),
            self.local_forward.clone(),
        );
        self.local_forwards =
            normalize_local_forwards(self.local_forwards.clone(), self.local_forward.clone());
        if !self.port_forward_rules.is_empty() || !self.local_forwards.is_empty() {
            self.local_forward = None;
        }
        if !self.port_forward_rules.is_empty() {
            self.local_forwards.clear();
        }
    }

    pub fn effective_port_forward_rules(&self) -> Vec<PortForwardRule> {
        normalize_port_forward_rules(
            self.port_forward_rules.clone(),
            self.local_forwards.clone(),
            self.local_forward.clone(),
        )
    }
}

fn sort_profiles(profiles: &mut [HostProfile]) {
    profiles.sort_by(|left, right| {
        right.favorite.cmp(&left.favorite).then_with(|| {
            left.display_name()
                .to_ascii_lowercase()
                .cmp(&right.display_name().to_ascii_lowercase())
        })
    });
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

    pub fn parse(local_port: &str, remote_host: &str, remote_port: &str) -> Result<Option<Self>> {
        let local_port = local_port.trim();
        let remote_host = remote_host.trim();
        let remote_port = remote_port.trim();

        if local_port.is_empty() && remote_host.is_empty() && remote_port.is_empty() {
            return Ok(None);
        }

        if local_port.is_empty() || remote_host.is_empty() || remote_port.is_empty() {
            bail!("Local forwarding requires local port, remote host, and remote port");
        }

        Ok(Some(Self {
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicPortForward {
    #[serde(default = "default_local_forward_host")]
    pub local_host: String,
    pub local_port: u16,
}

impl DynamicPortForward {
    pub fn display_name(&self) -> String {
        format!("SOCKS5 {}:{}", self.local_host, self.local_port)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePortForward {
    #[serde(default = "default_local_forward_host")]
    pub local_host: String,
    pub local_port: u16,
    #[serde(default = "default_local_forward_host")]
    pub remote_host: String,
    pub remote_port: u16,
}

impl RemotePortForward {
    pub fn display_name(&self) -> String {
        format!(
            "Remote {}:{} <- {}:{}",
            self.remote_host, self.remote_port, self.local_host, self.local_port
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardKind {
    #[default]
    Local,
    Dynamic,
    Remote,
}

impl PortForwardKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Dynamic => "Dynamic",
            Self::Remote => "Remote",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortForwardRule {
    Local {
        #[serde(flatten)]
        forward: LocalPortForward,
    },
    Dynamic {
        #[serde(flatten)]
        forward: DynamicPortForward,
    },
    Remote {
        #[serde(flatten)]
        forward: RemotePortForward,
    },
}

impl PortForwardRule {
    pub fn kind(&self) -> PortForwardKind {
        match self {
            Self::Local { .. } => PortForwardKind::Local,
            Self::Dynamic { .. } => PortForwardKind::Dynamic,
            Self::Remote { .. } => PortForwardKind::Remote,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Local { forward } => forward.display_name(),
            Self::Dynamic { forward } => forward.display_name(),
            Self::Remote { forward } => forward.display_name(),
        }
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
    #[serde(default)]
    pub pinned: bool,
    pub command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub theme_preset: ThemePreset,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: u16,
    #[serde(default)]
    pub onboarding_dismissed: bool,
    #[serde(default = "default_restore_workspaces_on_launch")]
    pub restore_workspaces_on_launch: bool,
    #[serde(default = "default_session_log_limit")]
    pub session_log_limit: u16,
    #[serde(default = "default_local_shell_config")]
    pub default_local_shell: LocalShellConfig,
    #[serde(default)]
    pub default_ssh_startup_directory: Option<String>,
    #[serde(default)]
    pub copy_on_select: bool,
    #[serde(default)]
    pub terminal_font_family: Option<String>,
    #[serde(default = "default_confirm_multiline_paste")]
    pub confirm_multiline_paste: bool,
    #[serde(default = "default_auto_reconnect_attempts")]
    pub auto_reconnect_attempts: u8,
    #[serde(default = "default_auto_reconnect_delay_secs")]
    pub auto_reconnect_delay_secs: u8,
    #[serde(default = "default_ssh_keepalive_secs")]
    pub ssh_keepalive_secs: u16,
    #[serde(default)]
    pub sync_folder_path: Option<String>,
    #[serde(default)]
    pub sync_last_pushed_at: Option<u64>,
    #[serde(default)]
    pub sync_last_pulled_at: Option<u64>,
}

fn default_confirm_multiline_paste() -> bool {
    true
}

fn default_auto_reconnect_attempts() -> u8 {
    3
}

fn default_auto_reconnect_delay_secs() -> u8 {
    5
}

fn default_ssh_keepalive_secs() -> u16 {
    30
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme_preset: ThemePreset::Ocean,
            terminal_font_size: default_terminal_font_size(),
            onboarding_dismissed: false,
            restore_workspaces_on_launch: default_restore_workspaces_on_launch(),
            session_log_limit: default_session_log_limit(),
            default_local_shell: default_local_shell_config(),
            default_ssh_startup_directory: None,
            copy_on_select: false,
            terminal_font_family: None,
            confirm_multiline_paste: default_confirm_multiline_paste(),
            auto_reconnect_attempts: default_auto_reconnect_attempts(),
            auto_reconnect_delay_secs: default_auto_reconnect_delay_secs(),
            ssh_keepalive_secs: default_ssh_keepalive_secs(),
            sync_folder_path: None,
            sync_last_pushed_at: None,
            sync_last_pulled_at: None,
        }
    }
}

impl AppSettings {
    pub fn normalize(&mut self) {
        self.terminal_font_size = self.terminal_font_size.clamp(11, 18);
        self.session_log_limit = self.session_log_limit.clamp(50, 1000);
        if self.default_local_shell.program.trim().is_empty() {
            self.default_local_shell.program = default_local_shell_config().program;
        }
        self.default_local_shell.cwd = self
            .default_local_shell
            .cwd
            .take()
            .filter(|cwd| !cwd.trim().is_empty());
        self.terminal_font_family = self
            .terminal_font_family
            .take()
            .map(|family| family.trim().to_string())
            .filter(|family| !family.is_empty());
        self.default_ssh_startup_directory = self
            .default_ssh_startup_directory
            .take()
            .map(|dir| dir.trim().to_string())
            .filter(|dir| !dir.is_empty());
        self.auto_reconnect_attempts = self.auto_reconnect_attempts.min(10);
        self.auto_reconnect_delay_secs = self.auto_reconnect_delay_secs.clamp(1, 60);
        self.ssh_keepalive_secs = self.ssh_keepalive_secs.min(300);
        self.sync_folder_path = self
            .sync_folder_path
            .take()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCommandHistoryEntry {
    pub command: String,
    #[serde(default)]
    pub scope_key: String,
    #[serde(default)]
    pub scope_label: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedHostGroup {
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub port_forward_rules: Vec<PortForwardRule>,
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

impl SavedHostGroup {
    pub fn normalize(&mut self) {
        self.label = self.label.trim().to_string();
        self.vault_id = self
            .vault_id
            .take()
            .filter(|value| !value.trim().is_empty());
        self.username = self
            .username
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.tags = normalize_tags(self.tags.clone());
        self.identity_id = self
            .identity_id
            .take()
            .filter(|value| !value.trim().is_empty());
        self.jump_host_id = self
            .jump_host_id
            .take()
            .filter(|value| !value.trim().is_empty());
        self.startup_directory = self
            .startup_directory
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.startup_command = self
            .startup_command
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.port_forward_rules =
            normalize_port_forward_rules(self.port_forward_rules.clone(), Vec::new(), None);
    }

    pub fn display_name(&self) -> String {
        self.label.trim().to_string()
    }
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = tag.trim().trim_start_matches('#');
        if tag.is_empty() {
            continue;
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            normalized.push(tag.to_string());
        }
    }
    normalized
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
    pub settings: AppSettings,
    #[serde(default)]
    pub vaults: Vec<SavedVault>,
    #[serde(default)]
    pub host_groups: Vec<SavedHostGroup>,
    #[serde(default)]
    pub profiles: Vec<HostProfile>,
    #[serde(default)]
    pub identities: Vec<SavedIdentity>,
    #[serde(default)]
    pub snippets: Vec<SavedSnippet>,
    #[serde(default)]
    pub command_history: Vec<String>,
    #[serde(default)]
    pub scoped_command_history: Vec<SavedCommandHistoryEntry>,
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
    fn trim_session_logs_to_limit(&mut self) {
        let max_session_logs = usize::from(self.settings.session_log_limit);
        if self.session_logs.len() > max_session_logs {
            let drain_count = self.session_logs.len() - max_session_logs;
            self.session_logs.drain(..drain_count);
        }
    }

    pub fn ensure_host_groups(&mut self) {
        for group in &mut self.host_groups {
            group.normalize();
        }
        self.host_groups
            .retain(|group| !group.label.trim().is_empty());
        self.host_groups.sort_by(|left, right| {
            left.display_name()
                .to_ascii_lowercase()
                .cmp(&right.display_name().to_ascii_lowercase())
        });
    }

    pub fn ensure_settings(&mut self) {
        self.settings.normalize();
        self.trim_session_logs_to_limit();
    }

    pub fn record_command_history(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        self.command_history.retain(|existing| existing != command);
        self.command_history.push(command.to_string());

        const MAX_COMMAND_HISTORY: usize = 200;
        if self.command_history.len() > MAX_COMMAND_HISTORY {
            let drain_count = self.command_history.len() - MAX_COMMAND_HISTORY;
            self.command_history.drain(..drain_count);
        }
    }

    pub fn record_command_history_for_scope(
        &mut self,
        command: &str,
        scope_key: &str,
        scope_label: &str,
    ) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        self.record_command_history(command);

        let scope_key = scope_key.trim();
        let scope_label = scope_label.trim();
        if scope_key.is_empty() {
            return;
        }

        self.scoped_command_history
            .retain(|existing| !(existing.command == command && existing.scope_key == scope_key));
        self.scoped_command_history.push(SavedCommandHistoryEntry {
            command: command.to_string(),
            scope_key: scope_key.to_string(),
            scope_label: scope_label.to_string(),
        });

        const MAX_SCOPED_COMMAND_HISTORY: usize = 400;
        if self.scoped_command_history.len() > MAX_SCOPED_COMMAND_HISTORY {
            let drain_count = self.scoped_command_history.len() - MAX_SCOPED_COMMAND_HISTORY;
            self.scoped_command_history.drain(..drain_count);
        }
    }

    pub fn ensure_vaults(&mut self) {
        self.ensure_settings();
        self.ensure_host_groups();
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
            profile.normalize();
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

    pub fn upsert_host_group(&mut self, mut group: SavedHostGroup) {
        group.normalize();
        if group.label.is_empty() {
            return;
        }

        if let Some(existing) = self
            .host_groups
            .iter_mut()
            .find(|item| item.label.eq_ignore_ascii_case(&group.label))
        {
            *existing = group;
        } else {
            self.host_groups.push(group);
        }

        self.ensure_host_groups();
    }

    pub fn remove_host_group(&mut self, label: &str) -> bool {
        let before = self.host_groups.len();
        self.host_groups
            .retain(|group| !group.label.eq_ignore_ascii_case(label));
        self.ensure_host_groups();
        before != self.host_groups.len()
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
        profile.normalize();

        if let Some(existing) = self.profiles.iter_mut().find(|item| item.id == profile.id) {
            *existing = profile.clone();
        } else {
            self.profiles.push(profile.clone());
        }

        self.ensure_vaults();
        sort_profiles(&mut self.profiles);
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
            imported.normalize();
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
        sort_profiles(&mut self.profiles);
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
        self.snippets.sort_by(|left, right| {
            right.pinned.cmp(&left.pinned).then_with(|| {
                left.display_name()
                    .to_ascii_lowercase()
                    .cmp(&right.display_name().to_ascii_lowercase())
            })
        });
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

pub fn parse_environment_pairs(raw: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || key.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            continue;
        }
        let mut value = value.trim().to_string();
        if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            value = value[1..value.len() - 1].to_string();
        }
        pairs.push((key.to_string(), value));
    }
    pairs
}

pub fn format_environment_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
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
    pub favorite: bool,
    pub group: String,
    pub tags: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub key_path: String,
    pub identity_id: Option<String>,
    pub jump_host_id: Option<String>,
    pub startup_directory: String,
    pub startup_command: String,
    pub start_in_files: bool,
    pub terminal_scrollback_rows: String,
    pub saved_port_forward_rules: Vec<PortForwardRule>,
    pub forward_kind: PortForwardKind,
    pub forward_local_port: String,
    pub forward_remote_host: String,
    pub forward_remote_port: String,
    pub key_passphrase: String,
    pub password_credential_id: Option<String>,
    pub auth_mode: AuthMode,
    pub description: String,
    pub color_tag: Option<HostColorTag>,
    pub environment: String,
}

impl DraftProfile {
    pub fn from_profile(profile: &HostProfile) -> Self {
        Self {
            label: profile.label.clone(),
            vault_id: profile.vault_id.clone(),
            favorite: profile.favorite,
            group: profile.group.clone(),
            tags: profile.tags.join(", "),
            host: profile.host.clone(),
            port: profile.port.to_string(),
            username: profile.username.clone(),
            password: String::new(),
            key_path: profile.key_path.clone(),
            identity_id: profile.identity_id.clone(),
            jump_host_id: profile.jump_host_id.clone(),
            startup_directory: profile.startup_directory.clone().unwrap_or_default(),
            startup_command: profile.startup_command.clone().unwrap_or_default(),
            start_in_files: profile.start_in_files,
            terminal_scrollback_rows: profile
                .terminal_scrollback_rows
                .unwrap_or(default_terminal_scrollback_rows())
                .to_string(),
            saved_port_forward_rules: profile.effective_port_forward_rules(),
            forward_kind: PortForwardKind::Local,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: profile.password_credential_id.clone(),
            auth_mode: profile.auth_mode,
            description: profile.description.clone(),
            color_tag: profile.color_tag,
            environment: format_environment_pairs(&profile.environment),
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

    pub fn parse_pending_port_forward_rule(&self) -> Result<Option<PortForwardRule>> {
        match self.forward_kind {
            PortForwardKind::Local => Ok(LocalPortForward::parse(
                &self.forward_local_port,
                &self.forward_remote_host,
                &self.forward_remote_port,
            )?
            .map(|forward| PortForwardRule::Local { forward })),
            PortForwardKind::Dynamic => {
                let local_port = self.forward_local_port.trim();
                if local_port.is_empty()
                    && self.forward_remote_host.trim().is_empty()
                    && self.forward_remote_port.trim().is_empty()
                {
                    return Ok(None);
                }

                if local_port.is_empty() {
                    bail!("Dynamic forwarding requires a local port");
                }

                Ok(Some(PortForwardRule::Dynamic {
                    forward: DynamicPortForward {
                        local_host: default_local_forward_host(),
                        local_port: local_port.parse::<u16>().with_context(|| {
                            format!("Invalid local forward port '{local_port}'")
                        })?,
                    },
                }))
            }
            PortForwardKind::Remote => {
                let local_port = self.forward_local_port.trim();
                let remote_host = self.forward_remote_host.trim();
                let remote_port = self.forward_remote_port.trim();

                if local_port.is_empty() && remote_host.is_empty() && remote_port.is_empty() {
                    return Ok(None);
                }

                if local_port.is_empty() || remote_host.is_empty() || remote_port.is_empty() {
                    bail!("Remote forwarding requires local port, remote host, and remote port");
                }

                Ok(Some(PortForwardRule::Remote {
                    forward: RemotePortForward {
                        local_host: default_local_forward_host(),
                        local_port: local_port.parse::<u16>().with_context(|| {
                            format!("Invalid local forward port '{local_port}'")
                        })?,
                        remote_host: remote_host.to_string(),
                        remote_port: remote_port.parse::<u16>().with_context(|| {
                            format!("Invalid remote forward port '{remote_port}'")
                        })?,
                    },
                }))
            }
        }
    }

    pub(crate) fn parse_port_forward_rules(&self) -> Result<Vec<PortForwardRule>> {
        let mut rules = self.saved_port_forward_rules.clone();
        if let Some(pending) = self.parse_pending_port_forward_rule()? {
            rules.push(pending);
        }
        Ok(normalize_port_forward_rules(rules, Vec::new(), None))
    }

    fn parse_terminal_scrollback_rows(&self) -> Result<Option<u32>> {
        let value = self.terminal_scrollback_rows.trim();
        if value.is_empty() {
            return Ok(None);
        }

        let rows = value
            .parse::<u32>()
            .with_context(|| format!("Invalid scrollback rows '{value}'"))?;
        if !(500..=200_000).contains(&rows) {
            bail!("Scrollback rows must be between 500 and 200000");
        }
        Ok(Some(rows))
    }

    fn parse_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for raw in self.tags.split(',') {
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
            favorite: self.favorite,
            group: self.group.trim().to_string(),
            tags: self.parse_tags(),
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
            startup_directory: non_empty(self.startup_directory.trim()),
            startup_command: non_empty(self.startup_command.trim()),
            start_in_files: self.start_in_files,
            terminal_scrollback_rows: self.parse_terminal_scrollback_rows()?,
            port_forward_rules: self.parse_port_forward_rules()?,
            local_forwards: Vec::new(),
            local_forward: None,
            password_credential_id: if self.auth_mode == AuthMode::Password {
                self.password_credential_id.clone()
            } else {
                None
            },
            source: ProfileSource::User,
            description: self.description.trim().to_string(),
            color_tag: self.color_tag,
            environment: parse_environment_pairs(&self.environment),
        })
    }

    pub fn to_connect_request(&self, session_id: u64) -> Result<ConnectRequest> {
        let profile = self.to_profile(Self::profile_id())?;
        let port_forward_rules = profile.effective_port_forward_rules();

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
            startup_directory: profile.startup_directory,
            startup_command: profile.startup_command,
            start_in_files: profile.start_in_files,
            terminal_scrollback_rows: profile
                .terminal_scrollback_rows
                .unwrap_or(default_terminal_scrollback_rows())
                as usize,
            port_forward_rules,
            local_shell: None,
            environment: profile.environment,
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

fn normalize_local_forwards(
    mut local_forwards: Vec<LocalPortForward>,
    local_forward: Option<LocalPortForward>,
) -> Vec<LocalPortForward> {
    if local_forwards.is_empty() {
        if let Some(local_forward) = local_forward {
            local_forwards.push(local_forward);
        }
    }

    let mut normalized = Vec::new();
    for forward in local_forwards {
        if normalized.iter().any(|existing: &LocalPortForward| {
            existing.local_host == forward.local_host
                && existing.local_port == forward.local_port
                && existing.remote_host == forward.remote_host
                && existing.remote_port == forward.remote_port
        }) {
            continue;
        }
        normalized.push(forward);
    }

    normalized
}

fn normalize_port_forward_rules(
    mut port_forward_rules: Vec<PortForwardRule>,
    local_forwards: Vec<LocalPortForward>,
    local_forward: Option<LocalPortForward>,
) -> Vec<PortForwardRule> {
    if port_forward_rules.is_empty() {
        port_forward_rules.extend(
            normalize_local_forwards(local_forwards, local_forward)
                .into_iter()
                .map(|forward| PortForwardRule::Local { forward }),
        );
    }

    let mut normalized = Vec::new();
    for rule in port_forward_rules {
        if normalized
            .iter()
            .any(|existing: &PortForwardRule| existing == &rule)
        {
            continue;
        }
        normalized.push(rule);
    }

    normalized
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
    pub startup_directory: Option<String>,
    pub startup_command: Option<String>,
    pub start_in_files: bool,
    pub terminal_scrollback_rows: usize,
    pub port_forward_rules: Vec<PortForwardRule>,
    pub local_shell: Option<LocalShellConfig>,
    pub environment: Vec<(String, String)>,
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

    pub fn history_scope_key(&self) -> String {
        match self.kind {
            ConnectionKind::Ssh => format!("ssh:{}@{}:{}", self.username, self.host, self.port),
            ConnectionKind::LocalShell => format!(
                "local:{}",
                self.local_shell
                    .as_ref()
                    .map(LocalShellConfig::display_name)
                    .unwrap_or_else(|| "default".to_string())
            ),
        }
    }

    pub fn history_scope_label(&self) -> String {
        match self.kind {
            ConnectionKind::Ssh => {
                if self.title.trim().is_empty() {
                    format!("{}@{}:{}", self.username, self.host, self.port)
                } else {
                    self.title.trim().to_string()
                }
            }
            ConnectionKind::LocalShell => self.endpoint_label(),
        }
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
                startup_directory: self.startup_directory.clone(),
                startup_command: self.startup_command.clone(),
                start_in_files: self.start_in_files,
                terminal_scrollback_rows: Some(self.terminal_scrollback_rows as u32),
                port_forward_rules: self.port_forward_rules.clone(),
                local_forwards: Vec::new(),
                local_forward: None,
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
                startup_directory: None,
                startup_command: None,
                start_in_files: false,
                terminal_scrollback_rows: None,
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
                local_forward: None,
                local_shell: self.local_shell.clone(),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn local_shell(session_id: u64) -> Self {
        let shell = default_local_shell_config();
        Self::local_shell_with_config(session_id, shell)
    }

    pub fn local_shell_with_config(session_id: u64, shell: LocalShellConfig) -> Self {
        Self {
            session_id,
            title: "Local Terminal".to_string(),
            kind: ConnectionKind::LocalShell,
            host: "local".to_string(),
            port: 0,
            username: current_username(),
            auth: None,
            jump_host: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: Some(shell),
            environment: Vec::new(),
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
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub start_in_files: bool,
    #[serde(default)]
    pub terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    pub port_forward_rules: Vec<PortForwardRule>,
    #[serde(default)]
    pub local_forwards: Vec<LocalPortForward>,
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
                    startup_directory: self.startup_directory.clone(),
                    startup_command: self.startup_command.clone(),
                    start_in_files: self.start_in_files,
                    terminal_scrollback_rows: self
                        .terminal_scrollback_rows
                        .unwrap_or(default_terminal_scrollback_rows())
                        as usize,
                    port_forward_rules: normalize_port_forward_rules(
                        self.port_forward_rules.clone(),
                        self.local_forwards.clone(),
                        self.local_forward.clone(),
                    ),
                    local_shell: None,
                    environment: Vec::new(),
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
                startup_directory: None,
                startup_command: None,
                start_in_files: false,
                terminal_scrollback_rows: default_terminal_scrollback_rows() as usize,
                port_forward_rules: Vec::new(),
                local_shell: self
                    .local_shell
                    .clone()
                    .or_else(|| Some(default_local_shell_config())),
                environment: Vec::new(),
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
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }
}

impl SavedState {
    pub fn record_session_log(&mut self, entry: SessionLogEntry) {
        self.session_logs.push(entry);
        self.trim_session_logs_to_limit();
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
        AppSettings, AuthConfig, AuthMode, ConnectRequest, ConnectionKind, DEFAULT_VAULT_ID,
        DraftProfile, HostColorTag, HostProfile, IdentitySource, ImportedIdentity,
        JumpHostConnection, LocalPortForward, LocalShellConfig, PortForwardKind, PortForwardRule,
        ProfileSource, QuickConnect, RestorableAuth, RestorableConnection,
        SavedCommandHistoryEntry, SavedIdentity, SavedSnippet, SavedState, SavedVault,
        SavedVaultMember, SavedWorkspace, SessionLogEntry, SessionLogStatus, SplitAxis,
        ThemePreset, VaultKind, VaultMemberRole, identity_id_for_path,
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
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
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
            startup_directory: Some("/srv/app".to_string()),
            startup_command: Some("git status".to_string()),
            start_in_files: true,
            terminal_scrollback_rows: 4096,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };

        let restored = request.to_restorable().unwrap();
        assert!(restored.start_in_files);
        assert_eq!(restored.terminal_scrollback_rows, Some(4096));
        let request = restored.to_connect_request(2);
        assert!(request.start_in_files);
        assert_eq!(request.terminal_scrollback_rows, 4096);
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
            startup_directory: Some("/var/www/prod".to_string()),
            startup_command: Some("docker compose ps".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 12000,
            port_forward_rules: vec![
                PortForwardRule::Local {
                    forward: LocalPortForward {
                        local_host: "127.0.0.1".to_string(),
                        local_port: 15432,
                        remote_host: "127.0.0.1".to_string(),
                        remote_port: 5432,
                    },
                },
                PortForwardRule::Local {
                    forward: LocalPortForward {
                        local_host: "127.0.0.1".to_string(),
                        local_port: 18080,
                        remote_host: "10.0.0.20".to_string(),
                        remote_port: 8080,
                    },
                },
            ],
            local_shell: None,
            environment: Vec::new(),
        };

        let restored = request.to_restorable().unwrap();
        assert_eq!(restored.title, "prod");
        assert_eq!(restored.port, 2222);
        assert_eq!(restored.startup_directory.as_deref(), Some("/var/www/prod"));
        assert_eq!(
            restored.startup_command.as_deref(),
            Some("docker compose ps")
        );
        assert!(!restored.start_in_files);
        assert_eq!(restored.terminal_scrollback_rows, Some(12_000));

        let request = restored.to_connect_request(9);
        assert_eq!(request.session_id, 9);
        assert_eq!(request.terminal_scrollback_rows, 12_000);
        assert_eq!(request.port_forward_rules.len(), 2);
        assert_eq!(
            request.port_forward_rules[0].display_name(),
            "127.0.0.1:15432 -> 127.0.0.1:5432"
        );
        assert_eq!(
            request.port_forward_rules[1].display_name(),
            "127.0.0.1:18080 -> 10.0.0.20:8080"
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
                startup_directory: None,
                startup_command: None,
                start_in_files: false,
                terminal_scrollback_rows: None,
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
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
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: Some(LocalShellConfig {
                program: "/bin/zsh".to_string(),
                args: vec!["-l".to_string()],
                cwd: Some("/tmp".to_string()),
            }),
            environment: Vec::new(),
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
            favorite: true,
            group: "Production".to_string(),
            tags: "critical, web, #blue".to_string(),
            host: "prod.example.com".to_string(),
            port: "22".to_string(),
            username: "ubuntu".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: "/var/www/app".to_string(),
            startup_command: "docker compose ps".to_string(),
            start_in_files: true,
            terminal_scrollback_rows: "4096".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Local,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
            description: "  Primary blue-green node  ".to_string(),
            color_tag: None,
            environment: String::new(),
        };

        let profile = draft.to_profile("profile-1".to_string()).unwrap();
        assert_eq!(profile.description, "Primary blue-green node");
        let round_trip = DraftProfile::from_profile(&profile);
        assert_eq!(round_trip.description, "Primary blue-green node");
        assert!(profile.favorite);
        assert_eq!(profile.identity_id.as_deref(), Some("identity-123"));
        assert_eq!(profile.startup_directory.as_deref(), Some("/var/www/app"));
        assert_eq!(
            profile.startup_command.as_deref(),
            Some("docker compose ps")
        );
        assert!(profile.start_in_files);
        assert_eq!(profile.terminal_scrollback_rows, Some(4096));
        assert_eq!(
            profile.tags,
            vec![
                "critical".to_string(),
                "web".to_string(),
                "blue".to_string()
            ]
        );
    }

    #[test]
    fn draft_profile_tags_are_deduplicated_case_insensitively() {
        let draft = DraftProfile {
            label: "ops".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            favorite: false,
            group: "Operations".to_string(),
            tags: "Prod, prod, #ops, ops ".to_string(),
            host: "ops.example.com".to_string(),
            port: "22".to_string(),
            username: "root".to_string(),
            password: String::new(),
            key_path: String::new(),
            identity_id: None,
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Local,
            forward_local_port: String::new(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::Password,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };

        let profile = draft.to_profile("profile-2".to_string()).unwrap();
        assert_eq!(profile.tags, vec!["Prod".to_string(), "ops".to_string()]);
    }

    #[test]
    fn favorite_profiles_are_sorted_before_other_hosts() {
        let mut state = SavedState::default();
        state.upsert_profile(
            DraftProfile {
                label: "zeta".to_string(),
                vault_id: Some(DEFAULT_VAULT_ID.to_string()),
                favorite: false,
                group: String::new(),
                tags: String::new(),
                host: "zeta.example.com".to_string(),
                port: "22".to_string(),
                username: "ubuntu".to_string(),
                password: "secret".to_string(),
                key_path: String::new(),
                identity_id: None,
                jump_host_id: None,
                startup_directory: String::new(),
                startup_command: String::new(),
                start_in_files: false,
                terminal_scrollback_rows: "10000".to_string(),
                saved_port_forward_rules: Vec::new(),
                forward_kind: PortForwardKind::Local,
                forward_local_port: String::new(),
                forward_remote_host: String::new(),
                forward_remote_port: String::new(),
                key_passphrase: String::new(),
                password_credential_id: None,
                auth_mode: AuthMode::Password,
                description: String::new(),
                color_tag: None,
                environment: String::new(),
            }
            .to_profile("profile-zeta".to_string())
            .unwrap(),
        );
        state.upsert_profile(
            DraftProfile {
                label: "alpha".to_string(),
                vault_id: Some(DEFAULT_VAULT_ID.to_string()),
                favorite: true,
                group: String::new(),
                tags: String::new(),
                host: "alpha.example.com".to_string(),
                port: "22".to_string(),
                username: "ubuntu".to_string(),
                password: "secret".to_string(),
                key_path: String::new(),
                identity_id: None,
                jump_host_id: None,
                startup_directory: String::new(),
                startup_command: String::new(),
                start_in_files: false,
                terminal_scrollback_rows: "10000".to_string(),
                saved_port_forward_rules: Vec::new(),
                forward_kind: PortForwardKind::Local,
                forward_local_port: String::new(),
                forward_remote_host: String::new(),
                forward_remote_port: String::new(),
                key_passphrase: String::new(),
                password_credential_id: None,
                auth_mode: AuthMode::Password,
                description: String::new(),
                color_tag: None,
                environment: String::new(),
            }
            .to_profile("profile-alpha".to_string())
            .unwrap(),
        );

        assert_eq!(state.profiles.len(), 2);
        assert_eq!(state.profiles[0].label, "alpha");
        assert!(state.profiles[0].favorite);
        assert_eq!(state.profiles[1].label, "zeta");
    }

    #[test]
    fn snippets_are_sorted_by_display_name() {
        let mut state = SavedState::default();
        state.upsert_snippet(SavedSnippet {
            id: "b".to_string(),
            label: "Restart".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Ops".to_string(),
            pinned: false,
            command: "sudo systemctl restart app".to_string(),
        });
        state.upsert_snippet(SavedSnippet {
            id: "a".to_string(),
            label: "Deploy".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Ops".to_string(),
            pinned: true,
            command: "./deploy.sh".to_string(),
        });

        assert_eq!(state.snippets.len(), 2);
        assert_eq!(state.snippets[0].label, "Deploy");
        assert_eq!(state.snippets[1].label, "Restart");
        assert!(state.snippets[0].pinned);
    }

    #[test]
    fn snippets_can_be_removed() {
        let mut state = SavedState::default();
        state.upsert_snippet(SavedSnippet {
            id: "snippet-1".to_string(),
            label: "Tail logs".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: String::new(),
            pinned: false,
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
            favorite: false,
            group: "Data".to_string(),
            tags: "postgres, private".to_string(),
            host: "db.example.com".to_string(),
            port: "22".to_string(),
            username: "postgres".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Local,
            forward_local_port: "15432".to_string(),
            forward_remote_host: "127.0.0.1".to_string(),
            forward_remote_port: "5432".to_string(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };

        let profile = draft.to_profile("profile-2".to_string()).unwrap();
        assert_eq!(
            profile
                .port_forward_rules
                .first()
                .map(PortForwardRule::display_name),
            Some("127.0.0.1:15432 -> 127.0.0.1:5432".to_string())
        );
    }

    #[test]
    fn draft_profile_parses_dynamic_forward() {
        let draft = DraftProfile {
            label: "proxy".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            favorite: false,
            group: "Networking".to_string(),
            tags: "socks".to_string(),
            host: "bastion.example.com".to_string(),
            port: "22".to_string(),
            username: "ubuntu".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Dynamic,
            forward_local_port: "1080".to_string(),
            forward_remote_host: String::new(),
            forward_remote_port: String::new(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };

        let profile = draft.to_profile("profile-dynamic".to_string()).unwrap();
        assert_eq!(
            profile
                .port_forward_rules
                .first()
                .map(PortForwardRule::display_name),
            Some("SOCKS5 127.0.0.1:1080".to_string())
        );
    }

    #[test]
    fn draft_profile_parses_remote_forward() {
        let draft = DraftProfile {
            label: "reverse-proxy".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            favorite: false,
            group: "Networking".to_string(),
            tags: "reverse".to_string(),
            host: "bastion.example.com".to_string(),
            port: "22".to_string(),
            username: "ubuntu".to_string(),
            password: String::new(),
            key_path: "/tmp/id_ed25519".to_string(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            terminal_scrollback_rows: "10000".to_string(),
            saved_port_forward_rules: Vec::new(),
            forward_kind: PortForwardKind::Remote,
            forward_local_port: "3000".to_string(),
            forward_remote_host: "0.0.0.0".to_string(),
            forward_remote_port: "8443".to_string(),
            key_passphrase: String::new(),
            password_credential_id: None,
            auth_mode: AuthMode::PrivateKey,
            description: String::new(),
            color_tag: None,
            environment: String::new(),
        };

        let profile = draft.to_profile("profile-remote".to_string()).unwrap();
        assert_eq!(
            profile
                .port_forward_rules
                .first()
                .map(PortForwardRule::display_name),
            Some("Remote 0.0.0.0:8443 <- 127.0.0.1:3000".to_string())
        );
    }

    #[test]
    fn host_profile_normalizes_legacy_local_forward_into_rule_list() {
        let mut profile = HostProfile {
            id: "legacy-forward".to_string(),
            label: "Legacy".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            favorite: false,
            group: String::new(),
            tags: Vec::new(),
            host: "legacy.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth_mode: AuthMode::Password,
            key_path: String::new(),
            identity_id: None,
            jump_host_id: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            terminal_scrollback_rows: None,
            port_forward_rules: Vec::new(),
            local_forwards: Vec::new(),
            local_forward: Some(LocalPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port: 15432,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 5432,
            }),
            password_credential_id: None,
            source: ProfileSource::User,
            description: String::new(),
            color_tag: None,
            environment: Vec::new(),
        };

        profile.normalize();
        assert!(profile.local_forward.is_none());
        assert_eq!(profile.port_forward_rules.len(), 1);
        assert_eq!(
            profile.port_forward_rules[0].display_name(),
            "127.0.0.1:15432 -> 127.0.0.1:5432"
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
            pinned: false,
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

    #[test]
    fn command_history_is_deduplicated_and_recency_sorted() {
        let mut state = SavedState::default();
        state.record_command_history("ls -la");
        state.record_command_history("git status");
        state.record_command_history("ls -la");

        assert_eq!(
            state.command_history,
            vec!["git status".to_string(), "ls -la".to_string()]
        );
    }

    #[test]
    fn scoped_command_history_is_deduplicated_per_target() {
        let mut state = SavedState::default();
        state.record_command_history_for_scope("git status", "ssh:ops@example:22", "Ops");
        state.record_command_history_for_scope("git pull", "ssh:web@example:22", "Web");
        state.record_command_history_for_scope("git status", "ssh:ops@example:22", "Ops");

        assert_eq!(
            state.command_history,
            vec!["git pull".to_string(), "git status".to_string()]
        );
        assert_eq!(
            state.scoped_command_history,
            vec![
                SavedCommandHistoryEntry {
                    command: "git pull".to_string(),
                    scope_key: "ssh:web@example:22".to_string(),
                    scope_label: "Web".to_string(),
                },
                SavedCommandHistoryEntry {
                    command: "git status".to_string(),
                    scope_key: "ssh:ops@example:22".to_string(),
                    scope_label: "Ops".to_string(),
                },
            ]
        );
    }

    #[test]
    fn connect_request_builds_stable_history_scope_keys() {
        let ssh = ConnectRequest {
            session_id: 1,
            title: "Production".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: None,
            jump_host: None,
            startup_directory: Some("/srv/app".to_string()),
            startup_command: Some("npm run status".to_string()),
            start_in_files: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };
        assert_eq!(
            ssh.history_scope_key(),
            "ssh:deploy@prod.example.com:22".to_string()
        );
        assert_eq!(ssh.history_scope_label(), "Production".to_string());

        let local = ConnectRequest::local_shell(2);
        assert!(local.history_scope_key().starts_with("local:"));
        assert_eq!(local.history_scope_label(), local.endpoint_label());
    }

    #[test]
    fn settings_are_normalized_on_saved_state() {
        let mut state = SavedState {
            settings: AppSettings {
                theme_preset: ThemePreset::Daylight,
                terminal_font_size: 99,
                onboarding_dismissed: false,
                restore_workspaces_on_launch: true,
                session_log_limit: 9999,
                default_local_shell: LocalShellConfig {
                    program: String::new(),
                    args: Vec::new(),
                    cwd: Some(String::new()),
                },
                ..AppSettings::default()
            },
            ..SavedState::default()
        };
        state.ensure_settings();

        assert_eq!(state.settings.theme_preset, ThemePreset::Daylight);
        assert_eq!(state.settings.terminal_font_size, 18);
        assert!(!state.settings.onboarding_dismissed);
        assert!(state.settings.restore_workspaces_on_launch);
        assert_eq!(state.settings.session_log_limit, 1000);
        assert!(!state.settings.default_local_shell.program.trim().is_empty());
        assert_eq!(state.settings.default_local_shell.cwd, None);
        assert!(!state.settings.copy_on_select);
    }

    #[test]
    fn parse_environment_pairs_drops_blank_comment_and_invalid_keys() {
        let input = "
            AWS_PROFILE=prod
            # leading comment
            LOG_LEVEL = info
            BAD KEY=oops
            1NUMERIC=oops
            VALID_KEY=

            QUOTED=\"hello world\"
            SINGLE='one two'
            EMPTY=
        ";
        let pairs = super::parse_environment_pairs(input);
        let map: std::collections::HashMap<_, _> = pairs.iter().cloned().collect();
        assert_eq!(map.get("AWS_PROFILE").map(String::as_str), Some("prod"));
        assert_eq!(map.get("LOG_LEVEL").map(String::as_str), Some("info"));
        assert_eq!(map.get("VALID_KEY").map(String::as_str), Some(""));
        assert_eq!(map.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(map.get("QUOTED").map(String::as_str), Some("hello world"));
        assert_eq!(map.get("SINGLE").map(String::as_str), Some("one two"));
        assert!(!map.contains_key("BAD KEY"));
        assert!(!map.contains_key("1NUMERIC"));
    }

    #[test]
    fn format_environment_pairs_serializes_for_text_input() {
        let pairs = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "with space".to_string()),
        ];
        assert_eq!(
            super::format_environment_pairs(&pairs),
            "FOO=bar\nBAZ=with space"
        );
    }

    #[test]
    fn host_color_tag_round_trips_and_lists_full_palette() {
        let json = serde_json::to_string(&HostColorTag::Violet).unwrap();
        assert_eq!(json, "\"violet\"");
        let parsed: HostColorTag = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, HostColorTag::Violet);
        assert_eq!(HostColorTag::all().len(), 8);
        assert!(HostColorTag::all().contains(&HostColorTag::Red));
    }

    #[test]
    fn settings_round_trip_through_serde() {
        let mut original = AppSettings::default();
        original.copy_on_select = true;
        original.terminal_font_size = 16;
        original.theme_preset = ThemePreset::Daylight;
        original.auto_reconnect_attempts = 5;
        original.auto_reconnect_delay_secs = 30;

        let json = serde_json::to_string(&original).expect("serialize settings");
        let parsed: AppSettings = serde_json::from_str(&json).expect("deserialize settings");

        assert!(parsed.copy_on_select);
        assert_eq!(parsed.terminal_font_size, 16);
        assert_eq!(parsed.theme_preset, ThemePreset::Daylight);
        assert_eq!(parsed.auto_reconnect_attempts, 5);
        assert_eq!(parsed.auto_reconnect_delay_secs, 30);

        let legacy = "{}";
        let from_legacy: AppSettings =
            serde_json::from_str(legacy).expect("deserialize legacy settings");
        assert!(!from_legacy.copy_on_select);
        assert_eq!(from_legacy.auto_reconnect_attempts, 3);
        assert_eq!(from_legacy.auto_reconnect_delay_secs, 5);
        assert!(from_legacy.confirm_multiline_paste);
    }

    #[test]
    fn settings_normalize_clamps_auto_reconnect_bounds() {
        let mut settings = AppSettings::default();
        settings.auto_reconnect_attempts = 99;
        settings.auto_reconnect_delay_secs = 0;
        settings.normalize();
        assert_eq!(settings.auto_reconnect_attempts, 10);
        assert_eq!(settings.auto_reconnect_delay_secs, 1);

        settings.auto_reconnect_delay_secs = 200;
        settings.normalize();
        assert_eq!(settings.auto_reconnect_delay_secs, 60);
    }

    #[test]
    fn session_logs_are_trimmed_to_configured_limit() {
        let mut state = SavedState {
            settings: AppSettings {
                session_log_limit: 50,
                ..AppSettings::default()
            },
            ..SavedState::default()
        };

        for index in 0..75 {
            state.record_session_log(SessionLogEntry {
                id: format!("log-{index}"),
                host: "example.com".to_string(),
                port: 22,
                username: "deploy".to_string(),
                title: format!("session-{index}"),
                status: SessionLogStatus::Connecting,
                started_at: index,
                ended_at: None,
                error_message: None,
            });
        }

        assert_eq!(state.session_logs.len(), 50);
        assert_eq!(state.session_logs.first().unwrap().id, "log-25");
        assert_eq!(state.session_logs.last().unwrap().id, "log-74");
    }
}
