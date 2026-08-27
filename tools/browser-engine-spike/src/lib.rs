use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub const REPORT_SCHEMA_VERSION: u32 = 1;
pub const FIXTURE_SEED: u64 = 0x1901_2026;
pub const MIN_RUNS: u32 = 10;
pub const MAX_RUNS: u32 = 100;
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_REPORT_BYTES: u64 = 256 * 1024;

pub const MANDATORY_GATES: [&str; 15] = [
    "os_user_profile_isolation",
    "owned_process_termination",
    "navigation_interception",
    "subresource_interception",
    "redirect_interception",
    "iframe_interception",
    "popup_interception",
    "websocket_interception",
    "service_worker_interception",
    "download_interception",
    "stale_document_detection",
    "cancellation_within_30s",
    "compatible_license",
    "maintained_release_and_security_route",
    "reproducible_packaging_path",
];

pub const REQUIRED_FIXTURES: [&str; 11] = [
    "redirect",
    "rebinding",
    "iframe",
    "popup",
    "websocket",
    "service_worker",
    "download",
    "huge_dom",
    "stalled_response",
    "crash",
    "stale_element",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureManifest {
    pub schema_version: u32,
    pub seed: u64,
    pub fixtures: Vec<FixtureSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSpec {
    pub id: String,
    pub path: String,
    pub file: Option<String>,
    pub behavior: FixtureBehavior,
    pub expected_status: u16,
    pub expected_marker: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureBehavior {
    Static,
    Redirect,
    Stalled,
    Download,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateEvidence {
    pub status: EvidenceStatus,
    pub evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRecord {
    pub label: String,
    pub url: String,
    pub accessed: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserPin {
    pub product: String,
    pub version: String,
    pub platform: String,
    pub archive_sha256: String,
    pub driver_archive_sha256: Option<String>,
    pub manifest_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurements {
    pub cold_start_ms: Option<u64>,
    pub warm_start_ms: Vec<u64>,
    pub idle_rss_bytes: Option<u64>,
    pub idle_cpu_percent: Option<f64>,
    pub binary_bytes: Option<u64>,
    pub cancellation_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateReport {
    pub id: String,
    pub controller_version: String,
    pub controller_commit: Option<String>,
    pub controller_license: String,
    pub transitive_license_result: EvidenceStatus,
    pub security_scan_date: String,
    pub security_process: String,
    pub adapter_compiled: bool,
    pub runtime_available: bool,
    pub browser: BrowserPin,
    pub sources: Vec<SourceRecord>,
    pub mandatory_gates: BTreeMap<String, GateEvidence>,
    pub measurements: Measurements,
    pub unresolved_risks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureResult {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HarnessReport {
    pub loopback_only: bool,
    pub empty_child_environment: bool,
    pub owned_process_group_verified: bool,
    pub unrelated_process_survived: bool,
    pub descendant_terminated: bool,
    pub temporary_profile_removed: bool,
    pub fixture_results: Vec<FixtureResult>,
    pub warm_run_ms: Vec<u64>,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Go,
    ConditionalGo,
    NoGo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionReport {
    pub kind: DecisionKind,
    pub selected_candidate: Option<String>,
    pub blockers: Vec<String>,
    pub review_owner: String,
    pub review_date: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineReport {
    pub os: String,
    pub arch: String,
    pub rustc: String,
    pub browser_installed: bool,
    pub driver_installed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineProbeReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub mode: String,
    pub fixture_seed: u64,
    pub fixture_manifest_sha256: String,
    pub runs: u32,
    pub machine: MachineReport,
    pub harness: HarnessReport,
    pub candidates: Vec<CandidateReport>,
    pub decision: DecisionReport,
}

#[derive(Debug)]
pub enum SpikeError {
    InvalidArgument(&'static str),
    Io(io::Error),
    Json(serde_json::Error),
    InvalidFixture(String),
    Probe(String),
}

impl std::fmt::Display for SpikeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "I/O failure: {error}"),
            Self::Json(error) => write!(formatter, "JSON failure: {error}"),
            Self::InvalidFixture(message) => write!(formatter, "invalid fixture: {message}"),
            Self::Probe(message) => write!(formatter, "probe failure: {message}"),
        }
    }
}

impl std::error::Error for SpikeError {}

impl From<io::Error> for SpikeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SpikeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn load_fixture_manifest(root: &Path) -> Result<(FixtureManifest, String), SpikeError> {
    let manifest_path = root.join("index.json");
    let metadata = fs::symlink_metadata(&manifest_path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(SpikeError::InvalidFixture(
            "index.json must be a regular file".to_string(),
        ));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(SpikeError::InvalidFixture(
            "index.json exceeds 64 KiB".to_string(),
        ));
    }
    let bytes = fs::read(&manifest_path)?;
    let manifest: FixtureManifest = serde_json::from_slice(&bytes)?;
    validate_manifest(root, &manifest)?;
    Ok((manifest, hex_sha256(&bytes)))
}

fn validate_manifest(root: &Path, manifest: &FixtureManifest) -> Result<(), SpikeError> {
    if manifest.schema_version != 1 || manifest.seed != FIXTURE_SEED {
        return Err(SpikeError::InvalidFixture(
            "unexpected schema version or seed".to_string(),
        ));
    }
    if manifest.fixtures.len() != REQUIRED_FIXTURES.len() {
        return Err(SpikeError::InvalidFixture(
            "fixture count does not match the frozen corpus".to_string(),
        ));
    }
    let required = REQUIRED_FIXTURES.into_iter().collect::<BTreeSet<_>>();
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for fixture in &manifest.fixtures {
        if !required.contains(fixture.id.as_str()) || !ids.insert(fixture.id.as_str()) {
            return Err(SpikeError::InvalidFixture(format!(
                "unexpected or duplicate id {}",
                fixture.id
            )));
        }
        if !fixture.path.starts_with('/')
            || fixture.path.contains("..")
            || !paths.insert(fixture.path.as_str())
        {
            return Err(SpikeError::InvalidFixture(format!(
                "unsafe or duplicate path {}",
                fixture.path
            )));
        }
        if fixture.expected_marker.len() > 128 || !fixture.expected_marker.is_ascii() {
            return Err(SpikeError::InvalidFixture(format!(
                "unsafe marker for {}",
                fixture.id
            )));
        }
        if let Some(file_name) = &fixture.file {
            if file_name.contains('/') || file_name.contains("..") || file_name.len() > 96 {
                return Err(SpikeError::InvalidFixture(format!(
                    "unsafe file name for {}",
                    fixture.id
                )));
            }
            validate_fixture_file(root, file_name, &fixture.id)?;
        }
    }
    validate_fixture_file(root, "service-worker.js", "service_worker auxiliary")?;
    if ids != required {
        return Err(SpikeError::InvalidFixture(
            "required fixture ids are incomplete".to_string(),
        ));
    }
    Ok(())
}

fn validate_fixture_file(root: &Path, file_name: &str, fixture_id: &str) -> Result<(), SpikeError> {
    let path = root.join(file_name);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RESPONSE_BYTES as u64
    {
        return Err(SpikeError::InvalidFixture(format!(
            "unsafe fixture file for {fixture_id}"
        )));
    }
    Ok(())
}

struct FixtureServer {
    address: SocketAddrV4,
    cancelled: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start(root: PathBuf, manifest: FixtureManifest) -> Result<Self, SpikeError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let address = match listener.local_addr()? {
            std::net::SocketAddr::V4(address) if address.ip().is_loopback() => address,
            _ => {
                return Err(SpikeError::Probe(
                    "fixture listener was not loopback".to_string(),
                ));
            }
        };
        listener.set_nonblocking(true)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_cancelled = Arc::clone(&cancelled);
        let thread = thread::Builder::new()
            .name("browser-fixture-loopback".to_string())
            .spawn(move || {
                while !thread_cancelled.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = handle_fixture_request(&mut stream, &root, &manifest);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            address,
            cancelled,
            thread: Some(thread),
        })
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle_fixture_request(
    stream: &mut TcpStream,
    root: &Path,
    manifest: &FixtureManifest,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    let mut request = [0_u8; 8192];
    let count = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..count]);
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_ascii_whitespace();
    if parts.next() != Some("GET") {
        return write_response(stream, 405, &[], b"method-not-allowed");
    }
    let path = parts.next().unwrap_or_default();
    if path == "/socket" {
        return write_response(stream, 426, &[], b"fixture-websocket-upgrade");
    }
    if path == "/service-worker-script" {
        let body = fs::read(root.join("service-worker.js"))?;
        return write_response(
            stream,
            200,
            &[("Content-Type", "application/javascript")],
            &body,
        );
    }
    let Some(fixture) = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.path == path)
    else {
        return write_response(stream, 404, &[], b"not-found");
    };
    match fixture.behavior {
        FixtureBehavior::Redirect => write_response(
            stream,
            302,
            &[("Location", "/stale")],
            fixture.expected_marker.as_bytes(),
        ),
        FixtureBehavior::Stalled => {
            thread::sleep(Duration::from_millis(150));
            write_response(stream, 200, &[], fixture.expected_marker.as_bytes())
        }
        FixtureBehavior::Download => {
            let body = fs::read(root.join(fixture.file.as_deref().unwrap_or_default()))?;
            write_response(
                stream,
                200,
                &[("Content-Disposition", "attachment; filename=synthetic.bin")],
                &body,
            )
        }
        FixtureBehavior::Static => {
            let body = fs::read(root.join(fixture.file.as_deref().unwrap_or_default()))?;
            write_response(stream, 200, &[], &body)
        }
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        405 => "Method Not Allowed",
        426 => "Upgrade Required",
        _ => "Synthetic",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)
}

fn probe_fixtures(server: &FixtureServer, manifest: &FixtureManifest) -> Vec<FixtureResult> {
    manifest
        .fixtures
        .iter()
        .map(|fixture| probe_fixture(server.address, fixture))
        .collect()
}

fn probe_fixture(address: SocketAddrV4, fixture: &FixtureSpec) -> FixtureResult {
    let result = (|| -> Result<String, String> {
        let mut stream = TcpStream::connect_timeout(&address.into(), Duration::from_secs(1))
            .map_err(|error| error.to_string())?;
        let timeout = if fixture.behavior == FixtureBehavior::Stalled {
            Duration::from_millis(40)
        } else {
            Duration::from_secs(1)
        };
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| error.to_string())?;
        write!(
            stream,
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            fixture.path,
            address.port()
        )
        .map_err(|error| error.to_string())?;
        let mut response = Vec::new();
        match stream
            .take(MAX_RESPONSE_BYTES as u64)
            .read_to_end(&mut response)
        {
            Err(error)
                if fixture.behavior == FixtureBehavior::Stalled
                    && matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                return Ok("bounded timeout observed".to_string());
            }
            Err(error) => return Err(error.to_string()),
            Ok(_) if fixture.behavior == FixtureBehavior::Stalled => {
                return Err("stalled response completed before timeout".to_string());
            }
            Ok(_) => {}
        }
        let response = String::from_utf8_lossy(&response);
        let status = format!("HTTP/1.1 {} ", fixture.expected_status);
        if !response.starts_with(&status) || !response.contains(&fixture.expected_marker) {
            return Err("status or synthetic marker mismatch".to_string());
        }
        Ok("served from bounded loopback harness".to_string())
    })();
    match result {
        Ok(detail) => FixtureResult {
            id: fixture.id.clone(),
            passed: true,
            detail,
        },
        Err(detail) => FixtureResult {
            id: fixture.id.clone(),
            passed: false,
            detail,
        },
    }
}

#[derive(Debug)]
pub struct CleanupProbe {
    pub empty_child_environment: bool,
    pub owned_process_group_verified: bool,
    pub unrelated_process_survived: bool,
    pub descendant_terminated: bool,
    pub temporary_profile_removed: bool,
}

pub fn run_process_cleanup_probe(
    executable: &Path,
    scratch_root: &Path,
) -> Result<CleanupProbe, SpikeError> {
    fs::create_dir_all(scratch_root)?;
    let profile = scratch_root.join(format!("profile-{}", std::process::id()));
    if profile.exists() {
        fs::remove_dir_all(&profile)?;
    }
    fs::create_dir(&profile)?;

    let mut owned_command = child_command(executable, "owned", Some(&profile));
    let mut sentinel_command = child_command(executable, "sentinel", None);
    configure_new_process_group(&mut owned_command);
    configure_new_process_group(&mut sentinel_command);
    let mut owned = owned_command.spawn()?;
    let mut sentinel = sentinel_command.spawn()?;

    let result = (|| {
        let descendant_path = profile.join("descendant.pid");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !descendant_path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let descendant_pid = fs::read_to_string(&descendant_path)
            .map_err(SpikeError::Io)?
            .trim()
            .parse::<i32>()
            .map_err(|_| SpikeError::Probe("invalid descendant pid".to_string()))?;

        let owned_group_verified = process_group_id(owned.id()) == Some(owned.id() as i32);
        if !owned_group_verified {
            return Err(SpikeError::Probe(
                "owned child did not establish its own process group".to_string(),
            ));
        }
        terminate_owned_group(&mut owned)?;
        let descendant_terminated =
            wait_until(Duration::from_secs(2), || !process_exists(descendant_pid));
        let unrelated_process_survived = sentinel.try_wait()?.is_none();
        fs::remove_dir_all(&profile)?;
        Ok(CleanupProbe {
            empty_child_environment: true,
            owned_process_group_verified: owned_group_verified,
            unrelated_process_survived,
            descendant_terminated,
            temporary_profile_removed: !profile.exists(),
        })
    })();

    let _ = terminate_owned_group(&mut owned);
    let _ = terminate_owned_group(&mut sentinel);
    let _ = fs::remove_dir_all(&profile);
    result
}

fn child_command(executable: &Path, kind: &str, profile: Option<&Path>) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("--child")
        .arg(kind)
        .env_clear()
        .env("TERMIRUST_BROWSER_SPIKE_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(profile) = profile {
        command.arg("--profile").arg(profile);
    }
    command
}

#[cfg(unix)]
fn configure_new_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_new_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn process_group_id(pid: u32) -> Option<i32> {
    let group = unsafe { libc::getpgid(pid as i32) };
    (group >= 0).then_some(group)
}

#[cfg(not(unix))]
fn process_group_id(_pid: u32) -> Option<i32> {
    None
}

#[cfg(unix)]
fn terminate_owned_group(child: &mut Child) -> io::Result<()> {
    let pid = child.id() as i32;
    if process_group_id(child.id()) != Some(pid) {
        return child.kill();
    }
    unsafe {
        libc::killpg(pid, libc::SIGTERM);
    }
    if !wait_until(Duration::from_millis(500), || {
        child.try_wait().ok().flatten().is_some()
    }) {
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.wait();
    Ok(())
}

#[cfg(not(unix))]
fn terminate_owned_group(child: &mut Child) -> io::Result<()> {
    child.kill()?;
    let _ = child.wait();
    Ok(())
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn process_exists(_pid: i32) -> bool {
    false
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    condition()
}

pub fn run_child(kind: &str, profile: Option<&Path>) -> Result<(), SpikeError> {
    if std::env::vars_os().count() != 1
        || std::env::var("TERMIRUST_BROWSER_SPIKE_CHILD").as_deref() != Ok("1")
    {
        return Err(SpikeError::Probe(
            "fixture child environment was not empty".to_string(),
        ));
    }
    if kind == "owned" {
        let profile = profile.ok_or(SpikeError::InvalidArgument("owned child needs profile"))?;
        let executable = std::env::current_exe()?;
        let leaf = child_command(&executable, "leaf", None).spawn()?;
        fs::write(profile.join("descendant.pid"), leaf.id().to_string())?;
    } else if kind != "leaf" && kind != "sentinel" {
        return Err(SpikeError::InvalidArgument("unknown child kind"));
    }
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

pub fn generate_fixture_only_report(
    fixture_root: &Path,
    runs: u32,
    executable: &Path,
    scratch_root: &Path,
    generated_at: &str,
    rustc: &str,
) -> Result<EngineProbeReport, SpikeError> {
    if !(MIN_RUNS..=MAX_RUNS).contains(&runs) {
        return Err(SpikeError::InvalidArgument(
            "runs must be between 10 and 100",
        ));
    }
    if generated_at.len() > 64 || rustc.len() > 128 || !generated_at.is_ascii() || !rustc.is_ascii()
    {
        return Err(SpikeError::InvalidArgument("invalid environment metadata"));
    }
    let (manifest, fixture_manifest_sha256) = load_fixture_manifest(fixture_root)?;
    let server = FixtureServer::start(fixture_root.to_path_buf(), manifest.clone())?;
    let mut durations = Vec::with_capacity(runs as usize);
    let mut aggregate: BTreeMap<String, FixtureResult> = BTreeMap::new();
    for _ in 0..runs {
        let started = Instant::now();
        for result in probe_fixtures(&server, &manifest) {
            aggregate
                .entry(result.id.clone())
                .and_modify(|existing| {
                    if !result.passed {
                        *existing = result.clone();
                    }
                })
                .or_insert(result);
        }
        durations.push(duration_ms_ceil(started.elapsed()));
    }
    drop(server);
    let cleanup = run_process_cleanup_probe(executable, scratch_root)?;
    let mut sorted = durations.clone();
    sorted.sort_unstable();
    let harness = HarnessReport {
        loopback_only: true,
        empty_child_environment: cleanup.empty_child_environment,
        owned_process_group_verified: cleanup.owned_process_group_verified,
        unrelated_process_survived: cleanup.unrelated_process_survived,
        descendant_terminated: cleanup.descendant_terminated,
        temporary_profile_removed: cleanup.temporary_profile_removed,
        fixture_results: aggregate.into_values().collect(),
        warm_run_ms: durations,
        p50_ms: percentile(&sorted, 50),
        p95_ms: percentile(&sorted, 95),
    };
    let candidates = candidate_reports();
    validate_no_go(&candidates)?;
    Ok(EngineProbeReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at: generated_at.to_string(),
        mode: "fixture_only".to_string(),
        fixture_seed: FIXTURE_SEED,
        fixture_manifest_sha256,
        runs,
        machine: MachineReport {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            rustc: rustc.to_string(),
            browser_installed: known_browser_installed(),
            driver_installed: command_on_path("chromedriver"),
        },
        harness,
        candidates,
        decision: DecisionReport {
            kind: DecisionKind::NoGo,
            selected_candidate: None,
            blockers: vec![
                "No candidate was run against the hostile corpus on the reference machine."
                    .to_string(),
                "headless_chrome fails the repository license policy; the other routes still require explicit Chrome for Testing distribution review."
                    .to_string(),
                "No route has candidate-specific proof for every interception, sandbox, stale-document, cancellation, and packaging gate."
                    .to_string(),
                "The two Rust controller repositories publish releases but no repository SECURITY.md was found at the pinned commits."
                    .to_string(),
            ],
            review_owner: "TermiRust desktop maintainers".to_string(),
            review_date: "2026-11-27".to_string(),
        },
    })
}

fn candidate_reports() -> Vec<CandidateReport> {
    vec![
        candidate(CandidateSeed {
            id: "chromiumoxide",
            version: "0.9.1",
            commit: Some("a7e2bb835b9643410f9e3dc044f0d947e96cbfa4"),
            license: "MIT OR Apache-2.0",
            transitive_license_result: EvidenceStatus::Pass,
            adapter_compiled: cfg!(feature = "chromiumoxide-candidate"),
            security_process: "No repository SECURITY.md at the pinned source; Chromium has a separate official confidential vulnerability route.",
            sources: vec![
                source(
                    "crate metadata",
                    "https://crates.io/api/v1/crates/chromiumoxide",
                ),
                source(
                    "release",
                    "https://github.com/mattsse/chromiumoxide/releases/tag/v0.9.1",
                ),
                source(
                    "source",
                    "https://github.com/mattsse/chromiumoxide/tree/a7e2bb835b9643410f9e3dc044f0d947e96cbfa4",
                ),
            ],
        }),
        candidate(CandidateSeed {
            id: "headless_chrome",
            version: "1.0.22",
            commit: Some("0a5c307a85debc450378a1f19e4dac1838d7b22d"),
            license: "MIT",
            transitive_license_result: EvidenceStatus::Fail,
            adapter_compiled: cfg!(feature = "headless-chrome-candidate"),
            security_process: "No repository SECURITY.md at the pinned source; the README documents API gaps including frames and WebSocket inspection.",
            sources: vec![
                source(
                    "crate metadata",
                    "https://crates.io/api/v1/crates/headless_chrome",
                ),
                source(
                    "release",
                    "https://github.com/rust-headless-chrome/rust-headless-chrome/releases/tag/1.0.22",
                ),
                source(
                    "source",
                    "https://github.com/rust-headless-chrome/rust-headless-chrome/tree/0a5c307a85debc450378a1f19e4dac1838d7b22d",
                ),
            ],
        }),
        candidate(CandidateSeed {
            id: "webdriver_chromedriver",
            version: "WebDriver 2 July 2026 WD + ChromeDriver 152.0.7977.64",
            commit: None,
            license: "W3C Document License; Chrome/ChromeDriver terms require review",
            transitive_license_result: EvidenceStatus::Pass,
            adapter_compiled: false,
            security_process: "W3C standards issue process plus official Chromium confidential vulnerability reporting and ChromeDriver issue route.",
            sources: vec![
                source(
                    "WebDriver",
                    "https://www.w3.org/TR/2026/WD-webdriver2-20260702/",
                ),
                source(
                    "WebDriver BiDi",
                    "https://www.w3.org/TR/2026/WD-webdriver-bidi-20260629/",
                ),
                source(
                    "ChromeDriver",
                    "https://developer.chrome.com/docs/chromedriver",
                ),
                source(
                    "Chrome for Testing manifest",
                    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json",
                ),
            ],
        }),
    ]
}

struct CandidateSeed {
    id: &'static str,
    version: &'static str,
    commit: Option<&'static str>,
    license: &'static str,
    transitive_license_result: EvidenceStatus,
    adapter_compiled: bool,
    security_process: &'static str,
    sources: Vec<SourceRecord>,
}

fn candidate(seed: CandidateSeed) -> CandidateReport {
    let CandidateSeed {
        id,
        version,
        commit,
        license,
        transitive_license_result,
        adapter_compiled,
        security_process,
        sources,
    } = seed;
    let mut gates = BTreeMap::new();
    for gate in MANDATORY_GATES {
        let (status, evidence) = match gate {
            "compatible_license" if id == "headless_chrome" => (
                EvidenceStatus::Fail,
                "The pinned graph fails policy on GPL-3.0-or-later auto_generate_cdp and CDLA-Permissive-2.0 webpki-roots.",
            ),
            "compatible_license" => (
                EvidenceStatus::Unknown,
                "The controller graph is acceptable; branded browser redistribution terms need explicit legal review.",
            ),
            "maintained_release_and_security_route" if id == "webdriver_chromedriver" => (
                EvidenceStatus::Pass,
                "W3C and Chromium publish maintained issue/security routes; runtime compatibility remains a separate gate.",
            ),
            _ => (
                EvidenceStatus::Unknown,
                "Fixture-only run cannot promote documentation or generic harness behavior to candidate-specific proof.",
            ),
        };
        gates.insert(
            gate.to_string(),
            GateEvidence {
                status,
                evidence: evidence.to_string(),
            },
        );
    }
    CandidateReport {
        id: id.to_string(),
        controller_version: version.to_string(),
        controller_commit: commit.map(str::to_string),
        controller_license: license.to_string(),
        transitive_license_result,
        security_scan_date: "2026-08-27".to_string(),
        security_process: security_process.to_string(),
        adapter_compiled,
        runtime_available: false,
        browser: browser_pin(),
        sources,
        mandatory_gates: gates,
        measurements: Measurements {
            cold_start_ms: None,
            warm_start_ms: Vec::new(),
            idle_rss_bytes: None,
            idle_cpu_percent: None,
            binary_bytes: None,
            cancellation_ms: None,
        },
        unresolved_risks: vec![
            "No live hostile-fixture execution on the reference machine.".to_string(),
            "No candidate-specific process-tree, sandbox, or profile cleanup measurement."
                .to_string(),
            "No complete navigation/subresource/redirect/iframe/popup/WebSocket/service-worker/download interception proof."
                .to_string(),
        ],
    }
}

fn source(label: &str, url: &str) -> SourceRecord {
    SourceRecord {
        label: label.to_string(),
        url: url.to_string(),
        accessed: "2026-08-27".to_string(),
    }
}

fn browser_pin() -> BrowserPin {
    BrowserPin {
        product: "Chrome for Testing Stable".to_string(),
        version: "152.0.7977.64".to_string(),
        platform: "mac-arm64".to_string(),
        archive_sha256: "10033804338bd0a5aa098149a8dd64f3f2e0e8b201bf3d400d7c17d067ff696f"
            .to_string(),
        driver_archive_sha256: Some(
            "9e8b67036bf3d744feb97d5711a6f6ce40855d9554e93adfa4a869aa69677ef3"
                .to_string(),
        ),
        manifest_url: "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json".to_string(),
    }
}

fn validate_no_go(candidates: &[CandidateReport]) -> Result<(), SpikeError> {
    for candidate in candidates {
        if candidate.mandatory_gates.len() != MANDATORY_GATES.len() {
            return Err(SpikeError::Probe(format!(
                "candidate {} has incomplete gates",
                candidate.id
            )));
        }
        if candidate
            .mandatory_gates
            .values()
            .all(|gate| gate.status == EvidenceStatus::Pass)
        {
            return Err(SpikeError::Probe(
                "fixture-only report unexpectedly satisfies every gate".to_string(),
            ));
        }
    }
    Ok(())
}

fn known_browser_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        [
            "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .iter()
        .any(|path| Path::new(path).is_file())
    }
    #[cfg(not(target_os = "macos"))]
    {
        command_on_path("chromium") || command_on_path("google-chrome")
    }
}

fn command_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
}

pub fn write_report_atomic(path: &Path, report: &EngineProbeReport) -> Result<(), SpikeError> {
    let parent = path.parent().ok_or(SpikeError::InvalidArgument(
        "output needs a parent directory",
    ))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".browser-spike-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, report)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if file.metadata()?.len() > MAX_REPORT_BYTES {
            return Err(SpikeError::Probe("report exceeds 256 KiB".to_string()));
        }
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn duration_ms_ceil(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    u64::try_from(nanos.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)]
}

#[cfg(feature = "chromiumoxide-candidate")]
pub fn chromiumoxide_adapter_type() -> &'static str {
    std::any::type_name::<chromiumoxide::Browser>()
}

#[cfg(feature = "headless-chrome-candidate")]
pub fn headless_chrome_adapter_type() -> &'static str {
    std::any::type_name::<headless_chrome::Browser>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/browser-hostile")
    }

    #[test]
    fn frozen_manifest_is_complete_and_regular() {
        let (manifest, hash) = load_fixture_manifest(&fixture_root()).unwrap();
        assert_eq!(manifest.fixtures.len(), REQUIRED_FIXTURES.len());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn loopback_server_exercises_every_fixture() {
        let (manifest, _) = load_fixture_manifest(&fixture_root()).unwrap();
        let server = FixtureServer::start(fixture_root(), manifest.clone()).unwrap();
        let results = probe_fixtures(&server, &manifest);
        assert_eq!(results.len(), REQUIRED_FIXTURES.len());
        assert!(results.iter().all(|result| result.passed), "{results:#?}");
    }

    #[test]
    fn fixture_only_candidates_cannot_become_go() {
        let candidates = candidate_reports();
        validate_no_go(&candidates).unwrap();
        assert!(candidates.iter().all(|candidate| {
            candidate
                .mandatory_gates
                .values()
                .any(|gate| gate.status != EvidenceStatus::Pass)
        }));
    }

    #[test]
    fn report_writer_is_bounded_and_atomic() {
        let root = std::env::temp_dir().join(format!(
            "termirust-browser-report-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let report = EngineProbeReport {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at: "2026-08-27T00:00:00Z".to_string(),
            mode: "fixture_only".to_string(),
            fixture_seed: FIXTURE_SEED,
            fixture_manifest_sha256: "0".repeat(64),
            runs: MIN_RUNS,
            machine: MachineReport {
                os: "test".to_string(),
                arch: "test".to_string(),
                rustc: "test".to_string(),
                browser_installed: false,
                driver_installed: false,
            },
            harness: HarnessReport {
                loopback_only: true,
                empty_child_environment: true,
                owned_process_group_verified: true,
                unrelated_process_survived: true,
                descendant_terminated: true,
                temporary_profile_removed: true,
                fixture_results: Vec::new(),
                warm_run_ms: vec![1; MIN_RUNS as usize],
                p50_ms: 1,
                p95_ms: 1,
            },
            candidates: candidate_reports(),
            decision: DecisionReport {
                kind: DecisionKind::NoGo,
                selected_candidate: None,
                blockers: vec!["test blocker".to_string()],
                review_owner: "test".to_string(),
                review_date: "2026-11-27".to_string(),
            },
        };
        let path = root.join("report.json");
        write_report_atomic(&path, &report).unwrap();
        let decoded: EngineProbeReport = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(decoded.schema_version, REPORT_SCHEMA_VERSION);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_manifest_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "termirust-browser-manifest-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("index.json"),
            br#"{"schema_version":1,"seed":419504166,"fixtures":[]}"#,
        )
        .unwrap();
        let error = load_fixture_manifest(&root).unwrap_err();
        assert!(matches!(error, SpikeError::InvalidFixture(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_count_bounds_fail_before_any_process_or_fixture_side_effect() {
        let error = generate_fixture_only_report(
            Path::new("missing-fixtures"),
            MIN_RUNS - 1,
            Path::new("missing-executable"),
            Path::new("missing-scratch"),
            "2026-08-27T00:00:00Z",
            "rustc test",
        )
        .unwrap_err();
        assert!(matches!(error, SpikeError::InvalidArgument(_)));
        assert!(!Path::new("missing-scratch").exists());
    }
}
