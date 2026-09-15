use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use termirust_browser::ApprovedOrigin;

use crate::backend::{ActionRequest, SourceError};
use termirust_cli::Cancellation;

const POLICY_FILE: &str = "action-policy.json";
const RECEIPT_FILE: &str = "action-receipts.json";
const LOCK_FILE: &str = "action-receipts.lock";
const AUDIT_FILE: &str = "action-audit.jsonl";
const MAX_POLICY_BYTES: u64 = 64 * 1024;
const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RECEIPTS: usize = 512;
const MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SCOPE_IDS: usize = 256;
const REVOCATION_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPolicy {
    pub schema_version: u16,
    pub grant_id: String,
    pub expires_at_unix_ms: u64,
    pub actions: Vec<ApprovedAction>,
    pub project_ids: Vec<String>,
    pub session_ids: Vec<String>,
    #[serde(default)]
    pub browser_origins: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovedAction {
    Launch,
    Wait,
    Attach,
    Cancel,
    Input,
    ResumeReview,
    Resume,
    CreateArtifact,
    BrowserText,
    BrowserScreenshot,
    BrowserDownload,
}

impl ApprovedAction {
    pub const ALL: [Self; 11] = [
        Self::Launch,
        Self::Wait,
        Self::Attach,
        Self::Cancel,
        Self::Input,
        Self::ResumeReview,
        Self::Resume,
        Self::CreateArtifact,
        Self::BrowserText,
        Self::BrowserScreenshot,
        Self::BrowserDownload,
    ];

    pub const fn policy_name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Wait => "wait",
            Self::Attach => "attach",
            Self::Cancel => "cancel",
            Self::Input => "input",
            Self::ResumeReview => "resume_review",
            Self::Resume => "resume",
            Self::CreateArtifact => "create_artifact",
            Self::BrowserText => "browser_text",
            Self::BrowserScreenshot => "browser_screenshot",
            Self::BrowserDownload => "browser_download",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.policy_name() == value)
    }

    fn for_request(request: &ActionRequest) -> Self {
        match request {
            ActionRequest::Launch { .. } => Self::Launch,
            ActionRequest::Wait { .. } => Self::Wait,
            ActionRequest::Attach { .. } => Self::Attach,
            ActionRequest::Cancel { .. } => Self::Cancel,
            ActionRequest::Input { .. } => Self::Input,
            ActionRequest::ResumeReview { .. } => Self::ResumeReview,
            ActionRequest::Resume { .. } => Self::Resume,
            ActionRequest::CreateArtifact { .. } => Self::CreateArtifact,
            ActionRequest::BrowserText { .. } => Self::BrowserText,
            ActionRequest::BrowserScreenshot { .. } => Self::BrowserScreenshot,
            ActionRequest::BrowserDownload { .. } => Self::BrowserDownload,
        }
    }
}

pub struct ActionPolicyStore {
    root: PathBuf,
    process_lock: Mutex<()>,
}

