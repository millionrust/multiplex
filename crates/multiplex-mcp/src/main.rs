use std::process::ExitCode;
use std::sync::Arc;

use multiplex_mcp::{LocalInspectionSource, McpServer, ServerConfiguration, run_stdio};

fn main() -> ExitCode {
    let configuration = match ServerConfiguration::from_environment() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("multiplex-mcp configuration error: {error}");
            return ExitCode::from(2);
        }
    };
    let source = match LocalInspectionSource::discover() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("multiplex-mcp startup error: {error}");
            return ExitCode::from(3);
        }
    };
    let capabilities = configuration.capabilities.clone();
    let server = McpServer::new(Arc::new(source), configuration);
    eprintln!(
        "multiplex-mcp started with capabilities: {}",
        capabilities.display_names().join(",")
    );
    match run_stdio(server, std::io::stdin(), std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("multiplex-mcp transport error: {error}");
            ExitCode::from(7)
        }
    }
}
