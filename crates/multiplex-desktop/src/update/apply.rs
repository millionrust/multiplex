//! The updater: `multiplex --apply-update <package> <version> <app pid>`, started by the app
//! from its old copy just before it quits.
//!
//! It waits for the app to exit, puts the staged package in place, restarts the background
//! service from the new files when one is installed, and opens the new version. When anything
//! fails, the old version stays in place and is opened again, and the version is recorded so the
//! app offers the release page instead of trying the same package again.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{Installer, Version, clear_staged, staging_dir};

const FAILED_RECORD: &str = "failed-version";
const PARENT_EXIT_WAIT: Duration = Duration::from_secs(60);

/// Runs the updater with the arguments after `--apply-update`.
pub fn run(arguments: &[String]) {
    let [package, version, parent] = arguments else {
        return;
    };
    let (Some(version), Ok(parent)) = (Version::parse(version), parent.parse::<u32>()) else {
        return;
    };
    let package = PathBuf::from(package);
    let Some(staging) = staging_dir() else {
        return;
    };
    // Only a package this app staged is installed, never a path someone else passed in.
    if !package.starts_with(&staging) || !package.is_file() {
        return;
    }
    wait_for_exit(parent);
    let installer = if package
        .extension()
        .is_some_and(|extension| extension == "msi")
    {
        Installer::WindowsMsi
    } else {
        Installer::MacAppZip
    };
    let outcome = match installer {
        Installer::MacAppZip => install_mac(&package, version),
        Installer::WindowsMsi => install_windows(&package),
    };
    match outcome {
        Ok(app) => {
            clear_staged(&staging);
            crate::controller::background_service::restart_after_update();
            open(&app);
        }
        Err(()) => {
            clear_staged(&staging);
            let _ = fs::create_dir_all(&staging);
            let _ = fs::write(staging.join(FAILED_RECORD), version.to_string());
            if let Ok(executable) = std::env::current_exe() {
                open(&installed_app(&executable).unwrap_or(executable));
            }
        }
    }
}

/// The version whose install failed last, so the app points at the release page for it instead.
pub fn failed_version(staging: &Path) -> Option<Version> {
    Version::parse(fs::read_to_string(staging.join(FAILED_RECORD)).ok()?.trim())
}

/// What to open afterwards: the `.app` bundle on macOS, the executable elsewhere.
fn installed_app(executable: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        super::mac_bundle(executable)
    } else {
        Some(executable.to_path_buf())
    }
}

#[cfg(unix)]
fn wait_for_exit(pid: u32) {
    let deadline = Instant::now() + PARENT_EXIT_WAIT;
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: signal 0 only asks whether the process exists; nothing is delivered.
    while Instant::now() < deadline && unsafe { libc::kill(pid, 0) } == 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: the handle is checked before use and closed once; a process that already exited
    // gives no handle, which is the same as having waited.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            WaitForSingleObject(handle, PARENT_EXIT_WAIT.as_millis() as u32);
            CloseHandle(handle);
        }
    }
    let _ = Instant::now();
}

#[cfg(not(any(unix, windows)))]
fn wait_for_exit(_: u32) {}

/// Unpacks the zip beside the running bundle, checks it is the version expected, and swaps it
/// in. The old bundle is moved aside first and put back if the swap fails.
fn install_mac(package: &Path, version: Version) -> Result<PathBuf, ()> {
    let executable = std::env::current_exe().map_err(drop)?;
    let bundle = super::mac_bundle(&executable).ok_or(())?;
    let parent = bundle.parent().ok_or(())?;
    let work = parent.join(format!(".Multiplex-update-{version}"));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(drop)?;
    let result = (|| {
        run_tool("/usr/bin/ditto", &["-x", "-k"], &[package, &work])?;
        let unpacked = work.join("Multiplex.app");
        if bundle_version(&unpacked) != Some(version) {
            return Err(());
        }
        let previous = parent.join(format!(".Multiplex-previous-{}.app", std::process::id()));
        fs::rename(&bundle, &previous).map_err(drop)?;
        if fs::rename(&unpacked, &bundle).is_err() {
            let _ = fs::rename(&previous, &bundle);
            return Err(());
        }
        let _ = fs::remove_dir_all(&previous);
        // Downloaded by this app, not a browser, but clear it anyway so Gatekeeper does not ask.
        let _ = run_tool(
            "/usr/bin/xattr",
            &["-dr", "com.apple.quarantine"],
            &[&bundle],
        );
        Ok(bundle.clone())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// `CFBundleShortVersionString` of an unpacked bundle.
fn bundle_version(bundle: &Path) -> Option<Version> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| Version::parse(String::from_utf8_lossy(&output.stdout).trim()))
        .flatten()
}

/// Runs the per-user MSI with a progress bar and no reboot. The installer replaces the files in
/// the folder this copy runs from, so the same path opens the new version.
fn install_windows(package: &Path) -> Result<PathBuf, ()> {
    let executable = std::env::current_exe().map_err(drop)?;
    crate::controller::background_service::stop_for_update();
    let status = std::process::Command::new("msiexec")
        .arg("/i")
        .arg(package)
        .args(["/passive", "/norestart"])
        .status()
        .map_err(drop)?;
    // 3010: installed, and Windows would like a restart for some other program's sake.
    match status.code() {
        Some(0 | 3010) => Ok(executable),
        _ => Err(()),
    }
}

fn run_tool(tool: &str, arguments: &[&str], paths: &[&Path]) -> Result<(), ()> {
    let status = std::process::Command::new(tool)
        .args(arguments)
        .args(paths)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(drop)?;
    status.success().then_some(()).ok_or(())
}

fn open(app: &Path) {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(app);
        command
    } else {
        std::process::Command::new(app)
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let _ = command.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_version_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(failed_version(dir.path()), None);
        fs::write(dir.path().join(FAILED_RECORD), "0.0.4\n").unwrap();
        assert_eq!(failed_version(dir.path()), Some(Version(0, 0, 4)));
    }

    #[test]
    fn nothing_runs_without_the_three_arguments() {
        run(&[]);
        run(&["only-one".to_owned()]);
    }
}