impl std::fmt::Debug for ActionPolicyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActionPolicyStore")
            .field("root", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ActionPolicyStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            process_lock: Mutex::new(()),
        }
    }

    pub fn policy_path(&self) -> PathBuf {
        self.root.join(POLICY_FILE)
    }

    pub fn write_policy(&self, policy: &ActionPolicy) -> Result<(), SourceError> {
        validate_policy(policy)?;
        let bytes = serde_json::to_vec_pretty(policy).map_err(|_| SourceError::InvalidInput)?;
        if bytes.len() as u64 > MAX_POLICY_BYTES {
            return Err(SourceError::ResourceLimit);
        }
        self.ensure_root()?;
        atomic_private_write(&self.policy_path(), &bytes)
    }

    pub fn revoke(&self) -> Result<(), SourceError> {
        match fs::symlink_metadata(self.policy_path()) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(SourceError::PermissionDenied)
            }
            Ok(_) => fs::remove_file(self.policy_path()).map_err(map_io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(map_io(error)),
        }
    }

    pub fn run<F>(
        &self,
        request: &ActionRequest,
        parent: &Cancellation,
        operation: F,
    ) -> Result<Value, SourceError>
    where
        F: FnOnce(&Cancellation) -> Result<Value, SourceError>,
    {
        self.run_with_policy(request, parent, |cancellation, _policy| {
            operation(cancellation)
        })
    }

    pub fn run_with_policy<F>(
        &self,
        request: &ActionRequest,
        parent: &Cancellation,
        operation: F,
    ) -> Result<Value, SourceError>
    where
        F: FnOnce(&Cancellation, &ActionPolicy) -> Result<Value, SourceError>,
    {
        let policy = match self.authorize(request) {
            Ok(value) => value,
            Err(error) => {
                let _ = self.audit(request, None, outcome_name(error));
                return Err(error);
            }
        };
        self.audit(request, Some(&policy.grant_id), "started")?;
        let child = Cancellation::default();
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let monitor_done = done.clone();
        let monitor_child = child.clone();
        let monitor_parent = parent.clone();
        let monitor_root = self.root.clone();
        let monitor_request = request.clone();
        let grant_id = policy.grant_id.clone();
        let monitor = std::thread::spawn(move || {
            let monitor_store = Self::new(monitor_root);
            while !monitor_done.load(Ordering::Acquire) {
                let still_authorized = monitor_store
                    .authorize(&monitor_request)
                    .is_ok_and(|current| current.grant_id == grant_id);
                if monitor_parent.is_cancelled() || !still_authorized {
                    monitor_child.cancel();
                    break;
                }
                std::thread::sleep(REVOCATION_POLL);
            }
        });

        let result = if request.command_id().is_some() {
            self.run_idempotent(request, &policy.grant_id, &child, |cancellation| {
                operation(cancellation, &policy)
            })
        } else {
            operation(&child, &policy)
        };
        done.store(true, Ordering::Release);
        let _ = monitor.join();
        if parent.is_cancelled() || child.is_cancelled() {
            let _ = self.audit(request, Some(&policy.grant_id), "cancelled");
            return Err(SourceError::Cancelled);
        }
        let outcome = if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let _ = self.audit(request, Some(&policy.grant_id), outcome);
        result
    }

    fn run_idempotent<F>(
        &self,
        request: &ActionRequest,
        grant_id: &str,
        cancellation: &Cancellation,
        operation: F,
    ) -> Result<Value, SourceError>
    where
        F: FnOnce(&Cancellation) -> Result<Value, SourceError>,
    {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| SourceError::Unavailable)?;
        self.ensure_root()?;
        let lock = open_private_rw(&self.root.join(LOCK_FILE))?;
        lock.lock_exclusive().map_err(map_io)?;
        let command_id = request.command_id().ok_or(SourceError::InvalidInput)?;
        let fingerprint = request.fingerprint();
        let mut receipts = self.load_receipts()?;
        if let Some(receipt) = receipts
            .receipts
            .iter()
            .find(|receipt| receipt.command_id == command_id)
        {
            if receipt.action != request.kind() || receipt.fingerprint != fingerprint {
                return Err(SourceError::Inconsistent);
            }
            return Ok(receipt.result.clone());
        }
        if cancellation.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let result = operation(cancellation)?;
        receipts.receipts.push(Receipt {
            command_id: command_id.to_string(),
            action: request.kind().to_string(),
            fingerprint,
            grant_id: grant_id.to_string(),
            occurred_at_unix_ms: now_millis(),
            result: result.clone(),
        });
        if receipts.receipts.len() > MAX_RECEIPTS {
            let remove = receipts.receipts.len() - MAX_RECEIPTS;
            receipts.receipts.drain(..remove);
        }
        let bytes = serde_json::to_vec(&receipts).map_err(|_| SourceError::Inconsistent)?;
        if bytes.len() as u64 > MAX_RECEIPT_BYTES {
            return Err(SourceError::ResourceLimit);
        }
        atomic_private_write(&self.root.join(RECEIPT_FILE), &bytes)?;
        Ok(result)
    }

    fn authorize(&self, request: &ActionRequest) -> Result<ActionPolicy, SourceError> {
        let policy = self.load_policy()?;
        validate_policy(&policy)?;
        if policy.expires_at_unix_ms <= now_millis()
            || !policy
                .actions
                .contains(&ApprovedAction::for_request(request))
        {
            return Err(SourceError::PermissionDenied);
        }
        let allowed_scope = match (request.project_scope(), request.session_scope()) {
            (Some(id), None) => policy.project_ids.iter().any(|candidate| candidate == id),
            (None, Some(id)) => policy.session_ids.iter().any(|candidate| candidate == id),
            _ => false,
        };
        if !allowed_scope {
            return Err(SourceError::PermissionDenied);
        }
        if let Some(url) = request.browser_url()
            && !policy.browser_origins.iter().any(|value| {
                ApprovedOrigin::parse(value).is_ok_and(|origin| origin.permits_url(url))
            })
        {
            return Err(SourceError::PermissionDenied);
        }
        Ok(policy)
    }

    fn load_policy(&self) -> Result<ActionPolicy, SourceError> {
        let bytes = read_private_bounded(&self.policy_path(), MAX_POLICY_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|_| SourceError::PermissionDenied)
    }

    fn load_receipts(&self) -> Result<ReceiptDocument, SourceError> {
        match fs::symlink_metadata(self.root.join(RECEIPT_FILE)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(ReceiptDocument::default())
            }
            Err(error) => Err(map_io(error)),
            Ok(_) => {
                let bytes = read_private_bounded(&self.root.join(RECEIPT_FILE), MAX_RECEIPT_BYTES)?;
                let document = serde_json::from_slice::<ReceiptDocument>(&bytes)
                    .map_err(|_| SourceError::Inconsistent)?;
                if document.schema_version != 1 || document.receipts.len() > MAX_RECEIPTS {
                    return Err(SourceError::Inconsistent);
                }
                Ok(document)
            }
        }
    }

    fn audit(
        &self,
        request: &ActionRequest,
        grant_id: Option<&str>,
        outcome: &'static str,
    ) -> Result<(), SourceError> {
        self.ensure_root()?;
        let path = self.root.join(AUDIT_FILE);
        if fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() >= MAX_AUDIT_BYTES
        }) {
            return Err(SourceError::ResourceLimit);
        }
        let record = json!({
            "schema_version": 1,
            "occurred_at_unix_ms": now_millis(),
            "grant_id": grant_id,
            "command_id": request.command_id(),
            "action": request.kind(),
            "scope_kind": if request.project_scope().is_some() { "project" } else { "session" },
            "outcome": outcome,
        });
        let mut bytes = serde_json::to_vec(&record).map_err(|_| SourceError::Inconsistent)?;
        bytes.push(b'\n');
        let mut file = open_private_append(&path)?;
        file.write_all(&bytes).map_err(map_io)?;
        file.sync_data().map_err(map_io)
    }

    fn ensure_root(&self) -> Result<(), SourceError> {
        ensure_private_directory(&self.root)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptDocument {
    #[serde(default = "receipt_schema")]
    schema_version: u16,
    #[serde(default)]
    receipts: Vec<Receipt>,
}

impl Default for ReceiptDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            receipts: Vec::new(),
        }
    }
}

