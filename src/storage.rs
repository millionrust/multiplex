use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::models::{AuthMode, HostProfile, ImportedIdentity, ProfileSource, SavedState};

const APP_DIR_NAME: &str = "termirust";
const STATE_FILE_NAME: &str = "state.json";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts.json";

fn app_dir() -> Result<PathBuf> {
    let base_dir = dirs::config_dir().unwrap_or(std::env::current_dir()?);
    let path = base_dir.join(APP_DIR_NAME);
    fs::create_dir_all(&path)
        .with_context(|| format!("Unable to create app directory at {}", path.display()))?;
    Ok(path)
}

fn state_file() -> Result<PathBuf> {
    Ok(app_dir()?.join(STATE_FILE_NAME))
}

fn known_hosts_file() -> Result<PathBuf> {
    Ok(app_dir()?.join(KNOWN_HOSTS_FILE_NAME))
}

pub fn load_saved_state() -> Result<SavedState> {
    let path = state_file()?;
    if !path.exists() {
        let mut state = SavedState::default();
        state.ensure_vaults();
        return Ok(state);
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("Unable to read {}", path.display()))?;
    let mut state: SavedState = serde_json::from_str(&content)
        .with_context(|| format!("Unable to parse {}", path.display()))?;
    state.ensure_vaults();
    Ok(state)
}

pub fn save_saved_state(state: &SavedState) -> Result<()> {
    let path = state_file()?;
    let mut persisted = state.clone();
    persisted.ensure_vaults();
    persisted
        .profiles
        .retain(|profile| profile.source == ProfileSource::User);
    persisted
        .restored_workspaces
        .iter_mut()
        .for_each(|workspace| workspace.normalize());
    persisted
        .restored_workspaces
        .retain(|workspace| !workspace.panes.is_empty());
    if persisted
        .selected_profile_id
        .as_ref()
        .is_some_and(|profile_id| !persisted.profiles.iter().any(|item| &item.id == profile_id))
    {
        persisted.selected_profile_id =
            persisted.profiles.first().map(|profile| profile.id.clone());
    }
    if persisted
        .active_workspace_index
        .is_some_and(|index| index >= persisted.restored_workspaces.len())
    {
        persisted.active_workspace_index = None;
    }

    let content = serde_json::to_string_pretty(&persisted)?;
    fs::write(&path, content).with_context(|| format!("Unable to write {}", path.display()))?;
    Ok(())
}

pub fn load_local_ssh_identities() -> Result<Vec<ImportedIdentity>> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(Vec::new());
    };

    let ssh_dir = home_dir.join(".ssh");
    if !ssh_dir.exists() {
        return Ok(Vec::new());
    }

    let mut identities = Vec::new();
    for entry in
        fs::read_dir(&ssh_dir).with_context(|| format!("Unable to read {}", ssh_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let label = entry.file_name().to_string_lossy().trim().to_string();
        if should_skip_ssh_entry(&label) {
            continue;
        }

        let Some(identity) = inspect_identity_file(&path)? else {
            continue;
        };

        identities.push(ImportedIdentity {
            label: label.clone(),
            ..identity
        });
    }

    identities.sort_by(|left, right| {
        identity_priority(&left.label)
            .cmp(&identity_priority(&right.label))
            .then_with(|| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
            })
    });

    Ok(identities)
}

pub fn inspect_identity_file(path: &std::path::Path) -> Result<Option<ImportedIdentity>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };

    let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let Some(kind) = detect_identity_kind(&preview) else {
        return Ok(None);
    };

    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| path.display().to_string());

    Ok(Some(ImportedIdentity {
        label,
        path: path.display().to_string(),
        kind: kind.to_string(),
    }))
}

fn should_skip_ssh_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();

    lower.starts_with('.')
        || lower.ends_with(".pub")
        || lower == "authorized_keys"
        || lower == "config"
        || lower.starts_with("known_hosts")
}

