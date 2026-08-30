use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use termirust_domain::{
    GroupDestination, GroupId, HostedSession, HostedSessionId, HostedSessionState, OutputSequence,
    PositionKey, ProjectId, Revision, SessionLaunchRoute, SessionOrigin, SessionTitle,
    SshAccessPolicy, TitleSource,
};
use termirust_protocol::{
    MobileDevicePairingError, MobileDevicePairingRequest, MobileDeviceRecord, MobileDeviceVaultKey,
};

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
    LocalAgent,
}

impl AuthMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::PrivateKey => "Private Key",
            Self::LocalAgent => "SSH Agent",
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
    Generated,
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
            Self::Ocean => "Ocean Dark",
            Self::Daylight => "Daylight Light",
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
    pub certificate_path: Option<String>,
    #[serde(default)]
    pub identity_agent: Option<String>,
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
    pub persistent_session: bool,
    #[serde(default)]
    pub persistent_session_name: Option<String>,
    #[serde(default)]
    pub persistent_session_detach_others: bool,
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

pub fn default_persistent_session_name_from_id(id: &str) -> String {
    let slug = id.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    format!("tr-{slug}")
}

pub fn default_persistent_session_name_for_endpoint(
    username: &str,
    host: &str,
    port: u16,
) -> String {
    let raw = format!("{username}-{host}-{port}");
    let slug = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "tr-session".to_string()
    } else {
        format!("tr-{slug}")
    }
}

impl HostProfile {
    pub fn ssh_access_policy(&self) -> SshAccessPolicy {
        match (self.auth_mode, self.certificate_path.is_some()) {
            (AuthMode::Password, _) => SshAccessPolicy::legacy_password(),
            (AuthMode::PrivateKey, false) => SshAccessPolicy::legacy_private_key(),
            (AuthMode::PrivateKey, true) => SshAccessPolicy {
                authentication: termirust_domain::SshAuthenticationKind::OpenSshCertificate,
                certificate_signer: Some(termirust_domain::SshCertificateSigner::PrivateKey),
                agent_forwarding: termirust_domain::SshAgentForwardingPolicy::Disabled,
            },
            (AuthMode::LocalAgent, _) => SshAccessPolicy {
                authentication: termirust_domain::SshAuthenticationKind::LocalAgent,
                certificate_signer: None,
                agent_forwarding: termirust_domain::SshAgentForwardingPolicy::Disabled,
            },
        }
    }

    pub fn saved_auth_config(&self) -> Result<AuthConfig> {
        let policy = self.ssh_access_policy();
        match self.auth_mode {
            AuthMode::Password => self
                .password_credential_id
                .clone()
                .map(|credential_id| AuthConfig::PasswordRef { credential_id })
                .with_context(|| {
                    format!(
                        "{} needs a saved password in the system credential store",
                        self.display_name()
                    )
                }),
            AuthMode::PrivateKey => {
                if self.key_path.trim().is_empty() {
                    bail!("{} needs a private key file", self.display_name());
                }
                match (policy.authentication, self.certificate_path.clone()) {
                    (
                        termirust_domain::SshAuthenticationKind::OpenSshCertificate,
                        Some(certificate_path),
                    ) => Ok(AuthConfig::OpenSshCertificate {
                        key_path: self.key_path.clone(),
                        certificate_path,
                        passphrase: None,
                    }),
                    _ => Ok(AuthConfig::PrivateKey {
                        key_path: self.key_path.clone(),
                        passphrase: None,
                    }),
                }
            }
            AuthMode::LocalAgent => Ok(AuthConfig::LocalAgent {
                socket_path: self.identity_agent.clone(),
                forward_agent: false,
            }),
        }
    }

    pub fn default_persistent_session_name(&self) -> String {
        default_persistent_session_name_from_id(&self.id)
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
        self.certificate_path = self
            .certificate_path
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.identity_agent = self
            .identity_agent
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
    #[serde(default = "default_diagnostics_enabled")]
    pub diagnostics_enabled: bool,
    #[serde(default = "default_diagnostics_max_file_mib")]
    pub diagnostics_max_file_mib: u8,
    #[serde(default = "default_diagnostics_retention_days")]
    pub diagnostics_retention_days: u8,
    #[serde(default)]
    pub sync_folder_path: Option<String>,
    #[serde(default)]
    pub sync_last_pushed_at: Option<u64>,
    #[serde(default)]
    pub sync_last_pulled_at: Option<u64>,
    #[serde(default)]
    pub mobile_device_id: Option<String>,
    #[serde(default)]
    pub mobile_devices: Vec<MobileDeviceRecord>,
    #[serde(default)]
    pub mobile_device_keys: Vec<MobileDeviceVaultKey>,
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

fn default_diagnostics_enabled() -> bool {
    true
}

fn default_diagnostics_max_file_mib() -> u8 {
    10
}

fn default_diagnostics_retention_days() -> u8 {
    14
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
            diagnostics_enabled: default_diagnostics_enabled(),
            diagnostics_max_file_mib: default_diagnostics_max_file_mib(),
            diagnostics_retention_days: default_diagnostics_retention_days(),
            sync_folder_path: None,
            sync_last_pushed_at: None,
            sync_last_pulled_at: None,
            mobile_device_id: None,
            mobile_devices: Vec::new(),
            mobile_device_keys: Vec::new(),
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
        self.diagnostics_max_file_mib = self.diagnostics_max_file_mib.clamp(1, 10);
        self.diagnostics_retention_days = self.diagnostics_retention_days.clamp(1, 14);
        self.sync_folder_path = self
            .sync_folder_path
            .take()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty());
        self.mobile_device_id = self
            .mobile_device_id
            .take()
            .map(|device_id| device_id.trim().to_string())
            .filter(|device_id| !device_id.is_empty());
        self.mobile_devices
            .retain(|device| !device.device_id.trim().is_empty());
        self.mobile_device_keys
            .retain(|key| !key.device_id.trim().is_empty() && !key.key_id.trim().is_empty());
    }

    pub fn apply_mobile_pairing_request(
        &mut self,
        request: MobileDevicePairingRequest,
        now_millis: u128,
    ) -> std::result::Result<(), MobileDevicePairingError> {
        request.validate()?;
        if self.mobile_devices.iter().any(|device| {
            device.device_id == request.device_id && device.revoked_at_millis.is_some()
        }) {
            return Err(MobileDevicePairingError::DeviceRevoked(request.device_id));
        }

        if let Some(device) = self
            .mobile_devices
            .iter_mut()
            .find(|device| device.device_id == request.device_id)
        {
            device.label = request.label;
            device.platform = Some(request.platform);
            device.public_key = request.public_key;
            device.paired_at_millis = device.paired_at_millis.or(Some(now_millis));
            device.last_seen_at_millis = Some(now_millis);
        } else {
            self.mobile_devices.push(MobileDeviceRecord {
                device_id: request.device_id,
                label: request.label,
                platform: Some(request.platform),
                public_key: request.public_key,
                paired_at_millis: Some(now_millis),
                last_seen_at_millis: Some(now_millis),
                revoked_at_millis: None,
            });
        }
        Ok(())
    }