const fn receipt_schema() -> u16 {
    1
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    command_id: String,
    action: String,
    fingerprint: String,
    grant_id: String,
    occurred_at_unix_ms: u64,
    result: Value,
}

fn validate_policy(policy: &ActionPolicy) -> Result<(), SourceError> {
    let now = now_millis();
    if policy.schema_version != 1
        || policy
            .grant_id
            .parse::<termirust_domain::CommandId>()
            .is_err()
        || policy.actions.is_empty()
        || policy.actions.len() > ApprovedAction::ALL.len()
        || policy.project_ids.len() > MAX_SCOPE_IDS
        || policy.session_ids.len() > MAX_SCOPE_IDS
        || policy.browser_origins.len() > 32
        || policy.expires_at_unix_ms <= now
        || policy.expires_at_unix_ms > now.saturating_add(24 * 60 * 60 * 1_000)
        || policy
            .project_ids
            .iter()
            .any(|id| id.parse::<termirust_domain::ProjectId>().is_err())
        || policy
            .session_ids
            .iter()
            .any(|id| id.parse::<termirust_domain::HostedSessionId>().is_err())
    {
        return Err(SourceError::InvalidInput);
    }
    let unique = policy.actions.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != policy.actions.len() {
        return Err(SourceError::InvalidInput);
    }
    let origins = policy
        .browser_origins
        .iter()
        .map(|value| ApprovedOrigin::parse(value).map(|origin| origin.as_string()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| SourceError::InvalidInput)?;
    if origins.len() != policy.browser_origins.len()
        || policy
            .browser_origins
            .iter()
            .any(|value| !origins.contains(value))
    {
        return Err(SourceError::InvalidInput);
    }
    let browser_action = policy.actions.iter().any(|action| {
        matches!(
            action,
            ApprovedAction::BrowserText
                | ApprovedAction::BrowserScreenshot
                | ApprovedAction::BrowserDownload
        )
    });
    if browser_action != !policy.browser_origins.is_empty() {
        return Err(SourceError::InvalidInput);
    }
    Ok(())
}

fn read_private_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, SourceError> {
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        return Err(SourceError::PermissionDenied);
    }
    verify_private_mode(&metadata)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(map_io)?
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(map_io)?;
    if bytes.len() as u64 > maximum {
        return Err(SourceError::ResourceLimit);
    }
    Ok(bytes)
}