fn detect_identity_kind(preview: &str) -> Option<&'static str> {
    let preview = preview.trim_start_matches('\u{feff}').trim_start();

    if preview.starts_with("PuTTY-User-Key-File-") {
        Some("PuTTY PPK")
    } else if preview.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        Some("OpenSSH")
    } else if preview.starts_with("-----BEGIN RSA PRIVATE KEY-----") {
        Some("RSA PEM")
    } else if preview.starts_with("-----BEGIN EC PRIVATE KEY-----") {
        Some("EC PEM")
    } else if preview.starts_with("-----BEGIN PRIVATE KEY-----")
        || preview.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----")
    {
        Some("PKCS#8")
    } else {
        None
    }
}

fn identity_priority(label: &str) -> u8 {
    let lower = label.to_ascii_lowercase();

    match lower.as_str() {
        "id_ed25519" => 0,
        "id_ecdsa" => 1,
        "id_rsa" => 2,
        "identity" => 3,
        _ if lower.starts_with("id_") => 4,
        _ => 5,
    }
}

pub fn load_local_ssh_hosts() -> Result<Vec<HostProfile>> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(Vec::new());
    };

    let config_path = home_dir.join(".ssh").join("config");
    if !config_path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Unable to read {}", config_path.display()))?;
    Ok(parse_ssh_config_hosts(&content))
}

#[derive(Default)]
struct SshConfigBlock {
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_jump: Option<String>,
    unsupported_proxy_command: bool,
}

fn parse_ssh_config_hosts(content: &str) -> Vec<HostProfile> {
    let mut entries = HashMap::new();
    let mut aliases = Vec::new();
    let mut block = SshConfigBlock::default();

    for raw_line in content.lines() {
        let line = strip_ssh_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        let Some((key, value)) = split_ssh_directive(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();

        if key == "host" {
            flush_ssh_config_block(&mut entries, &aliases, &block);
            aliases = value
                .split_whitespace()
                .filter(|alias| is_importable_host_alias(alias))
                .map(|alias| alias.to_string())
                .collect();
            block = SshConfigBlock::default();
            continue;
        }

        if key == "match" {
            flush_ssh_config_block(&mut entries, &aliases, &block);
            aliases.clear();
            block = SshConfigBlock::default();
            continue;
        }

        if aliases.is_empty() {
            continue;
        }

        match key.as_str() {
            "hostname" => block.host_name = Some(value.to_string()),
            "user" => block.user = Some(value.to_string()),
            "port" => block.port = value.parse::<u16>().ok(),
            "identityfile" => {
                if block.identity_file.is_none() {
                    block.identity_file = Some(expand_home_path(value));
                }
            }
            "proxyjump" => {
                if block.proxy_jump.is_none() {
                    block.proxy_jump = parse_proxyjump_alias(value);
                }
            }
            "proxycommand" => block.unsupported_proxy_command = true,
            _ => {}
        }
    }

    flush_ssh_config_block(&mut entries, &aliases, &block);

    let mut hosts = entries.into_values().collect::<Vec<_>>();
    hosts.sort_by_key(|profile| profile.display_name().to_ascii_lowercase());
    hosts
}

fn flush_ssh_config_block(
    entries: &mut HashMap<String, HostProfile>,
    aliases: &[String],
    block: &SshConfigBlock,
) {
    if aliases.is_empty() || block.unsupported_proxy_command {
        return;
    }

    for alias in aliases {
        let host = block
            .host_name
            .clone()
            .unwrap_or_else(|| alias.trim().to_string());
        if host.is_empty() {
            continue;
        }

        let username = block
            .user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_default();
        let key_path = block
            .identity_file
            .clone()
            .filter(|path| !path.to_ascii_lowercase().ends_with(".pub"))
            .unwrap_or_default();

        let auth_mode = if key_path.is_empty() {
            AuthMode::Password
        } else {
            AuthMode::PrivateKey
        };

        entries.insert(
            imported_host_id(alias),
            HostProfile {
                id: imported_host_id(alias),
                label: alias.trim().to_string(),
                vault_id: Some(crate::models::DEFAULT_VAULT_ID.to_string()),
                group: String::new(),
                tags: Vec::new(),
                host,
                port: block.port.unwrap_or(22),
                username,
                auth_mode,
                key_path,
                identity_id: None,
                jump_host_id: block
                    .proxy_jump
                    .as_ref()
                    .map(|alias| imported_host_id(alias)),
                local_forward: None,
                password_credential_id: None,
                source: ProfileSource::SshConfig,
            },
        );
    }
}

fn parse_proxyjump_alias(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }

    let host_part = first.rsplit('@').next().unwrap_or(first);
    let alias = host_part.split(':').next().unwrap_or(host_part).trim();
    if alias.is_empty() || !is_importable_host_alias(alias) {
        return None;
    }

    Some(alias.to_string())
}

