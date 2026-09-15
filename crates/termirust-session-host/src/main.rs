use std::io::Write as _;

use serde::Serialize;
use termirust_session_host::{HostError, LaunchDescriptor, start, stdin_is_pipe};

#[derive(Serialize)]
struct LifecycleLine {
    schema_version: u16,
    lifecycle: &'static str,
    code: &'static str,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        let line = LifecycleLine {
            schema_version: 1,
            lifecycle: "failed",
            code: error.stable_code(),
        };
        let _ = serde_json::to_writer(std::io::stderr(), &line);
        let _ = writeln!(std::io::stderr());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), HostError> {
    if !stdin_is_pipe()? {
        return Err(HostError::new(
            termirust_session_host::HostErrorCode::PermissionDenied,
        ));
    }
    let descriptor = LaunchDescriptor::read(std::io::stdin().lock())?;
    let host = start(descriptor).await?;
    serde_json::to_writer(
        std::io::stdout(),
        &LifecycleLine {
            schema_version: 1,
            lifecycle: "ready",
            code: "host_ready",
        },
    )
    .map_err(|_| HostError::new(termirust_session_host::HostErrorCode::Io))?;
    writeln!(std::io::stdout()).map_err(HostError::io)?;
    std::io::stdout().flush().map_err(HostError::io)?;
    host.wait().await
}
