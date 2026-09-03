use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::McpServer;

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_JOIN_HANDLES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    Input,
    Output,
    Worker,
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Input => "MCP input failed",
            Self::Output => "MCP output failed",
            Self::Worker => "MCP request worker failed",
        })
    }
}

impl std::error::Error for TransportError {}

pub fn run_stdio<R, W>(server: McpServer, input: R, output: W) -> Result<(), TransportError>
where
    R: Read,
    W: Write + Send + 'static,
{
    let mut reader = BufReader::new(input);
    let output = Arc::new(Mutex::new(output));
    let mut workers = Vec::new();
    loop {
        let line = match read_bounded_line(&mut reader)? {
            LineRead::Eof => break,
            LineRead::Oversize => {
                write_response(&output, &parse_error("request exceeds the transport limit"))?;
                continue;
            }
            LineRead::Line(line) => line,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let message = match serde_json::from_slice::<Value>(&line) {
            Ok(value) => value,
            Err(_) => {
                write_response(&output, &parse_error("invalid JSON"))?;
                continue;
            }
        };
        if McpServer::is_tool_call(&message) {
            if workers.len() >= MAX_JOIN_HANDLES {
                write_response(&output, &resource_error(message.get("id").cloned()))?;
                continue;
            }
            if !server.reserve_tool_call(&message) {
                write_response(&output, &resource_error(message.get("id").cloned()))?;
                continue;
            }
            let server = server.clone();
            let output = Arc::clone(&output);
            workers.push(thread::spawn(move || -> Result<(), TransportError> {
                if let Some(response) = server.process(message) {
                    write_response(&output, &response)?;
                }
                Ok(())
            }));
        } else if let Some(response) = server.process(message) {
            write_response(&output, &response)?;
        }
        reap_finished(&mut workers)?;
    }
    for worker in workers {
        worker.join().map_err(|_| TransportError::Worker)??;
    }
    Ok(())
}

enum LineRead {
    Eof,
    Line(Vec<u8>),
    Oversize,
}

fn read_bounded_line(reader: &mut impl io::BufRead) -> Result<LineRead, TransportError> {
    let mut bytes = Vec::new();
    let mut oversize = false;
    let mut consumed_any = false;
    loop {
        let available = reader.fill_buf().map_err(|_| TransportError::Input)?;
        if available.is_empty() {
            return if !consumed_any {
                Ok(LineRead::Eof)
            } else if oversize {
                Ok(LineRead::Oversize)
            } else {
                Ok(LineRead::Line(bytes))
            };
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let chunk = &available[..consumed];
        consumed_any = true;
        if !oversize {
            let remaining = MAX_REQUEST_BYTES.saturating_sub(bytes.len());
            if chunk.len() <= remaining {
                bytes.extend_from_slice(chunk);
            } else {
                bytes.extend_from_slice(&chunk[..remaining]);
                oversize = true;
            }
        }
        let ended = chunk.last() == Some(&b'\n');
        reader.consume(consumed);
        if ended {
            if !oversize {
                bytes.pop();
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                return Ok(LineRead::Line(bytes));
            }
            return Ok(LineRead::Oversize);
        }
    }
}

fn write_response<W: Write>(
    output: &Arc<Mutex<W>>,
    response: &Value,
) -> Result<(), TransportError> {
    let mut bytes = serde_json::to_vec(response).map_err(|_| TransportError::Output)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(TransportError::Output);
    }
    bytes.push(b'\n');
    let mut output = output.lock().map_err(|_| TransportError::Output)?;
    output
        .write_all(&bytes)
        .and_then(|()| output.flush())
        .map_err(|_| TransportError::Output)
}

fn reap_finished(
    workers: &mut Vec<thread::JoinHandle<Result<(), TransportError>>>,
) -> Result<(), TransportError> {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            worker.join().map_err(|_| TransportError::Worker)??;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn parse_error(message: &'static str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": -32700, "message": message },
    })
}

fn resource_error(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": -32001, "message": "request capacity is exhausted" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::backend::{InspectionPage, InspectionRequest, InspectionSource, SourceError};
    use crate::{CapabilitySet, ServerConfiguration};
    use termirust_cli::Cancellation;

    #[derive(Default)]
    struct FakeSource;

    impl InspectionSource for FakeSource {
        fn inspect(
            &self,
            _: InspectionRequest,
            _: usize,
            _: usize,
            _: &Cancellation,
        ) -> Result<InspectionPage, SourceError> {
            Ok(InspectionPage {
                data: json!({ "ok": true }),
                next_offset: None,
            })
        }
    }

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let mut bytes = self
                .0
                .lock()
                .map_err(|_| io::Error::other("poisoned writer"))?;
            bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn server() -> McpServer {
        McpServer::new(
            Arc::new(FakeSource),
            ServerConfiguration {
                capabilities: CapabilitySet::all(),
                ..ServerConfiguration::default()
            },
        )
    }

    #[test]
    fn stdio_is_newline_delimited_and_writes_only_json_rpc() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let writer = SharedWriter::default();
        let captured = writer.clone();
        run_stdio(server(), input.as_bytes(), writer).expect("stdio run succeeds");
        let bytes = captured.0.lock().expect("captured output");
        let lines = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty());
        let messages = lines
            .map(|line| serde_json::from_slice::<Value>(line).expect("valid JSON-RPC"))
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], 1);
        assert_eq!(messages[1]["id"], 2);
    }

    #[test]
    fn oversized_input_is_discarded_without_desynchronizing_the_next_message() {
        let mut input = vec![b'x'; MAX_REQUEST_BYTES + 1];
        input.push(b'\n');
        input.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        );
        let writer = SharedWriter::default();
        let captured = writer.clone();
        run_stdio(server(), input.as_slice(), writer).expect("stdio recovers");
        let output = String::from_utf8(captured.0.lock().expect("captured output").clone())
            .expect("utf8 output");
        assert!(output.contains("request exceeds the transport limit"));
        assert!(output.contains("\"protocolVersion\":\"2025-11-25\""));
    }
}