fn strip_ssh_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("")
}

fn split_ssh_directive(line: &str) -> Option<(&str, &str)> {
    let sep = line.find(|c: char| c == '=' || c.is_ascii_whitespace())?;
    let key = line[..sep].trim();
    let rest = line[sep..].trim_start_matches(|c: char| c == '=' || c.is_ascii_whitespace());
    let value = rest.trim();
    if key.is_empty() || value.is_empty() {
        None
    } else {
        Some((key, value))
    }
}

fn is_importable_host_alias(alias: &str) -> bool {
    !alias.is_empty() && !alias.contains('*') && !alias.contains('?') && !alias.starts_with('!')
}

fn expand_home_path(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("~/") {
        if let Some(home_dir) = dirs::home_dir() {
            return home_dir.join(stripped).display().to_string();
        }
    }

    value.to_string()
}

fn imported_host_id(alias: &str) -> String {
    let mut normalized = String::with_capacity(alias.len());
    for ch in alias.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push('-');
        }
    }

    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }

    format!("ssh-config-{}", normalized.trim_matches('-'))
}

#[derive(Default, Serialize, Deserialize)]
struct KnownHostsFile {
    #[serde(default)]
    entries: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    Existing,
    Added,
}

#[derive(Debug)]
pub struct KnownHostStore {
    path: PathBuf,
    entries: Mutex<HashMap<String, String>>,
}

impl KnownHostStore {
    pub fn load() -> Result<Self> {
        let path = known_hosts_file()?;
        if !path.exists() {
            return Ok(Self {
                path,
                entries: Mutex::new(HashMap::new()),
            });
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Unable to read {}", path.display()))?;
        let file: KnownHostsFile = serde_json::from_str(&content)
            .with_context(|| format!("Unable to parse {}", path.display()))?;

        Ok(Self {
            path,
            entries: Mutex::new(file.entries),
        })
    }

    pub fn verify_or_trust(&self, endpoint: &str, key: &str) -> Result<HostKeyDecision> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;

        match entries.get(endpoint) {
            Some(existing) if existing == key => Ok(HostKeyDecision::Existing),
            Some(_) => Err(anyhow::anyhow!(
                "Host key mismatch for {endpoint}. Remove the saved entry from {} if the server changed keys intentionally.",
                self.path.display()
            )),
            None => {
                entries.insert(endpoint.to_string(), key.to_string());
                let file = KnownHostsFile {
                    entries: entries.clone(),
                };
                let content = serde_json::to_string_pretty(&file)?;
                fs::write(&self.path, content)
                    .with_context(|| format!("Unable to write {}", self.path.display()))?;
                Ok(HostKeyDecision::Added)
            }
        }
    }

