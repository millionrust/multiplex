use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::ffi::OsString;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use termirust_domain::{
    DetectionCandidate, DetectionReport, DetectionStatus, ExecutableSpec, RuntimeId,
};

const MAX_PATH_ENTRIES: usize = 128;
const MAX_PATH_BYTES: usize = 64 * 1024;
const MAX_CANDIDATES_PER_RUNTIME: usize = 3;
const MAX_COMBINED_OUTPUT: usize = 8 * 1024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RuntimeProbeDescriptor {
    pub runtime: RuntimeId,
    pub executable_names: Vec<String>,
    pub version_arguments: Vec<String>,
    pub minimum_major_version: Option<u64>,
}

impl RuntimeProbeDescriptor {
    #[cfg(test)]
    fn fixture(runtime: &str, executable: &str) -> Self {
        Self {
            runtime: RuntimeId::new(runtime).unwrap(),
            executable_names: vec![executable.to_string()],
            version_arguments: vec!["--version".to_string()],
            minimum_major_version: None,
        }
    }
}

pub fn known_runtime_descriptors() -> Vec<RuntimeProbeDescriptor> {
    [
        ("claude", "claude"),
        ("codex", "codex"),
        ("gemini", "gemini"),
    ]
    .into_iter()
    .map(|(runtime, executable)| RuntimeProbeDescriptor {
        runtime: RuntimeId::new(runtime).expect("compiled runtime ID is valid"),
        executable_names: vec![executable.to_string()],
        version_arguments: vec!["--version".to_string()],
        minimum_major_version: None,
    })
    .collect()
}

pub fn discovery_path_snapshot() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

#[derive(Clone, Copy, Debug)]
pub struct DiscoveryLimits {
    pub probe_timeout: Duration,
    pub cache_ttl: Duration,
    pub max_concurrency: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            probe_timeout: Duration::from_secs(2),
            cache_ttl: Duration::from_secs(10 * 60),
            max_concurrency: 4,
        }
    }
}

#[derive(Clone, Default)]
pub struct DiscoveryCancellation(Arc<AtomicBool>);

impl DiscoveryCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct CachedReport {
    key: u64,
    created_at: Instant,
    report: DetectionReport,
}

pub struct CliDiscovery {
    limits: DiscoveryLimits,
    cache: Mutex<Option<CachedReport>>,
}

impl Default for CliDiscovery {
    fn default() -> Self {
        Self::new(DiscoveryLimits::default())
    }
}

impl CliDiscovery {
    pub fn new(limits: DiscoveryLimits) -> Self {
        Self {
            limits,
            cache: Mutex::new(None),
        }
    }

    pub fn discover(
        &self,
        descriptors: &[RuntimeProbeDescriptor],
        path_snapshot: &OsString,
        cancel: &DiscoveryCancellation,
        refresh: bool,
    ) -> DetectionReport {
        let bounded_path = bounded_path_entries(path_snapshot);
        let initial_found = find_candidates(descriptors, &bounded_path);
        let key = discovery_key(descriptors, &bounded_path, &initial_found);
        if !refresh
            && let Some(cached) = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .filter(|cached| {
                    cached.key == key && cached.created_at.elapsed() < self.limits.cache_ttl
                })
        {
            return cached.report.clone();
        }

        let mut descriptors = descriptors.to_vec();
        descriptors.sort_by(|left, right| left.runtime.cmp(&right.runtime));
        let found = find_candidates(&descriptors, &bounded_path);
        let mut jobs = Vec::new();
        let mut candidates = Vec::new();
        for descriptor in &descriptors {
            let paths = found.get(&descriptor.runtime).cloned().unwrap_or_default();
            if paths.is_empty() {
                if let Some(name) = descriptor.executable_names.first()
                    && let Ok(executable) = ExecutableSpec::parse(name)
                {
                    candidates.push(DetectionCandidate {
                        runtime: descriptor.runtime.clone(),
                        executable,
                        version: None,
                        status: DetectionStatus::Missing,
                        diagnostic_code: Some("not-found".to_string()),
                    });
                }
                continue;
            }
            for path in paths.into_iter().take(MAX_CANDIDATES_PER_RUNTIME) {
                jobs.push((descriptor.clone(), path));
            }
        }

        let path_value = std::env::join_paths(&bounded_path).unwrap_or_default();
        let concurrency = self.limits.max_concurrency.clamp(1, 4);
        for chunk in jobs.chunks(concurrency) {
            if cancel.is_cancelled() {
                break;
            }
            let results = thread::scope(|scope| {
                let handles = chunk
                    .iter()
                    .map(|(descriptor, path)| {
                        let descriptor = descriptor.clone();
                        let path = path.clone();
                        let path_value = path_value.clone();
                        let cancel = cancel.clone();
                        let timeout = self.limits.probe_timeout;
                        scope
                            .spawn(move || probe(&descriptor, &path, &path_value, timeout, &cancel))
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("probe thread panicked"))
                    .collect::<Vec<_>>()
            });
            candidates.extend(results);
        }
        candidates.sort_by(|left, right| {
            left.runtime
                .cmp(&right.runtime)
                .then_with(|| detection_rank(&left.status).cmp(&detection_rank(&right.status)))
                .then_with(|| left.executable.as_str().cmp(right.executable.as_str()))
        });
        let cancelled = cancel.is_cancelled();
        let partial = cancelled
            || candidates.iter().any(|candidate| {
                matches!(
                    candidate.status,
                    DetectionStatus::TimedOut
                        | DetectionStatus::PermissionDenied
                        | DetectionStatus::Failed
                )
            });
        let report = DetectionReport {
            candidates,
            partial,
            cancelled,
        };
        if !cancelled {
            *self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CachedReport {
                key,
                created_at: Instant::now(),
                report: report.clone(),
            });
        }
        report
    }
}

