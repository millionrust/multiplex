//! tmux sessions as a third Controller session source, beside durable sessions and live
//! desktop panes.
//!
//! Listing is ephemeral: every `ListSessions` asks tmux. Attaching starts a Session Host in
//! this process whose PTY runs one more tmux client, `tmux attach-session -f ignore-size`,
//! and the existing durable attach path then streams from it. `ignore-size` keeps that
//! client out of window sizing, so a phone never reflows the desktop layout. Input reaches
//! tmux through the Host PTY under the normal writer lease.
//!
//! Hosts are shared by every connection attached to the same tmux session and torn down
//! when the last one detaches. Tearing a host down ends only its tmux client; the user's
//! tmux session keeps running.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use rand::RngCore as _;
use sha2::{Digest as _, Sha256};
use termirust_client::LocalEndpoint;
use termirust_domain::{
    HostInstanceId, HostLifecycle, HostedSessionId, OccupantGeneration, OutputSequence,
};
use termirust_session_host::{LaunchDescriptor, SessionHostHandle, StopDeadlines};
use termirust_store::JournalLimits;
use termirust_tmux::{Tmux, TmuxSession};

use crate::{ListenerError, ListenerErrorCode};

/// The `runtime` a tmux row reports to the Controller.
pub const TMUX_RUNTIME_ID: &str = "tmux";
/// Titles are bounded like live desktop pane titles.
const MAX_TITLE_CHARS: usize = 256;
const STABLE_ID_DOMAIN: &[u8] = b"termirust-tmux-session-v1";
const ATTACH_DIRECTORY_PREFIX: &str = "tmux-";
const ATTACH_WORKER_THREADS: usize = 2;
const MAX_ATTACH_DIMENSION: u32 = 1_000;

/// One tmux session as the Controller sees it.
#[derive(Clone)]
pub(crate) struct DiscoveredTmuxSession {
    pub session_id: HostedSessionId,
    pub session: TmuxSession,
}

impl DiscoveredTmuxSession {
    pub fn title(&self) -> String {
        let name = if self.session.name.trim().is_empty() {
            format!("tmux {}", self.session.id())
        } else {
            self.session.name.clone()
        };
        name.chars().take(MAX_TITLE_CHARS).collect()
    }
}

/// Discovers tmux sessions and owns the Session Hosts that attach to them.
#[derive(Clone)]
pub struct TmuxSessionSource {
    inner: Arc<SourceInner>,
}