    pub fn entries(&self) -> Result<Vec<(String, String)>> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        let mut items = entries
            .iter()
            .map(|(endpoint, key)| (endpoint.clone(), key.clone()))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(items)
    }

    pub fn remove(&self, endpoint: &str) -> Result<bool> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        let removed = entries.remove(endpoint).is_some();
        if removed {
            let file = KnownHostsFile {
                entries: entries.clone(),
            };
            let content = serde_json::to_string_pretty(&file)?;
            fs::write(&self.path, content)
                .with_context(|| format!("Unable to write {}", self.path.display()))?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        detect_identity_kind, identity_priority, parse_ssh_config_hosts, should_skip_ssh_entry,
    };
    use crate::models::{AuthMode, ProfileSource};

    #[test]
    fn detects_supported_identity_formats() {
        assert_eq!(
            detect_identity_kind("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            Some("OpenSSH")
        );
        assert_eq!(
            detect_identity_kind("PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none"),
            Some("PuTTY PPK")
        );
        assert_eq!(
            detect_identity_kind("-----BEGIN RSA PRIVATE KEY-----\nabc"),
            Some("RSA PEM")
        );
        assert_eq!(
            detect_identity_kind("-----BEGIN PRIVATE KEY-----\nabc"),
            Some("PKCS#8")
        );
        assert_eq!(detect_identity_kind("not a key"), None);
    }

    #[test]
    fn skips_non_identity_ssh_files() {
        assert!(should_skip_ssh_entry("id_ed25519.pub"));
        assert!(should_skip_ssh_entry("known_hosts"));
        assert!(should_skip_ssh_entry("config"));
        assert!(!should_skip_ssh_entry("id_ed25519"));
        assert!(!should_skip_ssh_entry("runpod_key"));
    }

    #[test]
    fn prefers_common_default_identity_names() {
        assert!(identity_priority("id_ed25519") < identity_priority("id_rsa"));
        assert!(identity_priority("id_rsa") < identity_priority("vast"));
    }

    #[test]
    fn parses_simple_ssh_config_hosts() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host app-prod
  HostName 203.0.113.10
  User ubuntu
  IdentityFile ~/.ssh/id_ed25519
  Port 2222

Host wildcard-*
  HostName ignored.example.com
  User root

Host tunnel-box
  ProxyCommand ssh jumpbox -W %h:%p
  HostName 10.0.0.7
  User root
"#,
        );

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].label, "app-prod");
        assert_eq!(hosts[0].host, "203.0.113.10");
        assert_eq!(hosts[0].username, "ubuntu");
        assert_eq!(hosts[0].port, 2222);
        assert_eq!(hosts[0].auth_mode, AuthMode::PrivateKey);
        assert_eq!(hosts[0].source, ProfileSource::SshConfig);
        assert!(hosts[0].key_path.ends_with("/.ssh/id_ed25519"));
    }

    #[test]
    fn parses_ssh_config_with_equals_delimiter() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host eq-host
  HostName=10.0.0.1
  User=deploy
  Port=3022

Host eq-spaced
  HostName = 10.0.0.2
  User = admin
"#,
        );

        assert_eq!(hosts.len(), 2);
        let eq_host = hosts.iter().find(|h| h.label == "eq-host").unwrap();
        assert_eq!(eq_host.host, "10.0.0.1");
        assert_eq!(eq_host.username, "deploy");
        assert_eq!(eq_host.port, 3022);

        let eq_spaced = hosts.iter().find(|h| h.label == "eq-spaced").unwrap();
        assert_eq!(eq_spaced.host, "10.0.0.2");
        assert_eq!(eq_spaced.username, "admin");
    }

    #[test]
    fn parses_proxyjump_alias_as_jump_host_reference() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host bastion
  HostName 198.51.100.10
  User ubuntu
  IdentityFile ~/.ssh/id_ed25519

Host app-prod
  HostName 10.0.0.15
  User deploy
  ProxyJump bastion
"#,
        );

        assert_eq!(hosts.len(), 2);
        let app = hosts.iter().find(|host| host.label == "app-prod").unwrap();
        assert_eq!(app.jump_host_id.as_deref(), Some("ssh-config-bastion"));
    }
}