fn ensure_private_directory(path: &Path) -> Result<(), SourceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(SourceError::PermissionDenied);
            }
            verify_private_mode(&metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(map_io)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_io)?;
            Ok(())
        }
        Err(error) => Err(map_io(error)),
    }
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), SourceError> {
    let parent = path.parent().ok_or(SourceError::PermissionDenied)?;
    ensure_private_directory(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(SourceError::PermissionDenied);
    }
    let temporary = parent.join(format!(".mcp-{}.tmp", rand::random::<u64>()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).map_err(map_io)?;
    let result = (|| {
        file.write_all(bytes).map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        fs::rename(&temporary, path).map_err(map_io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn open_private_rw(path: &Path) -> Result<File, SourceError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        verify_existing_private_file(&metadata)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).map_err(map_io)
}

fn open_private_append(path: &Path) -> Result<File, SourceError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        verify_existing_private_file(&metadata)?;
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).map_err(map_io)
}

fn verify_existing_private_file(metadata: &fs::Metadata) -> Result<(), SourceError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SourceError::PermissionDenied);
    }
    verify_private_mode(metadata)
}

#[cfg(unix)]
fn verify_private_mode(metadata: &fs::Metadata) -> Result<(), SourceError> {
    if metadata.permissions().mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(SourceError::PermissionDenied)
    }
}

#[cfg(not(unix))]
fn verify_private_mode(_: &fs::Metadata) -> Result<(), SourceError> {
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn outcome_name(error: SourceError) -> &'static str {
    match error {
        SourceError::Cancelled => "cancelled",
        SourceError::PermissionDenied => "denied",
        _ => "failed",
    }
}

