//! Ties each local pane's pseudo-console and shell to the life of this process.
//!
//! A pane closes its pseudo-console when its session ends, but a process that exits with panes
//! still open never gets there: its session threads are simply stopped. The console host the
//! system started for such a pane then outlives it, and on Windows Server 2022 it spins a whole
//! core doing so, indefinitely. Five of them left behind by one run of the desktop tests held a
//! four-core runner at 100% until the job ended. A job object that kills what it holds when its
//! last handle closes ends them with the process however it ends; this process holds the only
//! handle and never closes it, so that happens exactly when the process exits.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

/// The console hosts a pseudo-console runs in: the system's, and the one Windows Terminal
/// ships, which a `conpty.dll` placed beside the executable starts instead.
const CONSOLE_HOSTS: [&str; 2] = ["conhost.exe", "openconsole.exe"];

struct Job(HANDLE);

// A job handle is a kernel handle, usable from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

pub(super) fn job() -> Option<HANDLE> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls on a handle this function owns; the limits structure is
        // zeroed, which is its documented empty value, before the one flag is set.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .expect("the limits structure is small"),
            );
            if set == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    })
    .as_ref()
    .map(|job| job.0)
}

/// Puts the shell and every console host this process has started into the job. Called once a
/// pane's shell is running, when both exist. A console host belonging to another pane may be
/// taken along; it belongs in the same job, and assigning one twice changes nothing. Programs the
/// shell starts join the job with it.
pub(super) fn adopt(shell_process_id: Option<u32>) {
    let Some(job) = job() else {
        return;
    };
    let mut processes = console_hosts_started_here();
    processes.extend(shell_process_id);
    for process_id in processes {
        // SAFETY: the handle is opened, used once, and closed here.
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
            if process.is_null() {
                continue;
            }
            AssignProcessToJobObject(job, process);
            CloseHandle(process);
        }
    }
}

fn console_hosts_started_here() -> Vec<u32> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is closed before returning, and each entry is read only after
    // the call that filled it succeeded.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }
        let this_process = GetCurrentProcessId();
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).expect("the entry is small");
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if entry.th32ParentProcessID == this_process {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
                if CONSOLE_HOSTS
                    .iter()
                    .any(|host| name.eq_ignore_ascii_case(host))
                {
                    found.push(entry.th32ProcessID);
                }
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found
}

#[cfg(test)]
mod tests {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    use super::{adopt, console_hosts_started_here, job};

    #[test]
    fn a_pseudo_consoles_host_and_shell_are_held_by_the_job() {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("a pseudo-console opens");
        let mut command = CommandBuilder::new(crate::test_support::test_shell_program());
        command.args(["-c", "sleep 5"]);
        let mut shell = pair.slave.spawn_command(command).expect("the shell starts");
        adopt(shell.process_id());

        let job = job().expect("the job was created");
        let hosts = console_hosts_started_here();
        assert!(
            !hosts.is_empty(),
            "no console host was found for the pseudo-console"
        );
        for process_id in hosts.into_iter().chain(shell.process_id()) {
            // SAFETY: the handle is opened, queried once, and closed here.
            let inside = unsafe {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
                assert!(
                    !process.is_null(),
                    "process {process_id} could not be opened"
                );
                let mut inside = 0;
                IsProcessInJob(process, job, &mut inside);
                CloseHandle(process);
                inside
            };
            assert_ne!(inside, 0, "process {process_id} is not in the job");
        }
        let _ = shell.kill();
    }
}
