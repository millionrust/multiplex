use std::process::ExitCode;
use std::sync::Arc;

use termirust_mcp::{LocalInspectionSource, McpServer, ServerConfiguration, run_stdio};

fn main() -> ExitCode {
    let configuration = match ServerConfiguration::from_environment() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("termirust-mcp configuration error: {error}");
            return ExitCode::from(2);
        }
    };
    let source = match LocalInspectionSource::discover() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("termirust-mcp startup error: {error}");
            return ExitCode::from(3);
        }
    };
    let capabilities = configuration.capabilities.clone();
    let server = McpServer::new(Arc::new(source), configuration);
    eprintln!(
        "termirust-mcp started with capabilities: {}",
        capabilities.display_names().join(",")
    );
    match run_stdio(server, std::io::stdin(), std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("termirust-mcp transport error: {error}");
            ExitCode::from(7)
        }
    }
}
