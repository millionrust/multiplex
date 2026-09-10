//! Inert projection of an authenticated desktop profile, not a connection request.
use super::DesktopHostProfile;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const COLLECTION: &str = "desktop-profiles";
const MAX_HOST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplicatedHostReviewError {
    Invalid,
}

impl std::fmt::Display for ReplicatedHostReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("replicated host is invalid or unsupported")
    }
}
impl std::error::Error for ReplicatedHostReviewError {}

/// Intentionally has no conversion into MobileHost. Credentials, routes, startup
/// actions, environment and terminal options require separate local configuration.
pub struct ReplicatedHostReview {
    pub stable_id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema_version: u16,
    stable_id: String,
    value: DesktopHostProfile,
}

pub fn review_replicated_host(
    plaintext: &[u8],
    collection: &str,
    record_id: &str,
) -> Result<ReplicatedHostReview, ReplicatedHostReviewError> {
    let invalid = ReplicatedHostReviewError::Invalid;
    if collection != COLLECTION || plaintext.is_empty() || plaintext.len() > MAX_HOST_BYTES {
        return Err(invalid);
    }
    let record: Envelope = serde_json::from_slice(plaintext).map_err(|_| invalid)?;
    if record.schema_version != 1
        || record.stable_id != record.value.id
        || !plain_text(&record.stable_id, 256)
        || !plain_text(&record.value.label, 256)
        || !plain_text(&record.value.host, 253)
        || !plain_text(&record.value.username, 256)
        || record.value.port == 0
        || record.value.host.chars().any(char::is_whitespace)
        || record.value.host.starts_with('-')
        || record.value.host.contains(['/', '\\', '@', '?', '#'])
    {
        return Err(invalid);
    }
    let mut digest = Sha256::new();
    digest.update(b"termirust.desktop-replication-record.v1\0");
    digest.update(COLLECTION.as_bytes());
    digest.update([0]);
    digest.update(record.stable_id.as_bytes());
    let expected = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if expected != record_id {
        return Err(invalid);
    }
    Ok(ReplicatedHostReview {
        stable_id: record.stable_id,
        label: record.value.label,
        host: record.value.host,
        port: record.value.port,
        username: record.value.username,
    })
}

fn plain_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> (Value, String) {
        let value = json!({"schema_version":1,"stable_id":"profile-1","value":{
            "id":"profile-1","label":"Production","host":"server.example","port":22,
            "username":"jacob","auth_mode":"private_key","identity_id":"desktop-key",
            "password_credential_id":"desktop-password", "key_path":"/private/key",
            "jump_host_id":"bastion", "startup_command":"touch /tmp/never-run",
            "environment":[["TOKEN","never-forward"]], "persistent_session":true,
            "persistent_session_detach_others":true }});
        let id =
            Sha256::digest(b"termirust.desktop-replication-record.v1\0desktop-profiles\0profile-1")
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        (value, id)
    }

    #[test]
    fn desktop_profile_projects_only_inert_connection_details() {
        let (value, id) = fixture();
        let review =
            review_replicated_host(&serde_json::to_vec(&value).unwrap(), COLLECTION, &id).unwrap();
        assert_eq!(review.stable_id, "profile-1");
        assert_eq!(review.label, "Production");
        assert_eq!(review.host, "server.example");
        assert_eq!(review.port, 22);
        assert_eq!(review.username, "jacob");
    }

    #[test]
    fn identity_schema_and_required_fields_are_checked() {
        let (original, id) = fixture();
        for path in [
            "schema", "stable", "profile", "port", "host", "user", "auth",
        ] {
            let mut value = original.clone();
            match path {
                "schema" => value["schema_version"] = json!(2),
                "stable" => value["stable_id"] = json!("different"),
                "profile" => value["value"]["id"] = json!("different"),
                "port" => value["value"]["port"] = json!(0),
                "host" => value["value"]["host"] = json!("ssh://wrong/path"),
                "user" => value["value"]["username"] = json!("\u{1b}[31m"),
                "auth" => value["value"]["auth_mode"] = json!("unsupported"),
                _ => unreachable!(),
            }
            assert!(
                review_replicated_host(&serde_json::to_vec(&value).unwrap(), COLLECTION, &id)
                    .is_err(),
                "{path}"
            );
        }
        let bytes = serde_json::to_vec(&original).unwrap();
        assert!(review_replicated_host(&bytes, "desktop-identities", &id).is_err());
        assert!(review_replicated_host(&bytes, COLLECTION, "wrong").is_err());
        assert!(review_replicated_host(&vec![0; MAX_HOST_BYTES + 1], COLLECTION, &id).is_err());
    }

    #[test]
    fn duplicate_required_fields_and_unknown_envelope_fields_fail() {
        let (_, id) = fixture();
        let duplicate = br#"{"schema_version":1,"stable_id":"profile-1","value":{"id":"profile-1","label":"One","label":"Two","host":"server.example","username":"jacob"}}"#;
        assert!(review_replicated_host(duplicate, COLLECTION, &id).is_err());
        let (mut value, _) = fixture();
        value["future_authority"] = json!(true);
        assert!(
            review_replicated_host(&serde_json::to_vec(&value).unwrap(), COLLECTION, &id).is_err()
        );
    }

    #[test]
    fn default_port_ipv6_and_unicode_labels_are_supported() {
        let (mut value, id) = fixture();
        value["value"].as_object_mut().unwrap().remove("port");
        value["value"]["host"] = json!("2001:db8::1");
        value["value"]["label"] = json!("Serveur \u{00e9}quipe");
        let host =
            review_replicated_host(&serde_json::to_vec(&value).unwrap(), COLLECTION, &id).unwrap();
        assert_eq!(host.port, 22);
        assert_eq!(host.host, "2001:db8::1");
        assert_eq!(host.label, "Serveur \u{00e9}quipe");
        value["value"]["label"] = json!("misleading\u{202e}label");
        assert!(
            review_replicated_host(&serde_json::to_vec(&value).unwrap(), COLLECTION, &id).is_err()
        );
    }
}
