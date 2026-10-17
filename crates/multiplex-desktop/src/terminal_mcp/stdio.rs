//! `multiplex terminal-mcp --stdio`: for MCP clients that start a command rather than connect
//! to a URL. Each line on stdin is posted to the running app's server, found through the
//! discovery file, and its answer written as a line on stdout. The app must be running with
//! the server on.

use std::io::{self, BufRead as _, Write as _};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::protocol::error_response;
use super::read_discovery;

pub const COMMAND: &str = "terminal-mcp";
pub const STDIO_FLAG: &str = "--stdio";

pub fn run(app_dir: &Path) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        // Longer than the longest wait_for_output.
        .timeout(Duration::from_secs(150))
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|value| value.get("id").cloned());
        let answer = match read_discovery(app_dir) {
            None => id.map(|id| {
                error_response(
                    id,
                    -32000,
                    "Multiplex is not running with Settings → Terminal MCP server turned on",
                )
                .to_string()
            }),
            Some(server) => {
                let response = client
                    .post(&server.url)
                    .bearer_auth(&server.token)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(line)
                    .send();
                match response {
                    Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED => None,
                    Ok(response) if response.status().is_success() => response.text().ok(),
                    Ok(response) => id.map(|id| {
                        error_response(
                            id,
                            -32000,
                            &format!("the Multiplex app answered {}", response.status()),
                        )
                        .to_string()
                    }),
                    Err(_) => id.map(|id| {
                        error_response(id, -32000, "the Multiplex app could not be reached")
                            .to_string()
                    }),
                }
            }
        };
        if let Some(answer) = answer {
            writeln!(stdout, "{}", answer.trim_end()).map_err(|error| error.to_string())?;
            stdout.flush().map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
