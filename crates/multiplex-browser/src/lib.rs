//! Isolated, ephemeral browser execution with deny-by-default network policy.

mod network;
mod process;
mod runtime;

pub use network::{ApprovedOrigin, NetworkPolicy};
pub use runtime::{
    BrowserArtifact, BrowserArtifactKind, BrowserCancellation, BrowserError, BrowserRequest,
    BrowserRuntime, BrowserRuntimeConfig, BrowserRuntimeStatus,
};

#[cfg(test)]
mod test_support {
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    /// What the machine was like at the moment a timing test failed, so a failure caused by a
    /// starved CI runner can be told apart from one in this code.
    ///
    /// These tests pass everywhere except inside the full Windows job, and there only after the
    /// desktop application's tests have run. A 10 ms sleep that wakes far too late says the
    /// process could not get a core; a loopback connection that is slow to open, or does not
    /// open, says the trouble is in the network stack instead. Evaluated only on failure.
    pub(crate) fn machine_state() -> String {
        let started = Instant::now();
        std::thread::sleep(Duration::from_millis(10));
        let slept = started.elapsed();
        let connect = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|listener| {
            let address = listener.local_addr()?;
            let started = Instant::now();
            TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
            Ok(started.elapsed())
        });
        match connect {
            Ok(opened) => format!(
                "machine: a 10 ms sleep took {slept:?}, a loopback connection opened in {opened:?}"
            ),
            Err(error) => format!(
                "machine: a 10 ms sleep took {slept:?}, a loopback connection failed: {error}"
            ),
        }
    }

    /// It only ever runs inside a failing assertion, where a panic of its own would replace the
    /// failure it was meant to explain.
    #[test]
    fn machine_state_reports_both_measurements_without_panicking() {
        let state = machine_state();
        assert!(state.contains("10 ms sleep took"), "{state}");
        assert!(state.contains("loopback connection"), "{state}");
    }
}
