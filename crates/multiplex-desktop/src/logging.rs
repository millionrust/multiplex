//! Local desktop process logs. Protocol workers keep their original stdout/stderr pipes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

struct DesktopLogger;
static LOGGER: DesktopLogger = DesktopLogger;

impl log::Log for DesktopLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        eprintln!(
            "{} {} {}: {}",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        let _ = io::stderr().flush();
    }
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Keep this file alive while the desktop runs: Windows standard handles refer to it.
pub fn initialize(app_root: &Path) -> io::Result<File> {
    let root = app_root.join("logs");
    fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    let file = open_log(&root)?;
    redirect_output(&file)?;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    eprintln!(
        "{} desktop_start version={} pid={}",
        timestamp(),
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    Ok(file)
}

fn open_log(root: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(root.join(format!(
        "multiplex-{}-{}.log",
        timestamp(),
        std::process::id()
    )))
}

#[cfg(unix)]
fn redirect_output(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    for descriptor in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::dup2(file.as_raw_fd(), descriptor) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn redirect_output(file: &File) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle as _, IntoRawHandle as _};
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    // The CRT owns this duplicate after open_osfhandle; keep the Rust File's handle separate.
    let handle = file.try_clone()?.into_raw_handle();
    let descriptor =
        unsafe { libc::open_osfhandle(handle as libc::intptr_t, libc::O_APPEND | libc::O_BINARY) };
    if descriptor == -1 {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        for target in [1, 2] {
            if unsafe { libc::dup2(descriptor, target) } == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        for target in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if unsafe { SetStdHandle(target, file.as_raw_handle()) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    })();
    unsafe {
        libc::close(descriptor);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_output_errors_and_panics_reach_the_local_log() {
        let root = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "logging::tests::logging_child", "--nocapture"])
            .env("MULTIPLEX_LOGGING_TEST_ROOT", root.path())
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let path = std::fs::read_dir(root.path().join("logs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(path).unwrap();
        for expected in [
            "desktop_start",
            "stdout-marker",
            "stderr-marker",
            "error-marker",
            "panic-marker",
        ] {
            assert!(text.contains(expected), "missing {expected} in {text:?}");
        }
    }

    #[test]
    fn logging_child() {
        let Some(root) = std::env::var_os("MULTIPLEX_LOGGING_TEST_ROOT") else {
            return;
        };
        let _file = super::initialize(std::path::Path::new(&root)).unwrap();
        println!("stdout-marker");
        eprintln!("stderr-marker");
        log::error!("error-marker");
        let _ = std::panic::catch_unwind(|| panic!("panic-marker"));
    }

    #[test]
    fn process_log_is_a_private_append_only_log_file() {
        use std::io::Write as _;
        let root = tempfile::tempdir().unwrap();
        let mut file = super::open_log(root.path()).unwrap();
        writeln!(file, "termination failed: ConnectionRefused").unwrap();
        writeln!(file, "termination retry succeeded").unwrap();
        let path = std::fs::read_dir(root.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(path.extension().unwrap(), "log");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "termination failed: ConnectionRefused\ntermination retry succeeded\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
