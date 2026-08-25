use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::ffi::OsString;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use termirust_domain::{
    ExecutableSpec, MAX_RUNTIME_CANDIDATES, RuntimeDescriptor, RuntimeDescriptorKind,
    RuntimeDetectionResult, RuntimeDetectionStatus, RuntimeId, compiled_runtime_descriptors,
    parse_runtime_version,
};

const MAX_PATH_ENTRIES: usize = 128;
const MAX_PATH_BYTES: usize = 64 * 1024;
const MAX_COMBINED_OUTPUT: usize = 8 * 1024;
use termirust_session_host::process_observation::fingerprint_executable;

pub fn known_runtime_descriptors() -> Vec<RuntimeDescriptor> {
    compiled_runtime_descriptors()
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDiscoveryEntry {
    pub result: RuntimeDetectionResult,
    pub executable: Option<ExecutableSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeDiscoveryReport {
    pub entries: Vec<RuntimeDiscoveryEntry>,
    pub partial: bool,
    pub cancelled: bool,
}

impl RuntimeDiscoveryReport {
    pub fn entry(&self, runtime_id: &RuntimeId) -> Option<&RuntimeDiscoveryEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.result.runtime_id == runtime_id)
    }
}

#[derive(Clone)]
struct CachedReport {
    key: u64,
    created_at: Instant,
    report: RuntimeDiscoveryReport,
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
        descriptors: &[RuntimeDescriptor],
        path_snapshot: &OsString,
        cancel: &DiscoveryCancellation,
        refresh: bool,
    ) -> RuntimeDiscoveryReport {
        let bounded_path = bounded_path_entries(path_snapshot);
        let found = find_candidates(descriptors, &bounded_path);
        let key = discovery_key(descriptors, &bounded_path, &found);
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if !refresh
            && let Some(cached) = cached.as_ref().filter(|cached| {
                cached.key == key && cached.created_at.elapsed() < self.limits.cache_ttl
            })
        {
            return cached.report.clone();
        }

        let mut descriptors = descriptors.to_vec();
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        let path_value = std::env::join_paths(&bounded_path).unwrap_or_default();
        let mut entries = Vec::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            if cancel.is_cancelled() {
                break;
            }
            if descriptor.kind == RuntimeDescriptorKind::GenericCommand {
                entries.push(RuntimeDiscoveryEntry {
                    result: RuntimeDetectionResult {
                        runtime_id: descriptor.id.clone(),
                        descriptor_version: descriptor.descriptor_version,
                        status: RuntimeDetectionStatus::Available,
                        fingerprint: None,
                        safe_version: None,
                        capabilities: Default::default(),
                        diagnostic_code: Some("generic-command".to_string()),
                    },
                    executable: None,
                });
                continue;
            }
            let paths = found.get(&descriptor.id).cloned().unwrap_or_default();
            if paths.is_empty() {
                entries.push(missing_entry(descriptor));
                continue;
            }
            let concurrency = self.limits.max_concurrency.clamp(1, 4);
            let mut probes = Vec::new();
            for chunk in paths
                .into_iter()
                .take(MAX_RUNTIME_CANDIDATES)
                .collect::<Vec<_>>()
                .chunks(concurrency)
            {
                if cancel.is_cancelled() {
                    break;
                }
                let results = thread::scope(|scope| {
                    let handles = chunk
                        .iter()
                        .map(|path| {
                            let descriptor = descriptor.clone();
                            let path = path.clone();
                            let path_value = path_value.clone();
                            let cancel = cancel.clone();
                            let timeout = self.limits.probe_timeout;
                            scope.spawn(move || {
                                probe(&descriptor, &path, &path_value, timeout, &cancel)
                            })
                        })
                        .collect::<Vec<_>>();
                    handles
                        .into_iter()
                        .map(|handle| handle.join().unwrap_or_else(|_| partial_probe(descriptor)))
                        .collect::<Vec<_>>()
                });
                probes.extend(results);
            }
            probes.sort_by(|left, right| {
                detection_rank(left.entry.result.status)
                    .cmp(&detection_rank(right.entry.result.status))
                    .then_with(|| {
                        left.entry
                            .executable
                            .as_ref()
                            .map(ExecutableSpec::as_str)
                            .cmp(&right.entry.executable.as_ref().map(ExecutableSpec::as_str))
                    })
            });
            entries.push(
                probes
                    .into_iter()
                    .next()
                    .map(|probe| probe.entry)
                    .unwrap_or_else(|| partial_entry(descriptor, "cancelled")),
            );
        }
        let cancelled = cancel.is_cancelled();
        if cancelled {
            for descriptor in &descriptors {
                if entries
                    .iter()
                    .any(|entry| entry.result.runtime_id == descriptor.id)
                {
                    continue;
                }
                let previous = cached
                    .as_ref()
                    .and_then(|cached| cached.report.entry(&descriptor.id))
                    .filter(|entry| cached_entry_is_current(entry, descriptor, &found));
                entries.push(
                    previous
                        .cloned()
                        .unwrap_or_else(|| partial_entry(descriptor, "cancelled")),
                );
            }
        }
        entries.sort_by(|left, right| left.result.runtime_id.cmp(&right.result.runtime_id));
        let partial = cancelled
            || entries.iter().any(|entry| {
                matches!(
                    entry.result.status,
                    RuntimeDetectionStatus::Partial | RuntimeDetectionStatus::PermissionDenied
                )
            });
        let report = RuntimeDiscoveryReport {
            entries,
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

fn cached_entry_is_current(
    entry: &RuntimeDiscoveryEntry,
    descriptor: &RuntimeDescriptor,
    found: &BTreeMap<RuntimeId, Vec<PathBuf>>,
) -> bool {
    if descriptor.kind == RuntimeDescriptorKind::GenericCommand {
        return entry.result.status == RuntimeDetectionStatus::Available
            && entry.result.fingerprint.is_none()
            && entry.result.capabilities.is_empty();
    }
    let paths = found
        .get(&descriptor.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if entry.result.status == RuntimeDetectionStatus::Missing {
        return paths.is_empty();
    }
    let (Some(expected), Some(executable)) = (entry.result.fingerprint, entry.executable.as_ref())
    else {
        return false;
    };
    paths.iter().any(|path| {
        path.to_string_lossy() == executable.as_str()
            && fingerprint_executable(path).ok() == Some(expected)
    })
}

fn bounded_path_entries(path_snapshot: &OsString) -> Vec<PathBuf> {
    if path_snapshot.to_string_lossy().len() > MAX_PATH_BYTES {
        return Vec::new();
    }
    std::env::split_paths(path_snapshot)
        .take(MAX_PATH_ENTRIES)
        .collect()
}

fn find_candidates(
    descriptors: &[RuntimeDescriptor],
    path_entries: &[PathBuf],
) -> BTreeMap<RuntimeId, Vec<PathBuf>> {
    let mut found = BTreeMap::new();
    for descriptor in descriptors {
        let mut runtime_paths = Vec::new();
        for executable_name in &descriptor.executable_candidates {
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
        found.insert(descriptor.id.clone(), runtime_paths);
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
    descriptors: &[RuntimeDescriptor],
    paths: &[PathBuf],
    found: &BTreeMap<RuntimeId, Vec<PathBuf>>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    for descriptor in descriptors {
        descriptor.id.hash(&mut hasher);
        descriptor.descriptor_version.hash(&mut hasher);
        descriptor.executable_candidates.hash(&mut hasher);
        descriptor.version_arguments.hash(&mut hasher);
    }
    for path in paths {
        path.hash(&mut hasher);
    }
    for path in found.values().flatten() {
        path.hash(&mut hasher);
        match fingerprint_executable(path) {
            Ok(fingerprint) => fingerprint.hash(&mut hasher),
            Err(error) => error.kind().hash(&mut hasher),
        }
    }
    hasher.finish()
}

struct ProbeResult {
    entry: RuntimeDiscoveryEntry,
}

fn probe(
    descriptor: &RuntimeDescriptor,
    path: &Path,
    path_snapshot: &OsString,
    timeout: Duration,
    cancel: &DiscoveryCancellation,
) -> ProbeResult {
    let executable = ExecutableSpec::parse(path.to_string_lossy().as_ref()).ok();
    let fingerprint = match fingerprint_executable(path) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let status = if error.kind() == std::io::ErrorKind::PermissionDenied {
                RuntimeDetectionStatus::PermissionDenied
            } else {
                RuntimeDetectionStatus::Partial
            };
            return ProbeResult {
                entry: RuntimeDiscoveryEntry {
                    result: RuntimeDetectionResult {
                        runtime_id: descriptor.id.clone(),
                        descriptor_version: descriptor.descriptor_version,
                        status,
                        fingerprint: None,
                        safe_version: None,
                        capabilities: Default::default(),
                        diagnostic_code: Some("fingerprint-unavailable".to_string()),
                    },
                    executable,
                },
            };
        }
    };
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
            return ProbeResult {
                entry: RuntimeDiscoveryEntry {
                    result: RuntimeDetectionResult {
                        runtime_id: descriptor.id.clone(),
                        descriptor_version: descriptor.descriptor_version,
                        status: if permission_denied {
                            RuntimeDetectionStatus::PermissionDenied
                        } else {
                            RuntimeDetectionStatus::Partial
                        },
                        fingerprint: Some(fingerprint),
                        safe_version: None,
                        capabilities: Default::default(),
                        diagnostic_code: Some(if permission_denied {
                            "permission-denied".to_string()
                        } else {
                            "spawn-failed".to_string()
                        }),
                    },
                    executable,
                },
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
    let diagnostic = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Some("cancelled");
        }
        match child.try_wait() {
            Ok(Some(exit)) if exit.success() => break None,
            Ok(Some(_)) => break Some("non-zero-exit"),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Some("timeout");
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break Some("wait-failed"),
        }
    };
    for reader in readers {
        let _ = reader.join();
    }
    let output = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let parsed = (diagnostic.is_none())
        .then(|| String::from_utf8_lossy(&output))
        .and_then(|output| parse_runtime_version(&output));
    let capabilities = parsed
        .map(|version| descriptor.capabilities_for(version))
        .unwrap_or_default();
    let (status, diagnostic_code) = if let Some(code) = diagnostic {
        (RuntimeDetectionStatus::Partial, Some(code.to_string()))
    } else if parsed.is_none() {
        (
            RuntimeDetectionStatus::UnsupportedVersion,
            Some("malformed-version".to_string()),
        )
    } else if capabilities.is_empty() {
        (
            RuntimeDetectionStatus::UnsupportedVersion,
            Some("unsupported-version".to_string()),
        )
    } else {
        (RuntimeDetectionStatus::Available, None)
    };
    ProbeResult {
        entry: RuntimeDiscoveryEntry {
            result: RuntimeDetectionResult {
                runtime_id: descriptor.id.clone(),
                descriptor_version: descriptor.descriptor_version,
                status,
                fingerprint: Some(fingerprint),
                safe_version: parsed.map(|version| version.to_string()),
                capabilities,
                diagnostic_code,
            },
            executable,
        },
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

fn missing_entry(descriptor: &RuntimeDescriptor) -> RuntimeDiscoveryEntry {
    RuntimeDiscoveryEntry {
        result: RuntimeDetectionResult {
            runtime_id: descriptor.id.clone(),
            descriptor_version: descriptor.descriptor_version,
            status: RuntimeDetectionStatus::Missing,
            fingerprint: None,
            safe_version: None,
            capabilities: Default::default(),
            diagnostic_code: Some("not-found".to_string()),
        },
        executable: descriptor
            .executable_candidates
            .first()
            .and_then(|candidate| ExecutableSpec::parse(candidate).ok()),
    }
}

fn partial_entry(descriptor: &RuntimeDescriptor, code: &str) -> RuntimeDiscoveryEntry {
    RuntimeDiscoveryEntry {
        result: RuntimeDetectionResult {
            runtime_id: descriptor.id.clone(),
            descriptor_version: descriptor.descriptor_version,
            status: RuntimeDetectionStatus::Partial,
            fingerprint: None,
            safe_version: None,
            capabilities: Default::default(),
            diagnostic_code: Some(code.to_string()),
        },
        executable: None,
    }
}

fn partial_probe(descriptor: &RuntimeDescriptor) -> ProbeResult {
    ProbeResult {
        entry: partial_entry(descriptor, "probe-thread-failed"),
    }
}

fn detection_rank(status: RuntimeDetectionStatus) -> u8 {
    match status {
        RuntimeDetectionStatus::Available => 0,
        RuntimeDetectionStatus::UnsupportedVersion => 1,
        RuntimeDetectionStatus::PermissionDenied => 2,
        RuntimeDetectionStatus::Partial => 3,
        RuntimeDetectionStatus::Missing => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static DISCOVERY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        DISCOVERY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn loaded_test_discovery() -> CliDiscovery {
        CliDiscovery::new(DiscoveryLimits {
            probe_timeout: Duration::from_secs(10),
            ..DiscoveryLimits::default()
        })
    }

    #[cfg(unix)]
    fn executable(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    fn committed_fixture(directory: &Path, relative: &str, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/runtimes")
            .join(relative);
        let destination = directory.join(name);
        fs::copy(source, &destination).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
        destination
    }

    fn descriptor(id: &str, executable: &str) -> RuntimeDescriptor {
        let mut descriptor = compiled_runtime_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.id.as_str() == id)
            .unwrap_or_else(|| {
                compiled_runtime_descriptors()
                    .into_iter()
                    .find(|descriptor| descriptor.id.as_str() == "codex")
                    .unwrap()
            });
        descriptor.id = RuntimeId::new(id).unwrap();
        descriptor.executable_candidates = vec![executable.to_string()];
        descriptor
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_detection_is_bounded_deterministic_and_argv_only() {
        let _guard = guard();
        let fixture = tempfile::tempdir().unwrap();
        let probe = "[ \"$#\" -eq 1 ] && [ \"$1\" = '--version' ] || exit 9; [ \"${LC_ALL:-}\" = C ] && [ \"${HOME+x}\" != x ] || exit 8";
        executable(
            fixture.path(),
            "codex-fixture",
            &format!("{probe}; printf 'codex-cli 1.0.7\\n'"),
        );
        executable(
            fixture.path(),
            "claude-fixture",
            &format!("{probe}; printf '2.0.4 (Claude Code)\\n'"),
        );
        let report = loaded_test_discovery().discover(
            &[
                descriptor("codex", "codex-fixture"),
                descriptor("claude", "claude-fixture"),
            ],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.result.runtime_id.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex"]
        );
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.result.status == RuntimeDetectionStatus::Available)
        );
        assert!(
            report
                .entries
                .iter()
                .all(|entry| !entry.result.capabilities.is_empty())
        );
        assert!(!report.partial);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_committed_contract_fixtures_match_truth_table() {
        let _guard = guard();
        for (runtime, cases) in [
            (
                "codex",
                [
                    ("codex/lower-boundary.sh", RuntimeDetectionStatus::Available),
                    ("codex/supported.sh", RuntimeDetectionStatus::Available),
                    (
                        "codex/upper-boundary.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                    (
                        "codex/unsupported.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                ],
            ),
            (
                "claude",
                [
                    (
                        "claude/lower-boundary.sh",
                        RuntimeDetectionStatus::Available,
                    ),
                    ("claude/supported.sh", RuntimeDetectionStatus::Available),
                    (
                        "claude/upper-boundary.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                    (
                        "claude/unsupported.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                ],
            ),
            (
                "gemini",
                [
                    (
                        "gemini/lower-boundary.sh",
                        RuntimeDetectionStatus::Available,
                    ),
                    ("gemini/supported.sh", RuntimeDetectionStatus::Available),
                    (
                        "gemini/upper-boundary.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                    (
                        "gemini/unsupported.sh",
                        RuntimeDetectionStatus::UnsupportedVersion,
                    ),
                ],
            ),
        ] {
            for (index, (fixture, expected)) in cases.into_iter().enumerate() {
                let directory = tempfile::tempdir().unwrap();
                let executable_name = format!("{runtime}-{index}");
                committed_fixture(directory.path(), fixture, &executable_name);
                let report = loaded_test_discovery().discover(
                    &[descriptor(runtime, &executable_name)],
                    &std::env::join_paths([directory.path()]).unwrap(),
                    &DiscoveryCancellation::default(),
                    true,
                );
                assert_eq!(report.entries[0].result.status, expected, "{fixture}");
                assert_eq!(
                    report.entries[0].result.capabilities.is_empty(),
                    expected != RuntimeDetectionStatus::Available,
                    "{fixture}"
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_reports_unsupported_malformed_hanging_and_missing() {
        let _guard = guard();
        assert_eq!(
            DiscoveryLimits::default().probe_timeout,
            Duration::from_secs(2)
        );
        let fixture = tempfile::tempdir().unwrap();
        executable(fixture.path(), "unsupported", "printf 'codex-cli 1.1.0\\n'");
        committed_fixture(fixture.path(), "generic/malformed.sh", "malformed");
        committed_fixture(fixture.path(), "generic/hanging.sh", "hang");
        let discovery = CliDiscovery::default();
        let report = discovery.discover(
            &[
                descriptor("unsupported-fixture", "unsupported"),
                descriptor("malformed-fixture", "malformed"),
                descriptor("zz-hanging-fixture", "hang"),
                descriptor("missing-fixture", "missing"),
            ],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert!(report.partial);
        assert_eq!(
            report
                .entry(&RuntimeId::new("unsupported-fixture").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::UnsupportedVersion),
            "{report:#?}"
        );
        assert_eq!(
            report
                .entry(&RuntimeId::new("zz-hanging-fixture").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::Partial),
            "{report:#?}"
        );
        assert_eq!(
            report
                .entry(&RuntimeId::new("missing-fixture").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::Missing),
            "{report:#?}"
        );
        assert!(
            report
                .entries
                .iter()
                .all(|entry| !format!("{entry:?}").contains("private-looking"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_cancellation_stops_exact_probe_and_does_not_cache_partial() {
        let _guard = guard();
        let fixture = tempfile::tempdir().unwrap();
        executable(fixture.path(), "hang", "while :; do :; done");
        let cancel = DiscoveryCancellation::default();
        let worker = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            worker.cancel();
        });
        let report = CliDiscovery::default().discover(
            &[descriptor("codex", "hang")],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &cancel,
            true,
        );
        assert!(report.cancelled);
        assert!(report.partial);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_cancel_retains_only_prior_fingerprint_valid_rows() {
        let _guard = guard();
        let fixture = tempfile::tempdir().unwrap();
        let changed = executable(fixture.path(), "changed", "printf 'codex-cli 1.0.1\\n'");
        executable(fixture.path(), "stable", "printf 'codex-cli 1.0.2\\n'");
        let descriptors = [
            descriptor("aaa-changed-fixture", "changed"),
            descriptor("codex", "stable"),
        ];
        let discovery = CliDiscovery::default();
        let path = std::env::join_paths([fixture.path()]).unwrap();
        let initial = discovery.discover(
            &descriptors,
            &path,
            &DiscoveryCancellation::default(),
            false,
        );
        assert!(
            initial
                .entries
                .iter()
                .all(|entry| entry.result.status == RuntimeDetectionStatus::Available)
        );

        fs::write(&changed, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        let cancel = DiscoveryCancellation::default();
        let worker = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            worker.cancel();
        });
        let refreshed = discovery.discover(&descriptors, &path, &cancel, true);

        assert!(refreshed.cancelled);
        assert_eq!(
            refreshed
                .entry(&RuntimeId::new("aaa-changed-fixture").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::Partial)
        );
        assert_eq!(
            refreshed
                .entry(&RuntimeId::new("codex").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::Available)
        );
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_cache_invalidates_on_same_name_content_replacement() {
        let _guard = guard();
        let fixture = tempfile::tempdir().unwrap();
        let path = executable(fixture.path(), "tool", "printf 'codex-cli 1.0.1\\n'");
        let descriptor = descriptor("codex", "tool");
        let discovery = CliDiscovery::default();
        let path_snapshot = std::env::join_paths([fixture.path()]).unwrap();
        let first = discovery.discover(
            std::slice::from_ref(&descriptor),
            &path_snapshot,
            &DiscoveryCancellation::default(),
            false,
        );
        fs::write(&path, "#!/bin/sh\nprintf 'codex-cli 1.0.2\\n'\n").unwrap();
        let second = discovery.discover(
            &[descriptor],
            &path_snapshot,
            &DiscoveryCancellation::default(),
            false,
        );
        assert_ne!(
            first.entries[0].result.fingerprint,
            second.entries[0].result.fingerprint
        );
        assert_ne!(
            first.entries[0].result.safe_version,
            second.entries[0].result.safe_version
        );
    }

    #[test]
    #[cfg(unix)]
    fn runtime_registry_path_candidate_output_and_permission_limits_hold() {
        use std::os::unix::fs::PermissionsExt as _;

        let _guard = guard();
        let fixture = tempfile::tempdir().unwrap();
        committed_fixture(fixture.path(), "generic/oversized.sh", "huge");
        let blocked = committed_fixture(fixture.path(), "generic/permission-denied.sh", "blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o100)).unwrap();
        let report = CliDiscovery::default().discover(
            &[descriptor("codex", "huge"), descriptor("claude", "blocked")],
            &std::env::join_paths([fixture.path()]).unwrap(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.result.capabilities.is_empty())
        );
        assert_eq!(
            report
                .entry(&RuntimeId::new("claude").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::PermissionDenied)
        );
        assert_eq!(
            report
                .entry(&RuntimeId::new("codex").unwrap())
                .map(|entry| entry.result.status),
            Some(RuntimeDetectionStatus::UnsupportedVersion)
        );

        let oversized = OsString::from("x".repeat(MAX_PATH_BYTES + 1));
        let report = CliDiscovery::default().discover(
            &[descriptor("codex", "tool")],
            &oversized,
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(
            report.entries[0].result.status,
            RuntimeDetectionStatus::Missing
        );
    }

    #[test]
    fn runtime_registry_generic_command_is_available_but_claims_no_semantics() {
        let generic = compiled_runtime_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.kind == RuntimeDescriptorKind::GenericCommand)
            .unwrap();
        let report = CliDiscovery::default().discover(
            &[generic],
            &OsString::new(),
            &DiscoveryCancellation::default(),
            true,
        );
        assert_eq!(
            report.entries[0].result.status,
            RuntimeDetectionStatus::Available
        );
        assert!(report.entries[0].result.capabilities.is_empty());
        assert!(report.entries[0].executable.is_none());
    }
}