fn bounded_path_entries(path_snapshot: &OsString) -> Vec<PathBuf> {
    let encoded = path_snapshot.to_string_lossy();
    if encoded.len() > MAX_PATH_BYTES {
        return Vec::new();
    }
    std::env::split_paths(path_snapshot)
        .take(MAX_PATH_ENTRIES)
        .collect()
}

fn find_candidates(
    descriptors: &[RuntimeProbeDescriptor],
    path_entries: &[PathBuf],
) -> BTreeMap<RuntimeId, Vec<PathBuf>> {
    let mut found = BTreeMap::new();
    for descriptor in descriptors {
        let mut runtime_paths = Vec::new();
        for executable_name in &descriptor.executable_names {
            for directory in path_entries {
                let candidate = directory.join(executable_name);
                if is_executable_file(&candidate) && !runtime_paths.contains(&candidate) {
                    runtime_paths.push(candidate);
                }
                #[cfg(windows)]
                for suffix in [".exe", ".cmd", ".bat"] {
                    let candidate = directory.join(format!("{executable_name}{suffix}"));
                    if is_executable_file(&candidate) && !runtime_paths.contains(&candidate) {
                        runtime_paths.push(candidate);
                    }
                }
            }
        }
        found.insert(descriptor.runtime.clone(), runtime_paths);
    }
    found
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn discovery_key(
    descriptors: &[RuntimeProbeDescriptor],
    paths: &[PathBuf],
    found: &BTreeMap<RuntimeId, Vec<PathBuf>>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    descriptors.hash(&mut hasher);
    for path in paths {
        path.hash(&mut hasher);
    }
    for path in found.values().flatten() {
        path.hash(&mut hasher);
        if let Ok(metadata) = path.metadata() {
            metadata.len().hash(&mut hasher);
            metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn probe(
    descriptor: &RuntimeProbeDescriptor,
    path: &Path,
    path_snapshot: &OsString,
    timeout: Duration,
    cancel: &DiscoveryCancellation,
) -> DetectionCandidate {
    let executable = ExecutableSpec::parse(path.to_string_lossy().as_ref())
        .expect("discovered executable path is absolute");
    let mut command = Command::new(path);
    command
        .args(&descriptor.version_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("PATH", path_snapshot)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1");
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let permission_denied = error.kind() == std::io::ErrorKind::PermissionDenied;
            return DetectionCandidate {
                runtime: descriptor.runtime.clone(),
                executable,
                version: None,
                status: if permission_denied {
                    DetectionStatus::PermissionDenied
                } else {
                    DetectionStatus::Failed
                },
                diagnostic_code: Some(
                    if permission_denied {
                        "permission-denied"
                    } else {
                        "spawn-failed"
                    }
                    .to_string(),
                ),
            };
        }
    };
    let output = Arc::new(Mutex::new(Vec::with_capacity(MAX_COMBINED_OUTPUT)));
    let mut readers = Vec::new();
    if let Some(stream) = child.stdout.take() {
        readers.push(spawn_output_reader(stream, output.clone()));
    }
    if let Some(stream) = child.stderr.take() {
        readers.push(spawn_output_reader(stream, output.clone()));
    }
    let started = Instant::now();
    let (status, diagnostic_code) = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break (DetectionStatus::Failed, Some("cancelled".to_string()));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break (DetectionStatus::TimedOut, Some("timeout".to_string()));
        }
        match child.try_wait() {
            Ok(Some(exit)) if exit.success() => break (DetectionStatus::Supported, None),
            Ok(Some(_)) => break (DetectionStatus::Failed, Some("non-zero-exit".to_string())),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break (DetectionStatus::Failed, Some("wait-failed".to_string())),
        }
    };
    for reader in readers {
        let _ = reader.join();
    }
    let output = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let version = String::from_utf8_lossy(&output).trim().to_string();
    let mut status = status;
    if status == DetectionStatus::Supported {
        let parsed_major = parse_first_number(&version);
        if parsed_major.is_none() {
            status = DetectionStatus::DetectedUnknownVersion;
        } else if let Some(minimum) = descriptor.minimum_major_version
            && parsed_major.is_some_and(|major| major < minimum)
        {
            status = DetectionStatus::UnsupportedVersion;
        }
    }
    DetectionCandidate {
        runtime: descriptor.runtime.clone(),
        executable,
        version: (!version.is_empty()).then_some(version),
        status,
        diagnostic_code,
    }
}

fn spawn_output_reader(
    mut stream: impl std::io::Read + Send + 'static,
    output: Arc<Mutex<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let mut output = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let remaining = MAX_COMBINED_OUTPUT.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    })
}