impl fmt::Debug for TmuxSessionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TmuxSessionSource")
            .field("runtime_parent", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

struct SourceInner {
    discovery: Discovery,
    runtime_parent: PathBuf,
    runtime: Option<tokio::runtime::Runtime>,
    hosts: tokio::sync::Mutex<HashMap<HostedSessionId, AttachHost>>,
    generations: StdMutex<HashMap<HostedSessionId, GenerationState>>,
}

enum Discovery {
    Fixed(Tmux),
    System(StdMutex<Option<Tmux>>),
}

struct AttachHost {
    session: TmuxSession,
    handle: SessionHostHandle,
    directory: PathBuf,
    endpoint: LocalEndpoint,
    users: usize,
}

#[derive(Clone, Copy, Default)]
struct GenerationState {
    last_started: u64,
    live: bool,
}

impl GenerationState {
    fn current(self) -> OccupantGeneration {
        OccupantGeneration::new(if self.live {
            self.last_started
        } else {
            self.last_started.saturating_add(1)
        })
    }
}

impl TmuxSessionSource {
    /// Discovers tmux on every listing until it is found, so installing tmux while the
    /// listener runs is enough.
    pub fn system(runtime_parent: impl Into<PathBuf>) -> Result<Self, ListenerError> {
        Self::new(
            Discovery::System(StdMutex::new(None)),
            runtime_parent.into(),
        )
    }

    /// Uses exactly this tmux, including its socket directory.
    pub fn with_tmux(
        tmux: Tmux,
        runtime_parent: impl Into<PathBuf>,
    ) -> Result<Self, ListenerError> {
        Self::new(Discovery::Fixed(tmux), runtime_parent.into())
    }

    fn new(discovery: Discovery, runtime_parent: PathBuf) -> Result<Self, ListenerError> {
        if !runtime_parent.is_absolute() {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }
        // Hosts get their own runtime so PTY and journal work never stalls the listener's
        // single-threaded connection loop.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(ATTACH_WORKER_THREADS)
            .thread_name("termirust-tmux-attach")
            .enable_all()
            .build()
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        Ok(Self {
            inner: Arc::new(SourceInner {
                discovery,
                runtime_parent,
                runtime: Some(runtime),
                hosts: tokio::sync::Mutex::new(HashMap::new()),
                generations: StdMutex::new(HashMap::new()),
            }),
        })
    }

    fn runtime(&self) -> &tokio::runtime::Handle {
        self.inner
            .runtime
            .as_ref()
            .expect("the attach runtime lives as long as the source")
            .handle()
    }

    fn tmux(&self) -> Option<Tmux> {
        match &self.inner.discovery {
            Discovery::Fixed(tmux) => Some(tmux.clone()),
            Discovery::System(cached) => {
                let mut cached = cached.lock().unwrap_or_else(|poison| poison.into_inner());
                if cached.is_none() {
                    *cached = Tmux::discover().ok();
                }
                cached.clone()
            }
        }
    }

    /// Lists sessions. A missing tmux, no server, or a failing tmux are all an empty list:
    /// the durable and live sources must keep working regardless.
    pub(crate) async fn list(&self) -> Vec<DiscoveredTmuxSession> {
        let Some(tmux) = self.tmux().filter(Tmux::supports_ignore_size) else {
            return Vec::new();
        };
        let listed = self
            .runtime()
            .spawn_blocking(move || {
                let server = tmux.server_identity();
                tmux.list_sessions().map(|listing| {
                    listing
                        .sessions
                        .into_iter()
                        .map(|session| DiscoveredTmuxSession {
                            session_id: stable_session_id(&server, &session),
                            session,
                        })
                        .collect::<Vec<_>>()
                })
            })
            .await;
        match listed {
            Ok(Ok(sessions)) => sessions,
            Ok(Err(_)) | Err(_) => Vec::new(),
        }
    }

    /// The generation a Controller must present for this session. It advances each time a
    /// host for the session goes away, so a phone holding output from an earlier host is
    /// told to start over instead of resuming from a watermark the new host never had.
    pub(crate) fn generation(&self, session_id: HostedSessionId) -> OccupantGeneration {
        self.inner
            .generations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&session_id)
            .copied()
            .unwrap_or_default()
            .current()
    }

    fn set_generation(
        &self,
        session_id: HostedSessionId,
        update: impl FnOnce(&mut GenerationState),
    ) {
        let mut generations = self
            .inner
            .generations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        update(generations.entry(session_id).or_default());
    }

    /// Ensures a host is attached to `discovered` and counts one more user of it.
    pub(crate) async fn acquire(
        &self,
        discovered: &DiscoveredTmuxSession,
        columns: u32,
        rows: u32,
    ) -> Result<AcquiredHost, ListenerError> {
        let Some(tmux) = self.tmux() else {
            return Err(ListenerError::new(ListenerErrorCode::HostUnavailable));
        };
        let session_id = discovered.session_id;
        let mut hosts = self.inner.hosts.lock().await;
        if let Some(host) = hosts.get_mut(&session_id) {
            let exited = matches!(
                host.handle.stats().await.lifecycle,
                HostLifecycle::Exited | HostLifecycle::Failed
            );
            if !exited && host.session.same_session(&discovered.session) {
                host.users += 1;
                return Ok(AcquiredHost {
                    endpoint: host.endpoint.clone(),
                    generation: self.generation(session_id),
                    fresh: false,
                });
            }
            let stale = hosts.remove(&session_id).expect("host was present");
            self.set_generation(session_id, |state| state.live = false);
            self.runtime()
                .spawn(stop_host(stale.handle, stale.directory));
        }
        let generation = self.generation(session_id);
        let directory = self.attach_directory()?;
        let started = self
            .start_host(&tmux, discovered, &directory, columns, rows)
            .await;
        let (handle, endpoint) = match started {
            Ok(started) => started,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };
        self.set_generation(session_id, |state| {
            state.last_started = generation.get();
            state.live = true;
        });
        hosts.insert(
            session_id,
            AttachHost {
                session: discovered.session.clone(),
                handle,
                directory,
                endpoint: endpoint.clone(),
                users: 1,
            },
        );
        Ok(AcquiredHost {
            endpoint,
            generation,
            fresh: true,
        })
    }

    /// Drops one user of the session's host, stopping the host after the last one. Waits
    /// for the host to stop.
    pub(crate) async fn release(&self, session_id: HostedSessionId) {
        if let Some(stop) = self.release_now(session_id).await {
            let _ = self.runtime().spawn(stop).await;
        }
    }

    /// [`TmuxSessionSource::release`] for places that cannot wait, such as `Drop`.
    pub(crate) fn release_in_background(&self, session_id: HostedSessionId) {
        let source = self.clone();
        self.runtime().spawn(async move {
            source.release(session_id).await;
        });
    }

    async fn release_now(
        &self,
        session_id: HostedSessionId,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let mut hosts = self.inner.hosts.lock().await;
        let host = hosts.get_mut(&session_id)?;
        host.users = host.users.saturating_sub(1);
        if host.users > 0 {
            return None;
        }
        let host = hosts.remove(&session_id)?;
        self.set_generation(session_id, |state| state.live = false);
        Some(stop_host(host.handle, host.directory))
    }

    /// How many hosts are running. Tests use it to prove teardown.
    pub async fn running_hosts(&self) -> usize {
        self.inner.hosts.lock().await.len()
    }

    fn attach_directory(&self) -> Result<PathBuf, ListenerError> {
        prepare_user_only_directory(&self.inner.runtime_parent)?;
        let mut suffix = [0_u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let name = suffix
            .iter()
            .fold(String::from(ATTACH_DIRECTORY_PREFIX), |mut name, byte| {
                name.push_str(&format!("{byte:02x}"));
                name
            });
        let directory = self.inner.runtime_parent.join(name);
        fs::create_dir(&directory)
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        prepare_user_only_directory(&directory)?;
        Ok(directory)
    }

    async fn start_host(
        &self,
        tmux: &Tmux,
        discovered: &DiscoveredTmuxSession,
        directory: &Path,
        columns: u32,
        rows: u32,
    ) -> Result<(SessionHostHandle, LocalEndpoint), ListenerError> {
        let unavailable = || ListenerError::new(ListenerErrorCode::HostUnavailable);
        let executable = tmux.canonical_executable().map_err(|_| unavailable())?;
        let environment = tmux
            .client_environment()
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let cwd = environment
            .get("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute() && home.is_dir())
            .unwrap_or_else(|| directory.to_path_buf());
        let session_id = discovered.session_id;
        let descriptor = LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: directory.join("run"),
            session_dir: directory.join("data"),
            executable,
            runtime_detection: None,
            arguments: discovered.session.attach_arguments(),
            environment,
            cwd: Some(cwd),
            columns: attach_dimension(columns),
            rows: attach_dimension(rows),
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        };
        let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
        let handle = self
            .runtime()
            .spawn(termirust_session_host::start(descriptor))
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
        Ok((handle, endpoint))
    }
}

impl Drop for SourceInner {
    fn drop(&mut self) {
        // Dropping the hosts cancels them; their tmux clients get SIGHUP when the PTYs
        // close. The user's tmux sessions are untouched.
        let directories = self
            .hosts
            .get_mut()
            .drain()
            .map(|(_, host)| host.directory)
            .collect::<Vec<_>>();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        for directory in directories {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

/// A host ready for a Controller connection.
pub(crate) struct AcquiredHost {
    pub endpoint: LocalEndpoint,
    pub generation: OccupantGeneration,
    /// The host was started for this attach, so it holds no output older than now.
    pub fresh: bool,
}

async fn stop_host(handle: SessionHostHandle, directory: PathBuf) {
    let _ = handle.shutdown().await;
    let _ = fs::remove_dir_all(directory);
}

/// A session id that stays the same for one tmux session across listings, connections,
/// and listener restarts, and differs once a restarted server reuses the tmux id.
pub(crate) fn stable_session_id(server_identity: &str, session: &TmuxSession) -> HostedSessionId {
    let mut digest = Sha256::new();
    for part in [
        STABLE_ID_DOMAIN,
        server_identity.as_bytes(),
        session.id().as_bytes(),
        session.created_unix_seconds.to_string().as_bytes(),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    HostedSessionId::from_uuid(uuid::Builder::from_sha1_bytes(bytes).into_uuid())
}

/// The revision component for a tmux listing. Attached-client counts are excluded so a
/// Controller attaching does not invalidate its own list.
pub(crate) fn listing_revision(sessions: &[DiscoveredTmuxSession]) -> u64 {
    let mut entries = sessions
        .iter()
        .map(|session| (session.session_id, session.title()))
        .collect::<Vec<_>>();
    entries.sort();
    let mut digest = Sha256::new();
    for (session_id, title) in entries {
        digest.update(session_id.as_uuid().as_bytes());
        digest.update((title.len() as u64).to_be_bytes());
        digest.update(title.as_bytes());
    }
    let digest = digest.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// Replays from zero when the host is new: tmux redraws the whole screen for a new client,
/// and a watermark from an earlier host means nothing to this one.
pub(crate) fn attach_from(acquired: &AcquiredHost, requested: OutputSequence) -> OutputSequence {
    if acquired.fresh {
        OutputSequence::ZERO
    } else {
        requested
    }
}

fn attach_dimension(value: u32) -> u16 {
    u16::try_from(value.clamp(1, MAX_ATTACH_DIMENSION)).unwrap_or(1)
}

#[cfg(unix)]
fn prepare_user_only_directory(path: &Path) -> Result<(), ListenerError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let unavailable = || ListenerError::new(ListenerErrorCode::HostUnavailable);
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(unavailable());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| unavailable())?;
        }
        Err(_) => return Err(unavailable()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| unavailable())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(unavailable());
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_user_only_directory(path: &Path) -> Result<(), ListenerError> {
    fs::create_dir_all(path).map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(line: &str) -> TmuxSession {
        termirust_tmux::parse_list_sessions(line.as_bytes()).sessions[0].clone()
    }

    #[test]
    fn stable_ids_follow_server_id_and_creation_time_but_not_name() {
        let server = "/tmp/tmux-501/default";
        let original = session("$0\t100\t0\t1\tdemo\tzsh");
        let renamed = session("$0\t100\t2\t3\trenamed\tvim");
        let reused = session("$0\t200\t0\t1\tdemo\tzsh");
        let other_id = session("$1\t100\t0\t1\tdemo\tzsh");
        let id = stable_session_id(server, &original);
        assert_eq!(id, stable_session_id(server, &renamed));
        assert_ne!(id, stable_session_id(server, &reused));
        assert_ne!(id, stable_session_id(server, &other_id));
        assert_ne!(id, stable_session_id("/other/tmux-501/default", &original));
        // Length-prefixed parts: shifting bytes between fields cannot collide.
        assert_ne!(
            stable_session_id("/tmp/a", &session("$1\t23\t0\t1\tx\tsh")),
            stable_session_id("/tmp/a$", &session("$12\t3\t0\t1\tx\tsh"))
        );
    }

    #[test]
    fn titles_are_bounded_and_never_empty() {
        let long = DiscoveredTmuxSession {
            session_id: HostedSessionId::new(),
            session: session(&format!("$3\t1\t0\t1\t{}\tsh", "é".repeat(400))),
        };
        assert_eq!(long.title().chars().count(), MAX_TITLE_CHARS);
        let unnamed = DiscoveredTmuxSession {
            session_id: HostedSessionId::new(),
            session: session("$4\t1\t0\t1\t\tsh"),
        };
        assert_eq!(unnamed.title(), "tmux $4");
    }

    #[test]
    fn listing_revision_ignores_attach_counts_and_order_but_not_renames() {
        let discovered = |line: &str| {
            let session = session(line);
            DiscoveredTmuxSession {
                session_id: stable_session_id("/tmp/tmux-1/default", &session),
                session,
            }
        };
        let first = [
            discovered("$0\t1\t0\t1\tone\tsh"),
            discovered("$1\t2\t0\t1\ttwo\tsh"),
        ];
        let attached_reordered = [
            discovered("$1\t2\t1\t1\ttwo\tvim"),
            discovered("$0\t1\t3\t4\tone\tsh"),
        ];
        let renamed = [
            discovered("$0\t1\t0\t1\tone\tsh"),
            discovered("$1\t2\t0\t1\tcalled two\tsh"),
        ];
        assert_eq!(
            listing_revision(&first),
            listing_revision(&attached_reordered)
        );
        assert_ne!(listing_revision(&first), listing_revision(&renamed));
        assert_ne!(listing_revision(&first), listing_revision(&first[..1]));
    }

    #[test]
    fn generations_advance_only_when_a_host_goes_away() {
        let mut state = GenerationState::default();
        assert_eq!(state.current(), OccupantGeneration::new(1));
        state.last_started = state.current().get();
        state.live = true;
        assert_eq!(state.current(), OccupantGeneration::new(1));
        state.live = false;
        assert_eq!(state.current(), OccupantGeneration::new(2));
    }

    #[test]
    fn attach_dimensions_are_clamped_to_host_limits() {
        assert_eq!(attach_dimension(0), 1);
        assert_eq!(attach_dimension(80), 80);
        assert_eq!(attach_dimension(u32::MAX), 1_000);
    }

    #[test]
    fn relative_runtime_parent_is_rejected() {
        assert_eq!(
            TmuxSessionSource::system("relative/runtime")
                .unwrap_err()
                .code,
            ListenerErrorCode::InvalidPolicy
        );
    }
}
