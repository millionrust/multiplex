use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

use crate::runtime::{BrowserCancellation, BrowserError};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct OwnedBrowserProcess {
    child: Child,
    profile: Option<TempDir>,
    debug_url: String,
}

impl std::fmt::Debug for OwnedBrowserProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedBrowserProcess")
            .field("pid", &self.child.id())
            .field("profile", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl OwnedBrowserProcess {
    pub(crate) fn launch(
        executable: &Path,
        profile_parent: &Path,
        proxy: std::net::SocketAddr,
        cancellation: &BrowserCancellation,
    ) -> Result<Self, BrowserError> {
        ensure_private_directory(profile_parent)?;
        let profile = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(profile_parent)
            .map_err(|_| BrowserError::Unavailable)?;
        let temp = profile.path().join("tmp");
        fs::create_dir(&temp).map_err(|_| BrowserError::Unavailable)?;
        let mut command = Command::new(executable);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(format!("--proxy-server=http://{proxy}"))
            .arg("--proxy-bypass-list=<-loopback>")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-default-apps")
            .arg("--disable-domain-reliability")
            .arg("--disable-extensions")
            .arg("--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable")
            .arg("--disable-notifications")
            .arg("--disable-quic")
            .arg("--disable-sync")
            .arg("--metrics-recording-only")
            .arg("--no-default-browser-check")
            .arg("--no-first-run")
            .arg("--password-store=basic")
            .arg("--use-mock-keychain")
            .arg("about:blank")
            .env_clear()
            .env("HOME", profile.path())
            .env("TMPDIR", &temp)
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|_| BrowserError::BrowserMissing)?;
        let mut owned = Self {
            child,
            profile: Some(profile),
            debug_url: String::new(),
        };
        owned.debug_url = owned.wait_for_debug_url(cancellation)?;
        Ok(owned)
    }

    pub(crate) fn debug_url(&self) -> &str {
        &self.debug_url
    }

    fn wait_for_debug_url(
        &mut self,
        cancellation: &BrowserCancellation,
    ) -> Result<String, BrowserError> {
        let started = Instant::now();
        let path = self
            .profile
            .as_ref()
            .ok_or(BrowserError::Unavailable)?
            .path()
            .join("DevToolsActivePort");
        while started.elapsed() < START_TIMEOUT {
            if cancellation.is_cancelled() {
                return Err(BrowserError::Cancelled);
            }
            if self
                .child
                .try_wait()
                .map_err(|_| BrowserError::Unavailable)?
                .is_some()
            {
                return Err(BrowserError::Unavailable);
            }
            if let Ok(contents) = fs::read_to_string(&path) {
                let mut lines = contents.lines();
                if let (Some(port), Some(endpoint)) = (lines.next(), lines.next())
                    && port.parse::<u16>().is_ok()
                    && endpoint.starts_with("/devtools/browser/")
                {
                    return Ok(format!("ws://127.0.0.1:{port}{endpoint}"));
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err(BrowserError::Timeout)
    }

    pub(crate) fn stop(&mut self) {
        #[cfg(unix)]
        terminate_group(self.child.id(), libc::SIGTERM);
        if self.child.try_wait().ok().flatten().is_none() {
            #[cfg(not(unix))]
            terminate_owned(&mut self.child, libc::SIGTERM);
            let started = Instant::now();
            while started.elapsed() < STOP_TIMEOUT {
                if self.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                terminate_owned(&mut self.child, libc::SIGKILL);
            }
        }
        let _ = self.child.wait();
        self.profile.take();
    }
}

impl Drop for OwnedBrowserProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(unix)]
fn terminate_group(pid: u32, signal: i32) {
    if let Ok(pid) = i32::try_from(pid) {
        unsafe {
            // SAFETY: the child was launched into a process group whose id is its pid.
            libc::kill(-pid, signal);
        }
    }
}

#[cfg(not(unix))]
fn terminate_group(_pid: u32, _signal: i32) {}

#[cfg(unix)]
fn terminate_owned(child: &mut Child, signal: i32) {
    terminate_group(child.id(), signal);
}

#[cfg(not(unix))]
fn terminate_owned(child: &mut Child, _signal: i32) {
    let _ = child.kill();
}

fn ensure_private_directory(path: &Path) -> Result<(), BrowserError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(BrowserError::Unavailable);
            }
            #[cfg(unix)]
            // SAFETY: geteuid has no preconditions and does not dereference memory.
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(BrowserError::Unavailable);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| BrowserError::Unavailable)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|_| BrowserError::Unavailable)?;
        }
        Err(_) => return Err(BrowserError::Unavailable),
    }
    Ok(())
}

pub(crate) fn discover_browser(explicit: Option<&Path>) -> Result<PathBuf, BrowserError> {
    if let Some(path) = explicit {
        return executable(path).ok_or(BrowserError::BrowserMissing);
    }
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from(
            "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        ),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ]);
    #[cfg(target_os = "linux")]
    candidates.extend(
        [
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    #[cfg(target_os = "windows")]
    if let Some(program_files) = std::env::var_os("PROGRAMFILES") {
        candidates.push(PathBuf::from(program_files).join("Google/Chrome/Application/chrome.exe"));
    }
    candidates
        .into_iter()
        .find_map(|path| executable(&path))
        .ok_or(BrowserError::BrowserMissing)
}

fn executable(path: &Path) -> Option<PathBuf> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn symlinked_profile_parent_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let linked = temp.path().join("linked");
        symlink(&outside, &linked).expect("symlink");
        assert_eq!(
            ensure_private_directory(&linked),
            Err(BrowserError::Unavailable)
        );
    }
}