fn parse_first_number(value: &str) -> Option<u64> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn detection_rank(status: &DetectionStatus) -> u8 {
    match status {
        DetectionStatus::Supported => 0,
        DetectionStatus::DetectedUnknownVersion => 1,
        DetectionStatus::UnsupportedVersion => 2,
        DetectionStatus::PermissionDenied => 3,
        DetectionStatus::TimedOut => 4,
        DetectionStatus::Failed => 5,
        DetectionStatus::Missing => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static DISCOVERY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn discovery_test_guard() -> std::sync::MutexGuard<'static, ()> {
        DISCOVERY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(unix)]
    fn executable(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn discovery_is_bounded_deterministic_and_argv_only() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        let probe = "[ \"$#\" -eq 1 ] && [ \"$1\" = '--version' ] || exit 9; \
                     [ \"${LC_ALL:-}\" = C ] && [ \"${HOME+x}\" != x ] || exit 8";
        executable(
            fixture.path(),
            "beta",
            &format!("{probe}; printf 'beta 2.0\\n'"),
        );
        executable(
            fixture.path(),
            "alpha",
            &format!("{probe}; printf 'alpha 1.0\\n'"),
        );
        let descriptors = vec![
            RuntimeProbeDescriptor::fixture("beta", "beta"),
            RuntimeProbeDescriptor::fixture("alpha", "alpha"),
        ];
        let report = CliDiscovery::default().discover(
            &descriptors,
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(
            report
                .candidates
                .iter()
                .map(|candidate| candidate.runtime.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert!(
            report
                .candidates
                .iter()
                .all(|candidate| candidate.status == DetectionStatus::Supported),
            "{report:?}"
        );
        assert!(!report.partial);
    }

    #[test]
    #[cfg(unix)]
    fn hanging_and_malformed_probes_do_not_hide_success() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        executable(fixture.path(), "good", "printf 'good 1.0\\n'");
        executable(fixture.path(), "hang", "while :; do sleep 1; done");
        executable(fixture.path(), "unknown", "printf 'not-a-version\\n'");
        let descriptors = vec![
            RuntimeProbeDescriptor::fixture("good", "good"),
            RuntimeProbeDescriptor::fixture("hang", "hang"),
            RuntimeProbeDescriptor::fixture("unknown", "unknown"),
        ];
        let discovery = CliDiscovery::new(DiscoveryLimits {
            // Preserve the production two-second budget while giving this
            // concurrency fixture headroom when the full GPUI suite saturates CI.
            probe_timeout: Duration::from_secs(5),
            ..DiscoveryLimits::default()
        });
        let report = discovery.discover(
            &descriptors,
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert!(report.partial);
        assert!(
            report
                .candidates
                .iter()
                .any(|candidate| candidate.status == DetectionStatus::TimedOut)
        );
        assert!(
            report
                .candidates
                .iter()
                .any(|candidate| { candidate.status == DetectionStatus::DetectedUnknownVersion }),
            "{report:?}"
        );
        assert!(
            report
                .candidates
                .iter()
                .any(|candidate| candidate.status == DetectionStatus::Supported)
        );
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_stops_scan() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        executable(fixture.path(), "hang", "while :; do sleep 1; done");
        let descriptor = RuntimeProbeDescriptor::fixture("hang", "hang");
        let cancel = DiscoveryCancellation::default();
        let cancel_worker = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel_worker.cancel();
        });
        let discovery = CliDiscovery::new(DiscoveryLimits {
            probe_timeout: Duration::from_secs(2),
            ..DiscoveryLimits::default()
        });
        let report = discovery.discover(
            &[descriptor],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &cancel,
            true,
        );
        assert!(report.cancelled);
        assert!(report.partial);
    }

    #[test]
    #[cfg(unix)]
    fn cache_invalidates_when_executable_metadata_changes() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        let path = executable(fixture.path(), "tool", "printf 'tool 1.0\\n'");
        let descriptor = RuntimeProbeDescriptor::fixture("tool", "tool");
        let discovery = CliDiscovery::default();
        let path_snapshot = std::env::join_paths([fixture.path()]).unwrap();
        let first = discovery.discover(
            std::slice::from_ref(&descriptor),
            &path_snapshot,
            &DiscoveryCancellation::default(),
            false,
        );
        thread::sleep(Duration::from_millis(20));
        fs::write(&path, "#!/bin/sh\nprintf 'tool 2.0\\n'\n").unwrap();
        let second = discovery.discover(
            std::slice::from_ref(&descriptor),
            &path_snapshot,
            &DiscoveryCancellation::default(),
            false,
        );
        assert_ne!(first.candidates[0].version, second.candidates[0].version);
    }

    #[test]
    #[cfg(unix)]
    fn huge_output_is_truncated_and_unsupported_is_explicit() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        executable(
            fixture.path(),
            "huge",
            "printf '1.0 '; i=0; while [ $i -lt 12000 ]; do printf x; i=$((i+1)); done; printf '\\n'",
        );
        let mut descriptor = RuntimeProbeDescriptor::fixture("huge", "huge");
        descriptor.minimum_major_version = Some(99);
        let report = CliDiscovery::default().discover(
            &[descriptor],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert!(report.candidates[0].version.as_ref().unwrap().len() <= MAX_COMBINED_OUTPUT);
        assert_eq!(
            report.candidates[0].status,
            DetectionStatus::UnsupportedVersion
        );
    }

    #[test]
    #[cfg(unix)]
    fn path_and_candidate_search_limits_are_enforced() {
        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        executable(fixture.path(), "late", "printf 'late 1.0\\n'");
        let mut entries = (0..MAX_PATH_ENTRIES)
            .map(|index| fixture.path().join(format!("missing-{index}")))
            .collect::<Vec<_>>();
        entries.push(fixture.path().to_path_buf());
        let report = CliDiscovery::default().discover(
            &[RuntimeProbeDescriptor::fixture("late", "late")],
            &std::env::join_paths(entries).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(report.candidates[0].status, DetectionStatus::Missing);

        let oversized_path = OsString::from("x".repeat(MAX_PATH_BYTES + 1));
        let report = CliDiscovery::default().discover(
            &[RuntimeProbeDescriptor::fixture("late", "late")],
            &oversized_path,
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(report.candidates[0].status, DetectionStatus::Missing);

        let directories = (0..4)
            .map(|_| tempfile::tempdir().unwrap())
            .collect::<Vec<_>>();
        for directory in &directories {
            executable(directory.path(), "tool", "printf 'tool 1.0\\n'");
        }
        let report = CliDiscovery::default().discover(
            &[RuntimeProbeDescriptor::fixture("tool", "tool")],
            &std::env::join_paths(directories.iter().map(|directory| directory.path())).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(report.candidates.len(), MAX_CANDIDATES_PER_RUNTIME);
        assert!(
            report
                .candidates
                .iter()
                .all(|candidate| candidate.status == DetectionStatus::Supported)
        );
    }

    #[test]
    #[cfg(unix)]
    fn inaccessible_probe_is_reported_as_permission_denied() {
        use std::os::unix::fs::PermissionsExt as _;

        let _guard = discovery_test_guard();
        let fixture = tempfile::tempdir().unwrap();
        let interpreter = fixture.path().join("blocked-interpreter");
        fs::write(&interpreter, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o600)).unwrap();
        let candidate = fixture.path().join("blocked");
        fs::write(&candidate, format!("#!{}\n", interpreter.to_string_lossy())).unwrap();
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();

        let report = CliDiscovery::default().discover(
            &[RuntimeProbeDescriptor::fixture("blocked", "blocked")],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(
            report.candidates[0].status,
            DetectionStatus::PermissionDenied
        );
        assert!(report.partial);
    }
}
