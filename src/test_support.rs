use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::storage::set_test_app_dir_override;

const TEST_SSH_IMAGE: &str = "termirust-e2e-sshd:local";
const TEST_SSH_USER: &str = "termirust";
const TEST_SSH_PASSWORD: &str = "termirust-pass";

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
            let fixture_dir =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-server");
            run_command(
                "docker",
                &["build", "-t", TEST_SSH_IMAGE, "."],
                Some(&fixture_dir),
            )
            .map(|_| ())
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
        let lock = test_mutex().lock().expect("test isolation lock poisoned");
        let temp_dir = std::env::temp_dir().join(format!("termirust-test-{}", now_millis()));
        fs::create_dir_all(&temp_dir).expect("unable to create test config dir");
        dialog_paths()
            .lock()
            .expect("dialog path queue lock poisoned")
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
            .expect("dialog path queue lock poisoned")
            .clear();
        set_test_app_dir_override(self.previous_config_dir.clone());
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

pub struct DockerSshServer {
    container_name: String,
    pub port: u16,
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
        run_command(
            "docker",
            &[
                "run",
                "--detach",
                "--rm",
                "--name",
                &container_name,
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

pub fn queue_dialog_path(path: Option<PathBuf>) {
    dialog_paths()
        .lock()
        .expect("dialog path queue lock poisoned")
        .push_back(path);
}

pub fn take_dialog_path() -> Option<PathBuf> {
    dialog_paths()
        .lock()
        .expect("dialog path queue lock poisoned")
        .pop_front()
        .flatten()
}

impl Drop for DockerSshServer {
    fn drop(&mut self) {
        self.stop();
    }
}