fn map_io(error: std::io::Error) -> SourceError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => SourceError::PermissionDenied,
        std::io::ErrorKind::NotFound => SourceError::PermissionDenied,
        _ => SourceError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ActionRequest;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    const PROJECT: &str = "00000000-0000-0000-0000-000000000001";
    const SESSION: &str = "00000000-0000-0000-0000-000000000002";
    const COMMAND: &str = "00000000-0000-0000-0000-000000000003";

    fn policy(actions: Vec<ApprovedAction>) -> ActionPolicy {
        ActionPolicy {
            schema_version: 1,
            grant_id: "00000000-0000-0000-0000-000000000004".to_string(),
            expires_at_unix_ms: now_millis().saturating_add(60_000),
            actions,
            project_ids: vec![PROJECT.to_string()],
            session_ids: vec![SESSION.to_string()],
            browser_origins: Vec::new(),
        }
    }

    fn input(value: &str) -> ActionRequest {
        ActionRequest::Input {
            command_id: COMMAND.to_string(),
            session_id: SESSION.to_string(),
            input: value.to_string(),
        }
    }

    #[test]
    fn scoped_policy_persists_redacted_idempotent_receipts() {
        let temp = tempfile::tempdir().expect("temporary policy root");
        let store = ActionPolicyStore::new(temp.path().join("mcp"));
        store
            .write_policy(&policy(vec![ApprovedAction::Input]))
            .expect("write policy");
        let calls = AtomicUsize::new(0);
        let first = store
            .run(&input("secret-payload\n"), &Cancellation::default(), |_| {
                calls.fetch_add(1, Ordering::AcqRel);
                Ok(json!({ "applied": true }))
            })
            .expect("first action");
        let replay = store
            .run(&input("secret-payload\n"), &Cancellation::default(), |_| {
                calls.fetch_add(1, Ordering::AcqRel);
                Ok(json!({ "applied": true }))
            })
            .expect("idempotent replay");
        assert_eq!(first, replay);
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(
            store.run(&input("different"), &Cancellation::default(), |_| {
                Ok(json!({}))
            }),
            Err(SourceError::Inconsistent)
        );
        let receipts = fs::read_to_string(store.root.join(RECEIPT_FILE)).expect("receipts");
        let audit = fs::read_to_string(store.root.join(AUDIT_FILE)).expect("audit");
        assert!(!receipts.contains("secret-payload"));
        assert!(!audit.contains("secret-payload"));
        assert!(audit.contains("sessions.input"));
    }

    #[test]
    fn wrong_scope_is_denied_and_policy_removal_cancels_in_flight_action() {
        let temp = tempfile::tempdir().expect("temporary policy root");
        let store = Arc::new(ActionPolicyStore::new(temp.path().join("mcp")));
        store
            .write_policy(&policy(vec![ApprovedAction::Wait]))
            .expect("write policy");
        let denied = ActionRequest::Wait {
            session_id: "00000000-0000-0000-0000-000000000099".to_string(),
            state: Some("done".to_string()),
            activity: None,
            timeout_ms: 1_000,
        };
        assert_eq!(
            store.run(&denied, &Cancellation::default(), |_| Ok(json!({}))),
            Err(SourceError::PermissionDenied)
        );

        let started = Arc::new(AtomicBool::new(false));
        let worker_store = Arc::clone(&store);
        let worker_started = Arc::clone(&started);
        let request = ActionRequest::Wait {
            session_id: SESSION.to_string(),
            state: Some("done".to_string()),
            activity: None,
            timeout_ms: 5_000,
        };
        let worker = std::thread::spawn(move || {
            worker_store.run(&request, &Cancellation::default(), |cancellation| {
                worker_started.store(true, Ordering::Release);
                while !cancellation.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(SourceError::Cancelled)
            })
        });
        while !started.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        store.revoke().expect("revoke policy");
        assert_eq!(
            worker.join().expect("join worker"),
            Err(SourceError::Cancelled)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_policy_fails_closed() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary policy root");
        let root = temp.path().join("mcp");
        fs::create_dir_all(&root).expect("policy root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("root mode");
        let outside = temp.path().join("outside.json");
        fs::write(
            &outside,
            serde_json::to_vec(&policy(vec![ApprovedAction::Input])).expect("policy JSON"),
        )
        .expect("outside policy");
        symlink(&outside, root.join(POLICY_FILE)).expect("policy symlink");
        let store = ActionPolicyStore::new(root);
        assert_eq!(
            store.run(&input("value"), &Cancellation::default(), |_| Ok(json!({}))),
            Err(SourceError::PermissionDenied)
        );
    }

    #[test]
    fn browser_policy_requires_an_exact_origin_and_keeps_urls_out_of_audit() {
        let temp = tempfile::tempdir().expect("temporary policy root");
        let store = ActionPolicyStore::new(temp.path().join("mcp"));
        let mut browser_policy = policy(vec![ApprovedAction::BrowserText]);
        browser_policy.browser_origins = vec!["https://example.com".to_string()];
        store.write_policy(&browser_policy).expect("browser policy");
        let request = ActionRequest::BrowserText {
            command_id: COMMAND.to_string(),
            session_id: SESSION.to_string(),
            display_name: "page.txt".to_string(),
            url: "https://example.com/reviewed".to_string(),
        };
        store
            .run_with_policy(&request, &Cancellation::default(), |_, policy| {
                assert_eq!(policy.browser_origins, ["https://example.com"]);
                Ok(json!({ "artifact_id": COMMAND }))
            })
            .expect("approved browser action");
        let denied = ActionRequest::BrowserText {
            command_id: "00000000-0000-0000-0000-000000000005".to_string(),
            session_id: SESSION.to_string(),
            display_name: "page.txt".to_string(),
            url: "https://example.org/not-approved".to_string(),
        };
        assert_eq!(
            store.run(&denied, &Cancellation::default(), |_| Ok(json!({}))),
            Err(SourceError::PermissionDenied)
        );
        let audit = fs::read_to_string(store.root.join(AUDIT_FILE)).expect("audit");
        let receipts = fs::read_to_string(store.root.join(RECEIPT_FILE)).expect("receipts");
        assert!(!audit.contains("example.com"));
        assert!(!receipts.contains("example.com"));
    }
}