    pub fn revoke_mobile_device(
        &mut self,
        device_id: &str,
        revoked_at_millis: u128,
    ) -> std::result::Result<(), MobileDevicePairingError> {
        let device_id = device_id.trim();
        if device_id.is_empty() {
            return Err(MobileDevicePairingError::MissingDeviceId);
        }
        let Some(device) = self
            .mobile_devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
        else {
            return Err(MobileDevicePairingError::UnknownDevice(
                device_id.to_string(),
            ));
        };
        device.revoked_at_millis = Some(revoked_at_millis);
        device.last_seen_at_millis = Some(revoked_at_millis);
        for key in self
            .mobile_device_keys
            .iter_mut()
            .filter(|key| key.device_id == device_id && key.revoked_at_millis.is_none())
        {
            key.revoked_at_millis = Some(revoked_at_millis);
        }
        Ok(())
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

/// Persisted window frame (global coordinates) plus the display it was on, so
/// the app reopens where the user last left it — including the right monitor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub display_id: Option<u32>,
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
    #[serde(default)]
    pub window_bounds: Option<SavedWindowBounds>,
    #[serde(default)]
    pub managed_agent_worktrees: Vec<SavedManagedWorktree>,
    #[serde(default)]
    pub app_attached_sessions: Vec<SavedAppAttachedSession>,
}

const MAX_APP_ATTACHED_SESSION_RECORDS: usize = 100_000;
const MAX_APP_ATTACHED_SESSION_LABEL_CHARS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedAppAttachedSession {
    pub id: HostedSessionId,
    pub route: SessionLaunchRoute,
    pub origin: SessionOrigin,
    pub state: HostedSessionState,
    pub project_label: String,
    pub preset_label: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub title_source: TitleSource,
    #[serde(default)]
    pub activity: termirust_domain::ActivityAggregate,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub read_through_sequence: u64,
    #[serde(default)]
    pub unread_sequence: Option<u64>,
    #[serde(default)]
    pub archived_at: Option<u64>,
    #[serde(default)]
    pub revision: Revision,
    #[serde(default)]
    pub durable_host: Option<SavedDurableHost>,
    #[serde(default)]
    pub group_id: Option<GroupId>,
    #[serde(default = "default_session_organization_position")]
    pub position: PositionKey,
    pub started_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedDurableHost {
    pub runtime_root: String,
    pub session_dir: String,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub last_sequence: u64,
    #[serde(default)]
    pub durable_sequence: u64,
    #[serde(default)]
    pub runtime_recognition: Option<termirust_domain::RuntimeRecognition>,
    #[serde(skip)]
    pub conversation_handle: Option<termirust_domain::ConversationHandle>,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub permission_policy: termirust_domain::PermissionPolicy,
    #[serde(default)]
    pub continuity_source_id: Option<HostedSessionId>,
}

fn default_session_organization_position() -> PositionKey {
    PositionKey::FIRST
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedSessionPlacement {
    pub id: HostedSessionId,
    pub group_id: Option<GroupId>,
    pub position: PositionKey,
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
        self.normalize_app_attached_sessions();
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
                if existing.source != IdentitySource::Imported {
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
    pub certificate_path: String,
    pub identity_agent: String,
    pub identity_id: Option<String>,
    pub jump_host_id: Option<String>,
    pub startup_directory: String,
    pub startup_command: String,
    pub start_in_files: bool,
    pub persistent_session: bool,
    pub persistent_session_name: String,
    pub persistent_session_detach_others: bool,
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
            certificate_path: profile.certificate_path.clone().unwrap_or_default(),
            identity_agent: profile.identity_agent.clone().unwrap_or_default(),
            identity_id: profile.identity_id.clone(),
            jump_host_id: profile.jump_host_id.clone(),
            startup_directory: profile.startup_directory.clone().unwrap_or_default(),
            startup_command: profile.startup_command.clone().unwrap_or_default(),
            start_in_files: profile.start_in_files,
            persistent_session: profile.persistent_session,
            persistent_session_name: if profile.persistent_session {
                profile
                    .persistent_session_name
                    .clone()
                    .unwrap_or_else(|| profile.default_persistent_session_name())
            } else {
                profile.persistent_session_name.clone().unwrap_or_default()
            },
            persistent_session_detach_others: profile.persistent_session_detach_others,
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
        let certificate_path = self.certificate_path.trim();
        let identity_agent = self.identity_agent.trim();

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
            certificate_path: if self.auth_mode == AuthMode::PrivateKey {
                non_empty(certificate_path)
            } else {
                None
            },
            identity_agent: if self.auth_mode == AuthMode::LocalAgent {
                non_empty(identity_agent)
            } else {
                None
            },
            identity_id: if self.auth_mode == AuthMode::PrivateKey {
                self.identity_id.clone()
            } else {
                None
            },
            jump_host_id: self.jump_host_id.clone(),
            startup_directory: non_empty(self.startup_directory.trim()),
            startup_command: non_empty(self.startup_command.trim()),
            start_in_files: self.start_in_files,
            persistent_session: self.persistent_session,
            persistent_session_name: non_empty(self.persistent_session_name.trim()),
            persistent_session_detach_others: self.persistent_session_detach_others,
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
            AuthMode::PrivateKey => match profile.certificate_path.clone() {
                Some(certificate_path) => AuthConfig::OpenSshCertificate {
                    key_path: profile.key_path.clone(),
                    certificate_path,
                    passphrase: non_empty(self.key_passphrase.trim()),
                },
                None => AuthConfig::PrivateKey {
                    key_path: profile.key_path.clone(),
                    passphrase: non_empty(self.key_passphrase.trim()),
                },
            },
            AuthMode::LocalAgent => AuthConfig::LocalAgent {
                socket_path: profile.identity_agent.clone(),
                forward_agent: false,
            },
        };
        let persistent_session_name = profile.persistent_session_name.clone().or_else(|| {
            profile.persistent_session.then(|| {
                default_persistent_session_name_for_endpoint(
                    &profile.username,
                    &profile.host,
                    profile.port,
                )
            })
        });

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
            persistent_session: profile.persistent_session,
            persistent_session_name,
            persistent_session_detach_others: profile.persistent_session_detach_others,
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
    OpenSshCertificate {
        key_path: String,
        certificate_path: String,
        passphrase: Option<String>,
    },
    LocalAgent {
        socket_path: Option<String>,
        forward_agent: bool,
    },
}

impl AuthConfig {
    pub fn enable_one_shot_agent_forwarding(&mut self) -> Result<()> {
        match self {
            Self::LocalAgent { forward_agent, .. } => {
                *forward_agent = true;
                Ok(())
            }
            _ => bail!("Agent forwarding requires SSH-agent authentication"),
        }
    }

    pub fn disable_agent_forwarding(&mut self) {
        if let Self::LocalAgent { forward_agent, .. } = self {
            *forward_agent = false;
        }
    }

    pub fn forwarded_agent_socket(&self) -> Option<Option<&str>> {
        match self {
            Self::LocalAgent {
                socket_path,
                forward_agent: true,
            } => Some(socket_path.as_deref()),
            _ => None,
        }
    }

    pub fn to_restorable(&self) -> Option<RestorableAuth> {
        match self {
            Self::Password { .. } => None,
            Self::PasswordRef { credential_id } => Some(RestorableAuth::PasswordKeychain {
                credential_id: credential_id.clone(),
            }),
            Self::PrivateKey { key_path, .. } => Some(RestorableAuth::PrivateKey {
                key_path: key_path.clone(),
            }),
            Self::OpenSshCertificate {
                key_path,
                certificate_path,
                ..
            } => Some(RestorableAuth::OpenSshCertificate {
                key_path: key_path.clone(),
                certificate_path: certificate_path.clone(),
            }),
            Self::LocalAgent { socket_path, .. } => Some(RestorableAuth::LocalAgent {
                socket_path: socket_path.clone(),
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
    pub persistent_session: bool,
    pub persistent_session_name: Option<String>,
    pub persistent_session_detach_others: bool,
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
                persistent_session: self.persistent_session,
                persistent_session_name: self.persistent_session_name.clone(),
                persistent_session_detach_others: self.persistent_session_detach_others,
                terminal_scrollback_rows: Some(self.terminal_scrollback_rows as u32),
                port_forward_rules: self.port_forward_rules.clone(),
                local_forwards: Vec::new(),
                local_forward: None,
                local_shell: None,
                durable_session_id: None,
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
                persistent_session: self.persistent_session,
                persistent_session_name: self.persistent_session_name.clone(),
                persistent_session_detach_others: self.persistent_session_detach_others,
                terminal_scrollback_rows: None,
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
                local_forward: None,
                local_shell: self.local_shell.clone(),
                durable_session_id: None,
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: Some(shell),
            environment: Vec::new(),
        }
    }

    pub fn persistent_local_shell_with_config(
        session_id: u64,
        shell: LocalShellConfig,
        session_name: String,
        detach_others: bool,
    ) -> Self {
        let mut request = Self::local_shell_with_config(session_id, shell);
        request.title = "Persistent Local Terminal".to_string();
        request.persistent_session = true;
        request.persistent_session_name = Some(session_name);
        request.persistent_session_detach_others = detach_others;
        request
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
    PasswordKeychain {
        credential_id: String,
    },
    PrivateKey {
        key_path: String,
    },
    OpenSshCertificate {
        key_path: String,
        certificate_path: String,
    },
    LocalAgent {
        #[serde(default)]
        socket_path: Option<String>,
    },
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
    pub persistent_session: bool,
    #[serde(default)]
    pub persistent_session_name: Option<String>,
    #[serde(default)]
    pub persistent_session_detach_others: bool,
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
    #[serde(default)]
    pub durable_session_id: Option<HostedSessionId>,
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
                    RestorableAuth::OpenSshCertificate {
                        key_path,
                        certificate_path,
                    } => AuthConfig::OpenSshCertificate {
                        key_path: key_path.clone(),
                        certificate_path: certificate_path.clone(),
                        passphrase: None,
                    },
                    RestorableAuth::LocalAgent { socket_path } => AuthConfig::LocalAgent {
                        socket_path: socket_path.clone(),
                        forward_agent: false,
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
                    persistent_session: self.persistent_session,
                    persistent_session_name: self.persistent_session_name.clone(),
                    persistent_session_detach_others: self.persistent_session_detach_others,
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
                persistent_session: self.persistent_session,
                persistent_session_name: self.persistent_session_name.clone(),
                persistent_session_detach_others: self.persistent_session_detach_others,
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
            RestorableAuth::OpenSshCertificate {
                key_path,
                certificate_path,
            } => AuthConfig::OpenSshCertificate {
                key_path: key_path.clone(),
                certificate_path: certificate_path.clone(),
                passphrase: None,
            },
            RestorableAuth::LocalAgent { socket_path } => AuthConfig::LocalAgent {
                socket_path: socket_path.clone(),
                forward_agent: false,
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

/// Persisted form of a workspace's recursive split layout — a binary tree of
/// pane indices into `SavedWorkspace::panes`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SavedSplitNode {
    Leaf(usize),
    Split {
        axis: SplitAxis,
        ratio: f32,
        a: Box<SavedSplitNode>,
        b: Box<SavedSplitNode>,
    },
}

pub const CANVAS_SCHEMA_VERSION: u32 = 1;
pub const CANVAS_MIN_ZOOM: f32 = 0.35;
pub const CANVAS_MAX_ZOOM: f32 = 2.0;
pub const CANVAS_DEFAULT_NODE_WIDTH: f32 = 720.0;
pub const CANVAS_DEFAULT_NODE_HEIGHT: f32 = 460.0;
pub const CANVAS_MIN_NODE_WIDTH: f32 = 320.0;
pub const CANVAS_MIN_TERMINAL_NODE_WIDTH: f32 = 640.0;
pub const CANVAS_MIN_NODE_HEIGHT: f32 = 220.0;

fn current_canvas_schema_version() -> u32 {
    CANVAS_SCHEMA_VERSION
}

fn truncate_string_at_utf8_boundary(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn default_true() -> bool {
    true
}

fn default_context_max_bytes() -> usize {
    8 * 1024
}

fn default_context_max_lines() -> usize {
    80
}

fn default_context_max_messages() -> usize {
    20
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLayoutMode {
    #[default]
    Split,
    Canvas,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CanvasNodeId(pub String);

impl CanvasNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CanvasEdgeId(pub String);

impl CanvasEdgeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasViewport {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl Default for SavedCanvasViewport {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    #[default]
    Codex,
    ClaudeCode,
    Gemini,
    CustomCli,
    GroqApi,
}

impl AgentProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Gemini => "Gemini CLI",
            Self::CustomCli => "Custom CLI",
            Self::GroqApi => "Groq API",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackendKind {
    #[default]
    InteractivePty,
    Structured,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentLocation {
    #[default]
    Local,
    SavedHost {
        profile_id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionPolicy {
    #[default]
    ProviderDefault,
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedWorktreePolicy {
    #[default]
    Isolated,
    SharedDirectory,
    ReadOnly,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAgentDefinition {
    #[serde(default)]
    pub provider: AgentProvider,
    #[serde(default)]
    pub backend: AgentBackendKind,
    #[serde(default)]
    pub location: AgentLocation,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub executable_override: Option<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub permission_policy: AgentPermissionPolicy,
    #[serde(default)]
    pub worktree: SavedWorktreePolicy,
    #[serde(default)]
    pub managed_worktree: Option<SavedManagedWorktree>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedManagedWorktree {
    pub repository_root: String,
    pub path: String,
    pub branch: String,
    pub base_revision: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub disposition: SavedManagedWorktreeDisposition,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedManagedWorktreeDisposition {
    #[default]
    Active,
    Complete,
    Kept,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedCanvasNodeKind {
    Terminal {
        pane_index: usize,
    },
    Agent {
        #[serde(default)]
        pane_index: Option<usize>,
        #[serde(default)]
        definition: SavedAgentDefinition,
    },
    Note {
        #[serde(default)]
        text: String,
        #[serde(default)]
        color: CanvasNoteColor,
    },
    Group {
        #[serde(default)]
        member_ids: Vec<CanvasNodeId>,
    },
}

impl SavedCanvasNodeKind {
    fn is_executable(&self) -> bool {
        matches!(self, Self::Terminal { .. } | Self::Agent { .. })
    }

    fn can_source_context(&self) -> bool {
        !matches!(self, Self::Group { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasNoteColor {
    #[default]
    Yellow,
    Blue,
    Green,
    Rose,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasNode {
    pub id: CanvasNodeId,
    pub kind: SavedCanvasNodeKind,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasEdgeKind {
    #[default]
    Context,
    Dependency,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedContextPolicy {
    #[serde(default = "default_context_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_context_max_lines")]
    pub max_terminal_lines: usize,
    #[serde(default = "default_context_max_messages")]
    pub max_agent_messages: usize,
    #[serde(default = "default_true")]
    pub redact_secrets: bool,
}

impl Default for SavedContextPolicy {
    fn default() -> Self {
        Self {
            max_bytes: default_context_max_bytes(),
            max_terminal_lines: default_context_max_lines(),
            max_agent_messages: default_context_max_messages(),
            redact_secrets: true,
        }
    }
}

impl SavedContextPolicy {
    fn normalize(&mut self) -> bool {
        let before = self.clone();
        self.max_bytes = self.max_bytes.clamp(256, 64 * 1024);
        self.max_terminal_lines = self.max_terminal_lines.clamp(1, 500);
        self.max_agent_messages = self.max_agent_messages.clamp(1, 100);
        *self != before
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasEdge {
    pub id: CanvasEdgeId,
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    #[serde(default)]
    pub kind: CanvasEdgeKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub context_policy: Option<SavedContextPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasState {
    #[serde(default = "current_canvas_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub viewport: SavedCanvasViewport,
    #[serde(default)]
    pub nodes: Vec<SavedCanvasNode>,
    #[serde(default)]
    pub edges: Vec<SavedCanvasEdge>,
}

impl Default for SavedCanvasState {
    fn default() -> Self {
        Self {
            schema_version: CANVAS_SCHEMA_VERSION,
            viewport: SavedCanvasViewport::default(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CanvasRepairReport {
    pub unsupported_schema: bool,
    pub viewport_repairs: usize,
    pub node_repairs: usize,
    pub removed_nodes: usize,
    pub edge_repairs: usize,
    pub removed_edges: usize,
}

impl CanvasRepairReport {
    pub fn changed(self) -> bool {
        self.viewport_repairs > 0
            || self.node_repairs > 0
            || self.removed_nodes > 0
            || self.edge_repairs > 0
            || self.removed_edges > 0
    }
}

impl SavedCanvasState {
    pub fn normalize(&mut self, pane_count: usize) -> CanvasRepairReport {
        let mut report = CanvasRepairReport::default();
        if self.schema_version > CANVAS_SCHEMA_VERSION {
            report.unsupported_schema = true;
            return report;
        }
        if self.schema_version == 0 {
            self.schema_version = CANVAS_SCHEMA_VERSION;
        }

        if !self.viewport.pan_x.is_finite() {
            self.viewport.pan_x = 0.0;
            report.viewport_repairs += 1;
        }
        if !self.viewport.pan_y.is_finite() {
            self.viewport.pan_y = 0.0;
            report.viewport_repairs += 1;
        }
        let repaired_zoom = if self.viewport.zoom.is_finite() {
            self.viewport.zoom.clamp(CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM)
        } else {
            1.0
        };
        if self.viewport.zoom != repaired_zoom {
            self.viewport.zoom = repaired_zoom;
            report.viewport_repairs += 1;
        }

        let original_node_count = self.nodes.len();
        self.nodes.retain_mut(|node| {
            let (keep, terminal_backed) = match &mut node.kind {
                SavedCanvasNodeKind::Terminal { pane_index } => (*pane_index < pane_count, true),
                SavedCanvasNodeKind::Agent { pane_index, .. } => {
                    if pane_index.is_some_and(|index| index >= pane_count) {
                        *pane_index = None;
                        report.node_repairs += 1;
                    }
                    (true, pane_index.is_some())
                }
                SavedCanvasNodeKind::Note { text, .. } => {
                    if text.len() > 64 * 1024 {
                        truncate_string_at_utf8_boundary(text, 64 * 1024);
                        report.node_repairs += 1;
                    }
                    (true, false)
                }
                SavedCanvasNodeKind::Group { .. } => (true, false),
            };
            if !keep {
                return false;
            }

            let ordinal = report.node_repairs + 1;
            if !node.x.is_finite() {
                node.x = ((ordinal - 1) % 4) as f32 * 760.0;
                report.node_repairs += 1;
            }
            if !node.y.is_finite() {
                node.y = ((ordinal - 1) / 4) as f32 * 500.0;
                report.node_repairs += 1;
            }
            let min_width = if terminal_backed {
                CANVAS_MIN_TERMINAL_NODE_WIDTH
            } else {
                CANVAS_MIN_NODE_WIDTH
            };
            let width = if node.width.is_finite() {
                node.width.max(min_width)
            } else {
                CANVAS_DEFAULT_NODE_WIDTH
            };
            let height = if node.height.is_finite() {
                node.height.max(CANVAS_MIN_NODE_HEIGHT)
            } else {
                CANVAS_DEFAULT_NODE_HEIGHT
            };
            if node.width != width {
                node.width = width;
                report.node_repairs += 1;
            }
            if node.height != height {
                node.height = height;
                report.node_repairs += 1;
            }
            true
        });
        report.removed_nodes = original_node_count - self.nodes.len();

        let mut node_ids = HashSet::new();
        for (index, node) in self.nodes.iter_mut().enumerate() {
            let original = node.id.0.trim().to_string();
            let mut candidate = if original.is_empty() {
                format!("canvas-node-{}", index + 1)
            } else {
                original
            };
            let base = candidate.clone();
            let mut suffix = 2;
            while node_ids.contains(&CanvasNodeId::new(candidate.clone())) {
                candidate = format!("{base}-{suffix}");
                suffix += 1;
            }
            if node.id.0 != candidate {
                node.id = CanvasNodeId::new(candidate.clone());
                report.node_repairs += 1;
            }
            node_ids.insert(CanvasNodeId::new(candidate));
        }

        let group_ids = self
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, SavedCanvasNodeKind::Group { .. }))
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let member_candidates = node_ids
            .difference(&group_ids)
            .cloned()
            .collect::<HashSet<_>>();
        let mut assigned_members = HashSet::new();
        for node in &mut self.nodes {
            let SavedCanvasNodeKind::Group { member_ids } = &mut node.kind else {
                continue;
            };
            let before = member_ids.clone();
            member_ids.retain(|member_id| {
                member_candidates.contains(member_id) && assigned_members.insert(member_id.clone())
            });
            if *member_ids != before {
                report.node_repairs += 1;
            }
        }

        let node_kinds = self
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.kind.clone()))
            .collect::<HashMap<_, _>>();

        let mut edge_ids = HashSet::new();
        let mut semantic_edges = HashSet::new();
        let mut dependency_adjacency: HashMap<CanvasNodeId, Vec<CanvasNodeId>> = HashMap::new();
        let original_edge_count = self.edges.len();
        self.edges.retain_mut(|edge| {
            if !node_ids.contains(&edge.source)
                || !node_ids.contains(&edge.target)
                || edge.source == edge.target
            {
                return false;
            }

            let Some(source_kind) = node_kinds.get(&edge.source) else {
                return false;
            };
            let Some(target_kind) = node_kinds.get(&edge.target) else {
                return false;
            };
            let valid_endpoints = match edge.kind {
                CanvasEdgeKind::Context => {
                    source_kind.can_source_context() && target_kind.is_executable()
                }
                CanvasEdgeKind::Dependency => {
                    source_kind.is_executable() && target_kind.is_executable()
                }
            };
            if !valid_endpoints {
                return false;
            }

            let semantic_key = (edge.source.clone(), edge.target.clone(), edge.kind);
            if !semantic_edges.insert(semantic_key) {
                return false;
            }

            if edge.kind == CanvasEdgeKind::Dependency
                && graph_has_path(&dependency_adjacency, &edge.target, &edge.source)
            {
                return false;
            }
            if edge.kind == CanvasEdgeKind::Dependency {
                dependency_adjacency
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());
            }

            let original = edge.id.0.trim().to_string();
            let mut candidate = if original.is_empty() {
                format!("canvas-edge-{}", edge_ids.len() + 1)
            } else {
                original
            };
            let base = candidate.clone();
            let mut suffix = 2;
            while edge_ids.contains(&CanvasEdgeId::new(candidate.clone())) {
                candidate = format!("{base}-{suffix}");
                suffix += 1;
            }
            if edge.id.0 != candidate {
                edge.id = CanvasEdgeId::new(candidate.clone());
                report.edge_repairs += 1;
            }
            edge_ids.insert(CanvasEdgeId::new(candidate));

            if edge.kind == CanvasEdgeKind::Context {
                let policy = edge
                    .context_policy
                    .get_or_insert_with(SavedContextPolicy::default);
                if policy.normalize() {
                    report.edge_repairs += 1;
                }
            } else if edge.context_policy.take().is_some() {
                report.edge_repairs += 1;
            }
            true
        });
        report.removed_edges = original_edge_count - self.edges.len();
        report
    }
}

fn graph_has_path(
    adjacency: &HashMap<CanvasNodeId, Vec<CanvasNodeId>>,
    start: &CanvasNodeId,
    target: &CanvasNodeId,
) -> bool {
    let mut pending = vec![start.clone()];
    let mut visited = HashSet::new();
    while let Some(node) = pending.pop() {
        if &node == target {
            return true;
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            pending.extend(next.iter().cloned());
        }
    }
    false
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub title: String,
    #[serde(default)]
    pub project_directory: Option<String>,
    #[serde(default)]
    pub layout_mode: WorkspaceLayoutMode,
    #[serde(default)]
    pub layout: Option<SavedSplitNode>,
    #[serde(default)]
    pub canvas: Option<SavedCanvasState>,
    #[serde(default)]
    pub active_pane_index: usize,
    #[serde(default)]
    pub panes: Vec<RestorableConnection>,
}

impl SavedWorkspace {
    pub fn normalize(&mut self) {
        self.project_directory = self
            .project_directory
            .take()
            .map(|directory| directory.trim().to_string())
            .filter(|directory| !directory.is_empty());
        if self.panes.is_empty() {
            self.active_pane_index = 0;
        } else {
            self.active_pane_index = self.active_pane_index.min(self.panes.len() - 1);
        }
        if let Some(canvas) = self.canvas.as_mut() {
            let report = canvas.normalize(self.panes.len());
            if report.unsupported_schema {
                self.layout_mode = WorkspaceLayoutMode::Split;
            }
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }
}

impl SavedState {
    fn normalize_app_attached_sessions(&mut self) {
        for session in &mut self.app_attached_sessions {
            session.project_label = session
                .project_label
                .chars()
                .take(MAX_APP_ATTACHED_SESSION_LABEL_CHARS)
                .collect();
            session.preset_label = session
                .preset_label
                .chars()
                .take(MAX_APP_ATTACHED_SESSION_LABEL_CHARS)
                .collect();
            if session.title.trim().is_empty() {
                session.title = session.preset_label.clone();
            }
            session.title = session
                .title
                .chars()
                .take(termirust_domain::MAX_SESSION_TITLE_SCALARS)
                .collect();
        }
        if self.app_attached_sessions.len() > MAX_APP_ATTACHED_SESSION_RECORDS {
            let drain = self.app_attached_sessions.len() - MAX_APP_ATTACHED_SESSION_RECORDS;
            self.app_attached_sessions.drain(..drain);
        }
    }

    pub fn upsert_app_attached_session(&mut self, session: SavedAppAttachedSession) {
        if let Some(existing) = self
            .app_attached_sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            *existing = session;
        } else {
            self.app_attached_sessions.push(session);
        }
        self.normalize_app_attached_sessions();
    }

    pub fn next_app_attached_session_position(
        &self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
    ) -> PositionKey {
        self.app_attached_sessions
            .iter()
            .filter(|session| {
                session.origin.project_id == project_id && session.group_id == group_id
            })
            .map(|session| session.position)
            .max()
            .and_then(|position| position.after().ok())
            .unwrap_or(PositionKey::FIRST)
    }

    pub fn move_app_attached_session(
        &mut self,
        id: HostedSessionId,
        destination: GroupDestination,
        before: Option<HostedSessionId>,
    ) -> Option<Vec<SavedSessionPlacement>> {
        let project_id = self
            .app_attached_sessions
            .iter()
            .find(|session| session.id == id)?
            .origin
            .project_id;
        let destination_group = destination.group_id();
        if before.is_some_and(|before_id| {
            self.app_attached_sessions.iter().all(|session| {
                session.id != before_id
                    || session.origin.project_id != project_id
                    || session.group_id != destination_group
            })
        }) {
            return None;
        }
        let inverse = self.project_session_placements(project_id);
        let moving_index = self
            .app_attached_sessions
            .iter()
            .position(|session| session.id == id)?;
        self.app_attached_sessions[moving_index].group_id = destination_group;

        let mut destination_ids = self
            .app_attached_sessions
            .iter()
            .filter(|session| {
                session.origin.project_id == project_id && session.group_id == destination_group
            })
            .map(|session| (session.position, session.id))
            .collect::<Vec<_>>();
        destination_ids.sort_by_key(|(position, session_id)| (*position, *session_id));
        destination_ids.retain(|(_, session_id)| *session_id != id);
        let insert_at = before
            .and_then(|before_id| {
                destination_ids
                    .iter()
                    .position(|(_, session_id)| *session_id == before_id)
            })
            .unwrap_or(destination_ids.len());
        destination_ids.insert(insert_at, (PositionKey::FIRST, id));
        for (index, (_, session_id)) in destination_ids.into_iter().enumerate() {
            let position = PositionKey::rebalanced(index).ok()?;
            let session = self
                .app_attached_sessions
                .iter_mut()
                .find(|session| session.id == session_id)?;
            session.position = position;
        }
        Some(inverse)
    }

    pub fn relocate_group_sessions(
        &mut self,
        project_id: ProjectId,
        group_id: GroupId,
        destination: GroupDestination,
    ) -> Vec<SavedSessionPlacement> {
        let inverse = self.project_session_placements(project_id);
        let ids = self
            .app_attached_sessions
            .iter()
            .filter(|session| {
                session.origin.project_id == project_id && session.group_id == Some(group_id)
            })
            .map(|session| session.id)
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.move_app_attached_session(id, destination, None);
        }
        inverse
    }

    pub fn restore_app_attached_session_placements(
        &mut self,
        placements: &[SavedSessionPlacement],
    ) {
        for placement in placements {
            if let Some(session) = self
                .app_attached_sessions
                .iter_mut()
                .find(|session| session.id == placement.id)
            {
                session.group_id = placement.group_id;
                session.position = placement.position;
            }
        }
    }

    pub fn repair_app_attached_group_references(
        &mut self,
        valid_groups: &HashMap<GroupId, ProjectId>,
    ) -> Vec<HostedSessionId> {
        let mut repaired = Vec::new();
        for session in &mut self.app_attached_sessions {
            let valid = session.group_id.is_none_or(|group_id| {
                valid_groups.get(&group_id) == Some(&session.origin.project_id)
            });
            if !valid {
                session.group_id = None;
                repaired.push(session.id);
            }
        }
        let project_ids = repaired
            .iter()
            .filter_map(|id| {
                self.app_attached_sessions
                    .iter()
                    .find(|session| session.id == *id)
                    .map(|session| session.origin.project_id)
            })
            .collect::<HashSet<_>>();
        for project_id in project_ids {
            self.rebalance_app_attached_destination(project_id, None);
        }
        repaired
    }

    fn project_session_placements(&self, project_id: ProjectId) -> Vec<SavedSessionPlacement> {
        self.app_attached_sessions
            .iter()
            .filter(|session| session.origin.project_id == project_id)
            .map(|session| SavedSessionPlacement {
                id: session.id,
                group_id: session.group_id,
                position: session.position,
            })
            .collect()
    }

    pub fn app_attached_session_placements(
        &self,
        project_id: ProjectId,
    ) -> Vec<SavedSessionPlacement> {
        self.project_session_placements(project_id)
    }

    fn rebalance_app_attached_destination(
        &mut self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
    ) {
        let mut ids = self
            .app_attached_sessions
            .iter()
            .filter(|session| {
                session.origin.project_id == project_id && session.group_id == group_id
            })
            .map(|session| (session.position, session.id))
            .collect::<Vec<_>>();
        ids.sort_by_key(|(position, id)| (*position, *id));
        for (index, (_, id)) in ids.into_iter().enumerate() {
            if let (Ok(position), Some(session)) = (
                PositionKey::rebalanced(index),
                self.app_attached_sessions
                    .iter_mut()
                    .find(|session| session.id == id),
            ) {
                session.position = position;
            }
        }
    }

    pub fn mark_app_attached_sessions_exited(&mut self) {
        for session in &mut self.app_attached_sessions {
            if session.route == SessionLaunchRoute::LegacyAppAttached
                && matches!(
                    session.state,
                    HostedSessionState::Draft
                        | HostedSessionState::Validating
                        | HostedSessionState::Starting
                        | HostedSessionState::RunningAppAttached
                )
            {
                session.state = HostedSessionState::Exited;
            }
        }
    }

    pub fn remove_app_attached_session(&mut self, id: HostedSessionId) {
        self.app_attached_sessions
            .retain(|session| session.id != id);
    }

    pub fn register_managed_agent_worktree(&mut self, worktree: SavedManagedWorktree) {
        if let Some(existing) = self
            .managed_agent_worktrees
            .iter_mut()
            .find(|existing| existing.path == worktree.path)
        {
            *existing = worktree;
        } else {
            self.managed_agent_worktrees.push(worktree);
        }
    }

    pub fn forget_managed_agent_worktree(&mut self, path: &str) {
        self.managed_agent_worktrees
            .retain(|worktree| worktree.path != path);
    }

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

impl SavedAppAttachedSession {
    pub fn to_hosted_session(&self) -> Result<HostedSession, termirust_domain::SessionStateError> {
        let fallback = if self.preset_label.trim().is_empty() {
            format!("Untitled session {}", &self.id.to_string()[..8])
        } else {
            self.preset_label.clone()
        };
        let title = SessionTitle::new(if self.title.trim().is_empty() {
            &fallback
        } else {
            &self.title
        })?;
        let last_output_sequence = OutputSequence::new(
            self.durable_host
                .as_ref()
                .map(|host| host.last_sequence)
                .unwrap_or_default(),
        );
        let read_through_sequence =
            OutputSequence::new(self.read_through_sequence).min(last_output_sequence);
        let unread_sequence = self
            .unread_sequence
            .map(OutputSequence::new)
            .filter(|sequence| *sequence <= last_output_sequence);
        Ok(HostedSession {
            id: self.id,
            project_id: self.origin.project_id,
            group_id: self.group_id,
            preset_id: Some(self.origin.preset_id),
            title,
            title_source: self.title_source,
            lifecycle: self.state,
            activity: self.activity.clone(),
            pinned: self.pinned,
            position: self.position,
            last_output_sequence,
            read_through_sequence,
            unread_sequence,
            archived_at: self.archived_at.filter(|_| self.state.is_exited()),
            created_at: self.started_at,
            updated_at: self.updated_at,
            revision: self.revision,
        })
    }

    pub fn apply_hosted_session(&mut self, session: &HostedSession) {
        self.state = session.lifecycle;
        self.group_id = session.group_id;
        self.position = session.position;
        self.title = session.title.as_str().to_string();
        self.title_source = session.title_source;
        self.activity = session.activity.clone();
        self.pinned = session.pinned;
        self.read_through_sequence = session.read_through_sequence.get();
        self.unread_sequence = session.unread_sequence.map(OutputSequence::get);
        self.archived_at = session.archived_at;
        self.revision = session.revision;
        self.updated_at = session.updated_at;
        if let Some(host) = self.durable_host.as_mut() {
            host.last_sequence = host.last_sequence.max(session.last_output_sequence.get());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, AppSettings,
        AuthConfig, AuthMode, CANVAS_MIN_NODE_WIDTH, CANVAS_MIN_TERMINAL_NODE_WIDTH, CanvasEdgeId,
        CanvasEdgeKind, CanvasNodeId, CanvasNoteColor, ConnectRequest, ConnectionKind,
        DEFAULT_VAULT_ID, DraftProfile, HostColorTag, HostProfile, IdentitySource,
        ImportedIdentity, JumpHostConnection, LocalPortForward, LocalShellConfig,
        MobileDevicePairingRequest, PortForwardKind, PortForwardRule, ProfileSource, QuickConnect,
        RestorableAuth, RestorableConnection, SavedAgentDefinition, SavedCanvasEdge,
        SavedCanvasNode, SavedCanvasNodeKind, SavedCanvasState, SavedCanvasViewport,
        SavedCommandHistoryEntry, SavedContextPolicy, SavedIdentity, SavedManagedWorktree,
        SavedManagedWorktreeDisposition, SavedSnippet, SavedState, SavedVault, SavedVaultMember,
        SavedWorkspace, SavedWorktreePolicy, SessionLogEntry, SessionLogStatus, ThemePreset,
        VaultKind, VaultMemberRole, WorkspaceLayoutMode,
        default_persistent_session_name_for_endpoint, default_persistent_session_name_from_id,
        identity_id_for_path,
    };
    use termirust_domain::{
        HostedSessionId, HostedSessionState, PresetId, ProjectId, Revision, SessionLaunchRoute,
        SessionOrigin, SshAgentForwardingPolicy, SshAuthenticationKind, TitleSource,
    };

    #[test]
    fn parses_user_at_host() {
        let qc = QuickConnect::parse("root@192.168.1.1").unwrap();
        assert_eq!(qc.username, "root");
        assert_eq!(qc.host, "192.168.1.1");
        assert_eq!(qc.port, 22);
    }

    #[test]
    fn app_attached_metadata_is_bounded_and_restart_marks_it_exited() {
        let mut state = SavedState::default();
        let id = HostedSessionId::new();
        state.upsert_app_attached_session(super::SavedAppAttachedSession {
            id,
            route: SessionLaunchRoute::LegacyAppAttached,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state: HostedSessionState::RunningAppAttached,
            project_label: "p".repeat(400),
            preset_label: "preset".to_string(),
            title: String::new(),
            title_source: TitleSource::Default,
            activity: termirust_domain::ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::ZERO,
            durable_host: None,
            group_id: None,
            position: termirust_domain::PositionKey::FIRST,
            started_at: 1,
            updated_at: 2,
        });
        assert_eq!(state.app_attached_sessions.len(), 1);
        assert_eq!(
            state.app_attached_sessions[0].project_label.chars().count(),
            256
        );
        state.mark_app_attached_sessions_exited();
        assert_eq!(
            state.app_attached_sessions[0].state,
            HostedSessionState::Exited
        );
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("argv"));
        assert!(!json.contains("initial_input"));
        assert!(!json.contains("working_directory"));
        assert_eq!(state.app_attached_sessions[0].id, id);
    }

    #[test]
    fn restart_preserves_durable_host_state_and_metadata() {
        let mut state = SavedState::default();
        let id = HostedSessionId::new();
        state.upsert_app_attached_session(super::SavedAppAttachedSession {
            id,
            route: SessionLaunchRoute::DurableHost,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state: HostedSessionState::Live,
            project_label: "project".to_string(),
            preset_label: "codex".to_string(),
            title: "codex".to_string(),
            title_source: TitleSource::Default,
            activity: termirust_domain::ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::ZERO,
            durable_host: Some(super::SavedDurableHost {
                runtime_root: "/tmp/runtime".to_string(),
                session_dir: "/tmp/session".to_string(),
                working_directory: Some("/tmp/project".to_string()),
                last_sequence: 41,
                durable_sequence: 39,
                runtime_recognition: None,
                conversation_handle: Some(
                    termirust_domain::ConversationHandle::codex(
                        "019cf76d-0493-77d1-8572-3fb4ac801ac8",
                    )
                    .unwrap(),
                ),
                executable: None,
                permission_policy: termirust_domain::PermissionPolicy::default(),
                continuity_source_id: None,
            }),
            group_id: None,
            position: termirust_domain::PositionKey::FIRST,
            started_at: 1,
            updated_at: 2,
        });

        state.mark_app_attached_sessions_exited();

        assert_eq!(
            state.app_attached_sessions[0].state,
            HostedSessionState::Live
        );
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("019cf76d-0493-77d1-8572-3fb4ac801ac8"));
        let restored: SavedState = serde_json::from_str(&json).unwrap();
        let host = restored.app_attached_sessions[0]
            .durable_host
            .as_ref()
            .unwrap();
        assert_eq!(host.last_sequence, 41);
        assert_eq!(host.durable_sequence, 39);
        assert_eq!(host.conversation_handle, None);
        assert_eq!(restored.app_attached_sessions[0].id, id);
    }

    #[test]
    fn restorable_connection_defaults_old_json_and_preserves_durable_session_id() {
        let old: RestorableConnection = serde_json::from_str(
            r#"{"title":"Local","kind":"local_shell","local_shell":{"program":"/bin/sh","args":[],"cwd":null}}"#,
        )
        .unwrap();
        assert_eq!(old.durable_session_id, None);

        let id = HostedSessionId::new();
        let mut durable = old;
        durable.durable_session_id = Some(id);
        let encoded = serde_json::to_string(&durable).unwrap();
        let decoded: RestorableConnection = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.durable_session_id, Some(id));
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
            AuthConfig::Password { .. }
            | AuthConfig::PasswordRef { .. }
            | AuthConfig::OpenSshCertificate { .. }
            | AuthConfig::LocalAgent { .. } => {
                panic!("expected private key auth")
            }
        }
    }

    #[test]
    fn openssh_certificate_profiles_and_sessions_round_trip_without_downgrade() {
        let draft = DraftProfile {
            label: "certificate host".to_string(),
            host: "cert.example.com".to_string(),
            username: "deploy".to_string(),
            auth_mode: AuthMode::PrivateKey,
            key_path: "/tmp/id_ed25519".to_string(),
            certificate_path: "/tmp/id_ed25519-cert.pub".to_string(),
            key_passphrase: "secret".to_string(),
            ..DraftProfile::default()
        };

        let profile = draft.to_profile("profile-cert".to_string()).unwrap();
        assert_eq!(
            profile.ssh_access_policy().authentication,
            SshAuthenticationKind::OpenSshCertificate
        );
        let request = draft.to_connect_request(91).unwrap();
        match request.auth.as_ref().unwrap() {
            AuthConfig::OpenSshCertificate {
                key_path,
                certificate_path,
                passphrase,
            } => {
                assert_eq!(key_path, "/tmp/id_ed25519");
                assert_eq!(certificate_path, "/tmp/id_ed25519-cert.pub");
                assert_eq!(passphrase.as_deref(), Some("secret"));
            }
            _ => panic!("certificate profile was downgraded to another authentication method"),
        }

        let restored = request
            .to_restorable()
            .expect("certificate request restores");
        let round_trip = restored.to_connect_request(92);
        match round_trip.auth.unwrap() {
            AuthConfig::OpenSshCertificate {
                key_path,
                certificate_path,
                passphrase,
            } => {
                assert_eq!(key_path, "/tmp/id_ed25519");
                assert_eq!(certificate_path, "/tmp/id_ed25519-cert.pub");
                assert_eq!(passphrase, None);
            }
            _ => panic!("restored certificate session was downgraded"),
        }
    }

    #[test]
    fn password_profile_discards_stale_certificate_fields() {
        let draft = DraftProfile {
            host: "password.example.com".to_string(),
            username: "deploy".to_string(),
            auth_mode: AuthMode::Password,
            key_path: "/tmp/stale-key".to_string(),
            certificate_path: "/tmp/stale-cert.pub".to_string(),
            password_credential_id: Some("profile:password".to_string()),
            ..DraftProfile::default()
        };

        let profile = draft.to_profile("profile-password".to_string()).unwrap();
        assert_eq!(profile.certificate_path, None);
        assert_eq!(
            profile.ssh_access_policy().authentication,
            SshAuthenticationKind::Password
        );
    }

    #[test]
    fn persistent_session_default_name_sanitizes_profile_id() {
        assert_eq!(
            default_persistent_session_name_from_id("profile-1719356789123"),
            "tr-profile-1719356789123"
        );
        assert_eq!(
            default_persistent_session_name_from_id("profile:prod/east"),
            "tr-profile-prod-east"
        );
    }

    #[test]
    fn persistent_session_endpoint_fallback_is_deterministic() {
        assert_eq!(
            default_persistent_session_name_for_endpoint("deploy", "prod.example.com", 2222),
            "tr-deploy-prod-example-com-2222"
        );
        assert_eq!(
            default_persistent_session_name_for_endpoint("", "", 22),
            "tr-22"
        );
    }

    #[test]
    fn legacy_host_profile_defaults_to_non_persistent_session() {
        let profile: HostProfile = serde_json::from_str(
            r#"{
                "id": "profile-1",
                "label": "prod",
                "host": "prod.example.com",
                "port": 22,
                "username": "deploy"
            }"#,
        )
        .expect("legacy profile should deserialize");

        assert!(!profile.persistent_session);
        assert_eq!(profile.persistent_session_name, None);
        assert!(!profile.persistent_session_detach_others);
    }

    #[test]
    fn legacy_host_profiles_project_to_safe_ssh_access_policies() {
        let password: HostProfile = serde_json::from_str(
            r#"{
                "id": "profile-password",
                "label": "password",
                "host": "password.example.com",
                "username": "deploy"
            }"#,
        )
        .expect("legacy password profile should deserialize");
        let private_key: HostProfile = serde_json::from_str(
            r#"{
                "id": "profile-key",
                "label": "key",
                "host": "key.example.com",
                "username": "deploy",
                "auth_mode": "private_key",
                "key_path": "/tmp/id_ed25519"
            }"#,
        )
        .expect("legacy private-key profile should deserialize");

        let password_policy = password.ssh_access_policy();
        assert_eq!(
            password_policy.authentication,
            SshAuthenticationKind::Password
        );
        assert_eq!(
            password_policy.agent_forwarding,
            SshAgentForwardingPolicy::Disabled
        );

        let key_policy = private_key.ssh_access_policy();
        assert_eq!(key_policy.authentication, SshAuthenticationKind::PrivateKey);
        assert_eq!(
            key_policy.agent_forwarding,
            SshAgentForwardingPolicy::Disabled
        );

        let serialized = serde_json::to_value(private_key).expect("serialize legacy profile");
        assert!(serialized.get("ssh_access_policy").is_none());
        assert!(serialized.get("agent_forwarding").is_none());
        assert!(serialized.get("certificate_signer").is_none());
    }

    #[test]
    fn local_agent_forwarding_is_one_shot_and_not_restorable() {
        let mut auth = AuthConfig::LocalAgent {
            socket_path: Some("/tmp/test-agent.sock".to_string()),
            forward_agent: false,
        };
        assert!(auth.forwarded_agent_socket().is_none());
        auth.enable_one_shot_agent_forwarding().unwrap();
        assert_eq!(
            auth.forwarded_agent_socket().flatten(),
            Some("/tmp/test-agent.sock")
        );

        let restorable = auth.to_restorable().expect("agent auth should restore");
        let RestorableAuth::LocalAgent { socket_path } = restorable else {
            panic!("expected local-agent restore state");
        };
        assert_eq!(socket_path.as_deref(), Some("/tmp/test-agent.sock"));

        auth.disable_agent_forwarding();
        assert!(auth.forwarded_agent_socket().is_none());
        assert!(
            AuthConfig::PrivateKey {
                key_path: "/tmp/key".to_string(),
                passphrase: None,
            }
            .enable_one_shot_agent_forwarding()
            .is_err()
        );
    }

    #[test]
    fn restored_local_agent_auth_never_forwards() {
        let restored = RestorableConnection {
            title: "agent host".to_string(),
            kind: ConnectionKind::Ssh,
            host: "agent.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: Some(RestorableAuth::LocalAgent {
                socket_path: Some("/tmp/test-agent.sock".to_string()),
            }),
            jump_host: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: None,
            port_forward_rules: Vec::new(),
            local_forwards: Vec::new(),
            local_forward: None,
            local_shell: None,
            durable_session_id: None,
        }
        .to_connect_request(99);

        assert!(matches!(
            restored.auth,
            Some(AuthConfig::LocalAgent {
                forward_agent: false,
                ..
            })
        ));
    }

    #[test]
    fn persistent_session_fields_round_trip_through_restorable_connection() {
        let request = ConnectRequest {
            session_id: 4,
            title: "prod".to_string(),
            kind: ConnectionKind::Ssh,
            host: "prod.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            auth: Some(AuthConfig::PrivateKey {
                key_path: "/tmp/id_ed25519".to_string(),
                passphrase: None,
            }),
            jump_host: None,
            startup_directory: Some("/srv/app".to_string()),
            startup_command: Some("uptime".to_string()),
            start_in_files: false,
            persistent_session: true,
            persistent_session_name: Some("tr-prod".to_string()),
            persistent_session_detach_others: true,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };

        let restored = request.to_restorable().expect("request should restore");
        assert!(restored.persistent_session);
        assert_eq!(restored.persistent_session_name.as_deref(), Some("tr-prod"));
        assert!(restored.persistent_session_detach_others);

        let round_trip = restored.to_connect_request(8);
        assert!(round_trip.persistent_session);
        assert_eq!(
            round_trip.persistent_session_name.as_deref(),
            Some("tr-prod")
        );
        assert!(round_trip.persistent_session_detach_others);
    }

    #[test]
    fn persistent_local_tmux_round_trips_through_workspace_restore() {
        let request = ConnectRequest::persistent_local_shell_with_config(
            11,
            LocalShellConfig {
                program: "/bin/zsh".to_string(),
                args: vec!["-l".to_string()],
                cwd: Some("/tmp/project with spaces".to_string()),
            },
            "tr-local-project-11".to_string(),
            true,
        );

        let restored = request
            .to_restorable()
            .expect("local request should restore");
        let round_trip = restored.to_connect_request(12);
        assert!(round_trip.is_local_shell());
        assert!(round_trip.persistent_session);
        assert_eq!(
            round_trip.persistent_session_name.as_deref(),
            Some("tr-local-project-11")
        );
        assert!(round_trip.persistent_session_detach_others);
        assert_eq!(round_trip.local_shell, request.local_shell);
    }

    #[test]
    fn saved_workspace_normalizes_active_pane() {
        let mut workspace = SavedWorkspace {
            title: "prod".to_string(),
            project_directory: None,
            layout_mode: WorkspaceLayoutMode::Split,
            layout: None,
            canvas: None,
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
                persistent_session: false,
                persistent_session_name: None,
                persistent_session_detach_others: false,
                terminal_scrollback_rows: None,
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
                local_forward: None,
                local_shell: None,
                durable_session_id: None,
            }],
        };

        workspace.normalize();
        assert_eq!(workspace.active_pane_index, 0);
    }

    #[test]
    fn saved_state_serializes_restored_password_workspace_shape() {
        let mut state = SavedState::default();
        state.settings.restore_workspaces_on_launch = true;
        state.restored_workspaces.push(SavedWorkspace {
            title: "docker-e2e".to_string(),
            project_directory: None,
            layout_mode: WorkspaceLayoutMode::Split,
            layout: None,
            canvas: None,
            active_pane_index: 0,
            panes: vec![RestorableConnection {
                title: "docker-e2e".to_string(),
                kind: ConnectionKind::Ssh,
                host: "127.0.0.1".to_string(),
                port: 55558,
                username: "termirust".to_string(),
                auth: Some(RestorableAuth::PasswordKeychain {
                    credential_id: "profile:real-app-docker-e2e".to_string(),
                }),
                jump_host: None,
                startup_directory: None,
                startup_command: None,
                start_in_files: false,
                persistent_session: false,
                persistent_session_name: None,
                persistent_session_detach_others: false,
                terminal_scrollback_rows: Some(10_000),
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
                local_forward: None,
                local_shell: None,
                durable_session_id: None,
            }],
        });
        state.active_workspace_index = Some(0);

        let value = serde_json::to_value(&state).expect("saved state should serialize");
        let workspace = &value["restored_workspaces"][0];
        assert_eq!(workspace["title"], "docker-e2e");
        assert_eq!(workspace["layout"], serde_json::Value::Null);
        assert_eq!(workspace["active_pane_index"], 0);
        assert_eq!(workspace["panes"][0]["kind"], "ssh");
        assert_eq!(
            workspace["panes"][0]["auth"]["password_keychain"]["credential_id"],
            "profile:real-app-docker-e2e"
        );
        assert_eq!(value["active_workspace_index"], 0);
    }

    fn canvas_terminal_node(id: &str, pane_index: usize) -> SavedCanvasNode {
        SavedCanvasNode {
            id: CanvasNodeId::new(id),
            kind: SavedCanvasNodeKind::Terminal { pane_index },
            x: 10.0,
            y: 20.0,
            width: 720.0,
            height: 460.0,
            z_index: 0,
            title: None,
            collapsed: false,
        }
    }

    #[test]
    fn legacy_saved_workspace_defaults_to_split_without_canvas() {
        let workspace: SavedWorkspace = serde_json::from_str(
            r#"{
                "title": "legacy",
                "active_pane_index": 0,
                "panes": []
            }"#,
        )
        .expect("legacy workspace should deserialize");

        assert_eq!(workspace.layout_mode, WorkspaceLayoutMode::Split);
        assert_eq!(workspace.canvas, None);
        assert_eq!(workspace.project_directory, None);
    }

    #[test]
    fn canvas_workspace_round_trips_agent_definition_and_edges() {
        let mut workspace = SavedWorkspace {
            title: "agents".to_string(),
            project_directory: Some(" /srv/project ".to_string()),
            layout_mode: WorkspaceLayoutMode::Canvas,
            layout: None,
            canvas: Some(SavedCanvasState {
                schema_version: 1,
                viewport: SavedCanvasViewport {
                    pan_x: 42.0,
                    pan_y: -18.0,
                    zoom: 1.25,
                },
                nodes: vec![SavedCanvasNode {
                    id: CanvasNodeId::new("agent-a"),
                    kind: SavedCanvasNodeKind::Agent {
                        pane_index: None,
                        definition: SavedAgentDefinition {
                            provider: AgentProvider::ClaudeCode,
                            backend: AgentBackendKind::Structured,
                            location: AgentLocation::SavedHost {
                                profile_id: "prod".to_string(),
                            },
                            working_directory: Some("/srv/app".to_string()),
                            executable_override: None,
                            arguments: vec!["--verbose".to_string()],
                            permission_policy: AgentPermissionPolicy::ReadOnly,
                            worktree: SavedWorktreePolicy::ReadOnly,
                            managed_worktree: None,
                        },
                    },
                    x: 10.0,
                    y: 20.0,
                    width: 720.0,
                    height: 460.0,
                    z_index: 2,
                    title: Some("Review".to_string()),
                    collapsed: false,
                }],
                edges: Vec::new(),
            }),
            active_pane_index: 0,
            panes: Vec::new(),
        };
        workspace.normalize();
        assert_eq!(workspace.project_directory.as_deref(), Some("/srv/project"));

        let value = serde_json::to_string(&workspace).expect("canvas should serialize");
        let decoded: SavedWorkspace =
            serde_json::from_str(&value).expect("canvas should deserialize");
        assert_eq!(decoded.layout_mode, WorkspaceLayoutMode::Canvas);
        assert_eq!(decoded.project_directory, workspace.project_directory);
        assert_eq!(decoded.canvas, workspace.canvas);
    }

    #[test]
    fn canvas_normalize_repairs_geometry_ids_and_dangling_edges() {
        let mut canvas = SavedCanvasState {
            schema_version: 0,
            viewport: SavedCanvasViewport {
                pan_x: f32::NAN,
                pan_y: f32::INFINITY,
                zoom: 50.0,
            },
            nodes: vec![
                SavedCanvasNode {
                    id: CanvasNodeId::new("same"),
                    x: f32::NAN,
                    width: 1.0,
                    height: f32::NAN,
                    ..canvas_terminal_node("same", 0)
                },
                canvas_terminal_node("same", 1),
                canvas_terminal_node("missing-pane", 5),
            ],
            edges: vec![SavedCanvasEdge {
                id: CanvasEdgeId::new("dangling"),
                source: CanvasNodeId::new("same"),
                target: CanvasNodeId::new("missing"),
                kind: CanvasEdgeKind::Context,
                enabled: true,
                context_policy: None,
            }],
        };

        let report = canvas.normalize(2);

        assert!(report.changed());
        assert_eq!(canvas.schema_version, 1);
        assert_eq!(canvas.viewport.pan_x, 0.0);
        assert_eq!(canvas.viewport.pan_y, 0.0);
        assert_eq!(canvas.viewport.zoom, 2.0);
        assert_eq!(canvas.nodes.len(), 2);
        assert_eq!(canvas.nodes[0].id.as_str(), "same");
        assert_eq!(canvas.nodes[1].id.as_str(), "same-2");
        assert_eq!(canvas.nodes[0].width, CANVAS_MIN_TERMINAL_NODE_WIDTH);
        assert_eq!(canvas.nodes[0].height, 460.0);
        assert!(canvas.edges.is_empty());
    }

    #[test]
    fn canvas_normalize_uses_terminal_specific_minimum_width() {
        let mut canvas = SavedCanvasState {
            nodes: vec![
                SavedCanvasNode {
                    width: 1.0,
                    ..canvas_terminal_node("terminal", 0)
                },
                SavedCanvasNode {
                    id: CanvasNodeId::new("agent"),
                    kind: SavedCanvasNodeKind::Agent {
                        pane_index: None,
                        definition: SavedAgentDefinition::default(),
                    },
                    x: 10.0,
                    y: 20.0,
                    width: 1.0,
                    height: 460.0,
                    z_index: 1,
                    title: None,
                    collapsed: false,
                },
            ],
            ..SavedCanvasState::default()
        };

        canvas.normalize(1);

        assert_eq!(canvas.nodes[0].width, CANVAS_MIN_TERMINAL_NODE_WIDTH);
        assert_eq!(canvas.nodes[1].width, CANVAS_MIN_NODE_WIDTH);
    }

    #[test]
    fn canvas_normalize_rejects_dependency_cycles_and_duplicate_edges() {
        let mut canvas = SavedCanvasState {
            nodes: vec![
                canvas_terminal_node("a", 0),
                canvas_terminal_node("b", 1),
                canvas_terminal_node("c", 2),
            ],
            edges: vec![
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("ab"),
                    source: CanvasNodeId::new("a"),
                    target: CanvasNodeId::new("b"),
                    kind: CanvasEdgeKind::Dependency,
                    enabled: true,
                    context_policy: Some(SavedContextPolicy::default()),
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("bc"),
                    source: CanvasNodeId::new("b"),
                    target: CanvasNodeId::new("c"),
                    kind: CanvasEdgeKind::Dependency,
                    enabled: true,
                    context_policy: None,
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("ca"),
                    source: CanvasNodeId::new("c"),
                    target: CanvasNodeId::new("a"),
                    kind: CanvasEdgeKind::Dependency,
                    enabled: true,
                    context_policy: None,
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("ab-copy"),
                    source: CanvasNodeId::new("a"),
                    target: CanvasNodeId::new("b"),
                    kind: CanvasEdgeKind::Dependency,
                    enabled: true,
                    context_policy: None,
                },
            ],
            ..SavedCanvasState::default()
        };

        let report = canvas.normalize(3);

        assert_eq!(report.removed_edges, 2);
        assert_eq!(canvas.edges.len(), 2);
        assert!(
            canvas
                .edges
                .iter()
                .all(|edge| edge.context_policy.is_none())
        );
    }

    #[test]
    fn canvas_notes_and_groups_normalize_and_round_trip_safely() {
        let note_id = CanvasNodeId::new("note");
        let terminal_id = CanvasNodeId::new("terminal");
        let group_id = CanvasNodeId::new("group");
        let mut canvas = SavedCanvasState {
            nodes: vec![
                canvas_terminal_node(terminal_id.as_str(), 0),
                SavedCanvasNode {
                    id: note_id.clone(),
                    kind: SavedCanvasNodeKind::Note {
                        text: "é".repeat(40_000),
                        color: CanvasNoteColor::Blue,
                    },
                    x: 20.0,
                    y: 30.0,
                    width: 420.0,
                    height: 300.0,
                    z_index: 2,
                    title: Some("Deploy notes".to_string()),
                    collapsed: false,
                },
                SavedCanvasNode {
                    id: group_id.clone(),
                    kind: SavedCanvasNodeKind::Group {
                        member_ids: vec![
                            terminal_id.clone(),
                            note_id.clone(),
                            note_id.clone(),
                            CanvasNodeId::new("missing"),
                            group_id.clone(),
                        ],
                    },
                    x: 0.0,
                    y: 0.0,
                    width: 900.0,
                    height: 600.0,
                    z_index: 0,
                    title: Some("Release".to_string()),
                    collapsed: false,
                },
            ],
            edges: vec![
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("note-context"),
                    source: note_id.clone(),
                    target: terminal_id.clone(),
                    kind: CanvasEdgeKind::Context,
                    enabled: true,
                    context_policy: Some(SavedContextPolicy::default()),
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("note-dependency"),
                    source: note_id.clone(),
                    target: terminal_id.clone(),
                    kind: CanvasEdgeKind::Dependency,
                    enabled: true,
                    context_policy: None,
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("group-context"),
                    source: group_id.clone(),
                    target: terminal_id.clone(),
                    kind: CanvasEdgeKind::Context,
                    enabled: true,
                    context_policy: None,
                },
                SavedCanvasEdge {
                    id: CanvasEdgeId::new("invalid-target"),
                    source: terminal_id.clone(),
                    target: note_id.clone(),
                    kind: CanvasEdgeKind::Context,
                    enabled: true,
                    context_policy: None,
                },
            ],
            ..SavedCanvasState::default()
        };

        let report = canvas.normalize(1);

        assert!(report.changed());
        assert_eq!(report.removed_edges, 3);
        assert_eq!(canvas.edges.len(), 1);
        let SavedCanvasNodeKind::Note { text, color } = &canvas.nodes[1].kind else {
            panic!("expected note node");
        };
        assert!(text.len() <= 64 * 1024);
        assert!(text.is_char_boundary(text.len()));
        assert_eq!(*color, CanvasNoteColor::Blue);
        let SavedCanvasNodeKind::Group { member_ids } = &canvas.nodes[2].kind else {
            panic!("expected group node");
        };
        assert_eq!(member_ids, &vec![terminal_id, note_id]);

        let json = serde_json::to_string(&canvas).expect("canvas should serialize");
        let restored: SavedCanvasState =
            serde_json::from_str(&json).expect("canvas should deserialize");
        assert_eq!(restored, canvas);
    }

    #[test]
    fn canvas_future_schema_falls_back_to_split_without_mutation() {
        let mut workspace = SavedWorkspace {
            title: "future".to_string(),
            layout_mode: WorkspaceLayoutMode::Canvas,
            canvas: Some(SavedCanvasState {
                schema_version: 99,
                viewport: SavedCanvasViewport {
                    pan_x: f32::NAN,
                    pan_y: 0.0,
                    zoom: 1.0,
                },
                ..SavedCanvasState::default()
            }),
            ..SavedWorkspace::default()
        };

        workspace.normalize();

        assert_eq!(workspace.layout_mode, WorkspaceLayoutMode::Split);
        assert!(workspace
            .canvas
            .as_ref()
            .is_some_and(|canvas| canvas.schema_version == 99 && canvas.viewport.pan_x.is_nan()));
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
    fn generated_identity_source_round_trips_and_old_state_defaults_to_user() {
        let generated = SavedIdentity {
            id: "generated-id".to_string(),
            label: "Generated".to_string(),
            vault_id: None,
            key_path: "/tmp/generated".to_string(),
            kind: "ED25519".to_string(),
            source: IdentitySource::Generated,
        };
        let encoded = serde_json::to_string(&generated).unwrap();
        assert!(encoded.contains("\"source\":\"generated\""));
        assert_eq!(
            serde_json::from_str::<SavedIdentity>(&encoded)
                .unwrap()
                .source,
            IdentitySource::Generated
        );

        let old_state = r#"{
            "id":"legacy-id",
            "label":"Legacy",
            "key_path":"/tmp/legacy",
            "kind":"OpenSSH"
        }"#;
        assert_eq!(
            serde_json::from_str::<SavedIdentity>(old_state)
                .unwrap()
                .source,
            IdentitySource::User
        );
    }

    #[test]
    fn imported_identities_do_not_replace_generated_identities() {
        let mut state = SavedState::default();
        state.identities.push(SavedIdentity {
            id: identity_id_for_path("/tmp/generated"),
            label: "generated-key".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            key_path: "/tmp/generated".to_string(),
            kind: "ED25519".to_string(),
            source: IdentitySource::Generated,
        });

        state.merge_imported_identities(vec![ImportedIdentity {
            label: "generated".to_string(),
            path: "/tmp/generated".to_string(),
            kind: "OpenSSH".to_string(),
        }]);

        assert_eq!(state.identities.len(), 1);
        assert_eq!(state.identities[0].label, "generated-key");
        assert_eq!(state.identities[0].source, IdentitySource::Generated);
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
            certificate_path: String::new(),
            identity_agent: String::new(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: "/var/www/app".to_string(),
            startup_command: "docker compose ps".to_string(),
            start_in_files: true,
            persistent_session: false,
            persistent_session_name: String::new(),
            persistent_session_detach_others: false,
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
            certificate_path: String::new(),
            identity_agent: String::new(),
            identity_id: None,
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: String::new(),
            persistent_session_detach_others: false,
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
                certificate_path: String::new(),
                identity_agent: String::new(),
                identity_id: None,
                jump_host_id: None,
                startup_directory: String::new(),
                startup_command: String::new(),
                start_in_files: false,
                persistent_session: false,
                persistent_session_name: String::new(),
                persistent_session_detach_others: false,
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
                certificate_path: String::new(),
                identity_agent: String::new(),
                identity_id: None,
                jump_host_id: None,
                startup_directory: String::new(),
                startup_command: String::new(),
                start_in_files: false,
                persistent_session: false,
                persistent_session_name: String::new(),
                persistent_session_detach_others: false,
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
            certificate_path: String::new(),
            identity_agent: String::new(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: String::new(),
            persistent_session_detach_others: false,
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
            certificate_path: String::new(),
            identity_agent: String::new(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: String::new(),
            persistent_session_detach_others: false,
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
            certificate_path: String::new(),
            identity_agent: String::new(),
            identity_id: Some("identity-123".to_string()),
            jump_host_id: None,
            startup_directory: String::new(),
            startup_command: String::new(),
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: String::new(),
            persistent_session_detach_others: false,
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
            certificate_path: None,
            identity_agent: None,
            identity_id: None,
            jump_host_id: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
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
    fn legacy_saved_state_defaults_managed_worktree_registry() {
        let state: SavedState = serde_json::from_str("{}").expect("deserialize legacy state");
        assert!(state.managed_agent_worktrees.is_empty());
    }

    #[test]
    fn managed_worktree_registry_upserts_and_forgets_by_path() {
        let first = SavedManagedWorktree {
            repository_root: "/repo".to_string(),
            path: "/managed/agent".to_string(),
            branch: "termirust/agent/one".to_string(),
            base_revision: "abc".to_string(),
            owner_id: Some("node-1".to_string()),
            disposition: SavedManagedWorktreeDisposition::Active,
        };
        let mut updated = first.clone();
        updated.branch = "termirust/agent/two".to_string();
        let mut state = SavedState::default();

        state.register_managed_agent_worktree(first);
        state.register_managed_agent_worktree(updated.clone());

        assert_eq!(state.managed_agent_worktrees, vec![updated]);
        state.forget_managed_agent_worktree("/managed/agent");
        assert!(state.managed_agent_worktrees.is_empty());
    }

    #[test]
    fn legacy_managed_worktree_defaults_lifecycle_metadata() {
        let worktree: SavedManagedWorktree = serde_json::from_str(
            r#"{
                "repository_root": "/repo",
                "path": "/managed/agent",
                "branch": "termirust/agent/one",
                "base_revision": "abc"
            }"#,
        )
        .expect("deserialize legacy managed worktree");

        assert_eq!(worktree.owner_id, None);
        assert_eq!(
            worktree.disposition,
            SavedManagedWorktreeDisposition::Active
        );
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
        assert!(from_legacy.mobile_devices.is_empty());
        assert!(from_legacy.mobile_device_keys.is_empty());
        assert!(from_legacy.diagnostics_enabled);
        assert_eq!(from_legacy.diagnostics_max_file_mib, 10);
        assert_eq!(from_legacy.diagnostics_retention_days, 14);
    }

    #[test]
    fn diagnostic_settings_are_clamped_to_privacy_bounds() {
        let mut state = SavedState::default();
        state.settings.diagnostics_max_file_mib = u8::MAX;
        state.settings.diagnostics_retention_days = u8::MAX;
        state.ensure_settings();
        assert_eq!(state.settings.diagnostics_max_file_mib, 10);
        assert_eq!(state.settings.diagnostics_retention_days, 14);
    }

    #[test]
    fn settings_apply_mobile_pairing_request_persists_device_record() {
        let mut settings = AppSettings::default();
        let request = MobileDevicePairingRequest::new(
            "pair-1",
            "ios-1",
            "Jacob iPhone",
            "ios",
            Some("x25519-public-key".to_string()),
            10,
        );

        settings
            .apply_mobile_pairing_request(request, 20)
            .expect("pairing request should apply");

        assert_eq!(settings.mobile_devices.len(), 1);
        let device = &settings.mobile_devices[0];
        assert_eq!(device.device_id, "ios-1");
        assert_eq!(device.label, "Jacob iPhone");
        assert_eq!(device.platform.as_deref(), Some("ios"));
        assert_eq!(device.public_key.as_deref(), Some("x25519-public-key"));
        assert_eq!(device.paired_at_millis, Some(20));
        assert_eq!(device.last_seen_at_millis, Some(20));
        assert_eq!(device.revoked_at_millis, None);
    }

    #[test]
    fn settings_revoke_mobile_device_blocks_repairing() {
        let mut settings = AppSettings::default();
        let request = MobileDevicePairingRequest::new(
            "pair-1",
            "android-1",
            "Jacob Android",
            "android",
            None,
            10,
        );
        settings
            .apply_mobile_pairing_request(request.clone(), 20)
            .expect("pairing request should apply");
        settings
            .revoke_mobile_device("android-1", 30)
            .expect("device should revoke");

        let error = settings
            .apply_mobile_pairing_request(request, 40)
            .expect_err("revoked device should not pair");

        assert_eq!(
            error.to_string(),
            "Mobile device android-1 is revoked and cannot be paired."
        );
        assert_eq!(settings.mobile_devices[0].revoked_at_millis, Some(30));
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
