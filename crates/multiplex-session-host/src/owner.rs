//! An optional owning process whose exit stops a Host.
//!
//! A Host is detached on purpose so it outlives the frontend that started it. A test that starts
//! one cannot rely on its own cleanup running: a runner that times a test out kills its process,
//! and an abort skips every `Drop`. Such a Host was left running for good, holding a shell and a
//! runtime slot. Test builds name their own process here, and the Host stops itself once that
//! process is gone. Nothing shipped sets the variable, so a real Host is never tied to anything.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// The process id whose exit stops the Host.
pub const OWNER_PROCESS_VARIABLE: &str = "MULTIPLEX_SESSION_HOST_OWNER_PID";

const OWNER_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The owner named in this process's environment, if any.
pub(crate) fn owner_from_environment() -> Option<u32> {
    std::env::var(OWNER_PROCESS_VARIABLE)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|pid| *pid != 0)
}

/// Cancels `stop` once `owner` has exited, and returns when either happens.
pub(crate) async fn stop_when_owner_exits(owner: u32, stop: CancellationToken) {
    loop {
        if !process_alive(owner) {
            stop.cancel();
            return;
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(OWNER_POLL_INTERVAL) => {}
        }
    }
}

#[cfg(unix)]
pub(crate) fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 checks the process exists without touching it; EPERM means it exists but belongs
    // to someone else, which still counts as alive.
    (unsafe { libc::kill(pid, 0) }) == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
pub(crate) fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0u32;
    let queried = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
    unsafe { CloseHandle(handle) };
    queried && code == STILL_ACTIVE as u32
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_an_exited_child_is_not() {
        assert!(process_alive(std::process::id()));
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[][..]
            })
            .spawn()
            .expect("a short child should start");
        let pid = child.id();
        child.wait().expect("the child should exit");
        assert!(!process_alive(pid));
    }

    #[tokio::test]
    async fn the_stop_token_is_cancelled_once_the_owner_is_gone() {
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[][..]
            })
            .spawn()
            .expect("a short child should start");
        let pid = child.id();
        child.wait().expect("the child should exit");
        let stop = CancellationToken::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            stop_when_owner_exits(pid, stop.clone()),
        )
        .await
        .expect("the watch should end once the owner has exited");
        assert!(stop.is_cancelled());
    }

    #[tokio::test]
    async fn a_living_owner_keeps_the_host_running() {
        let stop = CancellationToken::new();
        let watch = tokio::spawn(stop_when_owner_exits(std::process::id(), stop.clone()));
        tokio::time::sleep(OWNER_POLL_INTERVAL * 3).await;
        assert!(!stop.is_cancelled());
        stop.cancel();
        watch
            .await
            .expect("the watch should end when the Host stops");
    }
}
