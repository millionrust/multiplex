use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::{Child, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::storage::set_test_app_dir_override;

const TEST_SSH_IMAGE: &str = "termirust-e2e-sshd:local";
const TEST_SSH_USER: &str = "termirust";
const TEST_SSH_PASSWORD: &str = "termirust-pass";
pub const TEST_POLL_INTERVAL: Duration = Duration::from_millis(20);

fn test_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn dialog_paths() -> &'static Mutex<VecDeque<Option<PathBuf>>> {
    static PATHS: OnceLock<Mutex<VecDeque<Option<PathBuf>>>> = OnceLock::new();
    PATHS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", std::process::id(), nanos)
}

fn command_error(program: &str, output: std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!(
        "{program} failed with status {}.\nstdout:\n{}\nstderr:\n{}",
        output.status, stdout, stderr
    )
}

fn run_command(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to execute {program} {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(command_error(program, output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ensure_test_ssh_image() -> Result<(), String> {
    static BUILD: OnceLock<Result<(), String>> = OnceLock::new();
    BUILD
        .get_or_init(|| {
            if run_command("docker", &["image", "inspect", TEST_SSH_IMAGE], None).is_ok() {
                return Ok(());
            }
            let fixture_dir =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-server");
            let status = Command::new("docker")
                .args(["build", "-t", TEST_SSH_IMAGE, "."])
                .current_dir(&fixture_dir)
                .status()
                .map_err(|error| format!("failed to execute docker build: {error}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("docker build failed with status {status}"))
            }
        })
        .clone()
}

pub struct TestIsolation {
    _lock: MutexGuard<'static, ()>,
    temp_dir: PathBuf,
    previous_config_dir: Option<PathBuf>,
}

impl TestIsolation {
    pub fn acquire() -> Self {
        let lock = test_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp_dir = std::env::temp_dir().join(format!("termirust-test-{}", now_millis()));
        fs::create_dir_all(&temp_dir).expect("unable to create test config dir");
        dialog_paths()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        let previous_config_dir = set_test_app_dir_override(Some(temp_dir.clone()));

        Self {
            _lock: lock,
            temp_dir,
            previous_config_dir,
        }
    }
}

impl Drop for TestIsolation {
    fn drop(&mut self) {
        dialog_paths()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        set_test_app_dir_override(self.previous_config_dir.clone());
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

pub struct DockerSshServer {
    container_name: String,
    pub port: u16,
}

#[cfg(unix)]
pub struct TestSshAgent {
    child: Child,
    _directory: tempfile::TempDir,
    socket_path: PathBuf,
}

#[cfg(unix)]
impl TestSshAgent {
    pub fn start_empty() -> Result<Self, String> {
        let directory = tempfile::TempDir::new()
            .map_err(|error| format!("unable to create SSH-agent test directory: {error}"))?;
        let socket_path = directory.path().join("agent.sock");
        let mut child = Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                format!("unable to start the local SSH-agent test fixture: {error}")
            })?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if socket_path.exists() {
                return Ok(Self {
                    child,
                    _directory: directory,
                    socket_path,
                });
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("unable to inspect SSH-agent fixture: {error}"))?
            {
                return Err(format!(
                    "local SSH-agent test fixture exited before creating its socket: {status}"
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err("timed out waiting for the local SSH-agent test fixture".to_string())
    }

    pub fn start_with_fixture_key() -> Result<Self, String> {
        let agent = Self::start_empty()?;
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-server/id_ed25519");
        let key = agent._directory.path().join("authorized-test-key");
        fs::copy(source, &key)
            .map_err(|error| format!("unable to prepare SSH-agent test key: {error}"))?;
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("unable to secure SSH-agent test key: {error}"))?;
        agent.add_key(&key)?;
        Ok(agent)
    }

    pub fn start_with_untrusted_key() -> Result<Self, String> {
        let agent = Self::start_empty()?;
        let key = agent._directory.path().join("untrusted-test-key");
        let output = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", ""])
            .arg("-f")
            .arg(&key)
            .output()
            .map_err(|error| format!("unable to generate SSH-agent test key: {error}"))?;
        if !output.status.success() {
            return Err(command_error("ssh-keygen", output));
        }
        agent.add_key(&key)?;
        Ok(agent)
    }

    fn add_key(&self, key: &Path) -> Result<(), String> {
        let output = Command::new("ssh-add")
            .arg(key)
            .env("SSH_AUTH_SOCK", &self.socket_path)
            .output()
            .map_err(|error| format!("unable to add an SSH-agent test identity: {error}"))?;
        if !output.status.success() {
            return Err(command_error("ssh-add", output));
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

#[cfg(unix)]
impl Drop for TestSshAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl DockerSshServer {
    pub fn docker_available() -> bool {
        run_command("docker", &["info"], None).is_ok()
    }

    pub fn start() -> Result<Self, String> {
        Self::start_with_port_mapping("127.0.0.1::22")
    }

    pub fn start_on_port(port: u16) -> Result<Self, String> {
        Self::start_with_port_mapping(&format!("127.0.0.1:{port}:22"))
    }

    fn start_with_port_mapping(port_mapping: &str) -> Result<Self, String> {
        ensure_test_ssh_image()?;

        let container_name = format!("termirust-e2e-sshd-{}", unique_suffix());
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-server");
        let ca_mount = format!(
            "{}:/etc/ssh/termirust_test_ca.pub:ro",
            fixture_dir.join("id_ed25519.pub").display()
        );
        let policy_mount = format!(
            "{}:/etc/ssh/sshd_config.d/termirust-test-ca.conf:ro",
            fixture_dir.join("certificate-auth.conf").display()
        );
        run_command(
            "docker",
            &[
                "run",
                "--detach",
                "--rm",
                "--name",
                &container_name,
                "--volume",
                &ca_mount,
                "--volume",
                &policy_mount,
                "-p",
                port_mapping,
                TEST_SSH_IMAGE,
            ],
            None,
        )?;

        let port = run_command(
            "docker",
            &[
                "inspect",
                "-f",
                "{{(index (index .NetworkSettings.Ports \"22/tcp\") 0).HostPort}}",
                &container_name,
            ],
            None,
        )?
        .parse::<u16>()
        .map_err(|error| format!("unable to parse docker port mapping: {error}"))?;

        let server = Self {
            container_name,
            port,
        };
        if let Err(error) = server.wait_until_ready() {
            server.stop();
            return Err(error);
        }
        Ok(server)
    }

    pub fn host(&self) -> &str {
        "127.0.0.1"
    }

    pub fn username(&self) -> &str {
        TEST_SSH_USER
    }

    pub fn password(&self) -> &str {
        TEST_SSH_PASSWORD
    }

    pub fn exec(&self, command: &str) -> Result<String, String> {
        run_command(
            "docker",
            &["exec", &self.container_name, "sh", "-lc", command],
            None,
        )
    }

    pub fn stop(&self) {
        let _ = run_command("docker", &["rm", "-f", &self.container_name], None);
    }

    fn wait_until_ready(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));

        while Instant::now() < deadline {
            if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(250))
            {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                let mut buffer = [0_u8; 128];
                if let Ok(bytes_read) = stream.read(&mut buffer) {
                    let banner = String::from_utf8_lossy(&buffer[..bytes_read]);
                    if banner.starts_with("SSH-2.0-") {
                        return Ok(());
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let logs = run_command("docker", &["logs", &self.container_name], None)
            .unwrap_or_else(|error| error);
        Err(format!(
            "timed out waiting for Docker SSH server on port {}.\n{}",
            self.port, logs
        ))
    }
}

pub fn allocate_local_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("unable to bind local test port")
        .local_addr()
        .expect("unable to read local test port")
        .port()
}

pub fn create_test_user_certificate(
    directory: &Path,
    principal: &str,
    trusted_signer: bool,
) -> PathBuf {
    use russh::keys::ssh_key::certificate::{Builder, CertType};

    let fixture_key_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-server/id_ed25519");
    let subject = russh::keys::load_secret_key(&fixture_key_path, None)
        .expect("unable to load certificate test subject key");
    let untrusted_signer;
    let signer = if trusted_signer {
        &subject
    } else {
        untrusted_signer = russh::keys::PrivateKey::random(
            &mut rand::rngs::OsRng,
            russh::keys::Algorithm::Ed25519,
        )
        .expect("unable to generate untrusted certificate signer");
        &untrusted_signer
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after Unix epoch")
        .as_secs();
    let mut builder = Builder::new_with_random_nonce(
        &mut rand::rngs::OsRng,
        subject.public_key().key_data().clone(),
        now.saturating_sub(60),
        now + 3600,
    )
    .expect("unable to create certificate builder");
    builder
        .cert_type(CertType::User)
        .expect("unable to set user certificate type");
    builder
        .key_id("termirust-docker-test")
        .expect("unable to set certificate key id");
    builder
        .valid_principal(principal.to_string())
        .expect("unable to set certificate principal");
    builder
        .extension("permit-port-forwarding", "")
        .expect("unable to permit certificate port forwarding");
    let certificate = builder
        .sign(signer)
        .expect("unable to sign test user certificate");
    let path = directory.join(if trusted_signer {
        "trusted-user-cert.pub"
    } else {
        "untrusted-user-cert.pub"
    });
    certificate
        .write_file(&path)
        .expect("unable to write test user certificate");
    path
}

pub fn queue_dialog_path(path: Option<PathBuf>) {
    dialog_paths()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(path);
}

pub fn take_dialog_path() -> Option<PathBuf> {
    take_dialog_selection().flatten()
}

pub fn take_dialog_selection() -> Option<Option<PathBuf>> {
    dialog_paths()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pop_front()
}

impl Drop for DockerSshServer {
    fn drop(&mut self) {
        self.stop();
    }
}
