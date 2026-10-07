//! Browser terminal operations over authenticated Session Hosts. No shell commands or caller-supplied paths.
use crate::CliPaths;
use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
use multiplex_domain::{CommandId, HostedSessionId, OutputSequence};
use multiplex_host_protocol::wire;
use multiplex_store::AtomicWriter as _;
use rand::RngCore as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    paste: bool,
    #[serde(default)]
    plain: bool,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    cols: Option<u32>,
    #[serde(default)]
    rows: Option<u32>,
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    name: Option<String>,
}

fn read_saved_size(path: &std::path::Path) -> Option<(u32, u32)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 128 {
        return None;
    }
    let size: (u32, u32) = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    ((1..=500).contains(&size.0) && (1..=200).contains(&size.1)).then_some(size)
}

async fn send_input(
    client: &mut HostClient,
    data: String,
    paste: bool,
    bracketed: bool,
    cancel: &CancellationToken,
) -> Result<(), &'static str> {
    let maximum = if paste { 1024 * 1024 } else { 4096 };
    if data.is_empty() || data.len() > maximum {
        return Err("Input too large or empty");
    }
    if !client
        .set_writer_lease(CommandId::new(), true, cancel)
        .await
        .map_err(|_| "Terminal control unavailable")?
    {
        return Err("Another client controls this terminal");
    }
    let bytes = if paste && bracketed {
        format!("\x1b[200~{data}\x1b[201~").into_bytes()
    } else {
        data.into_bytes()
    };
    for chunk in bytes.chunks(4096) {
        client
            .input(CommandId::new(), chunk.to_vec(), cancel)
            .await
            .map_err(|_| "Could not send input")?;
    }
    let _ = client
        .set_writer_lease(CommandId::new(), false, cancel)
        .await;
    Ok(())
}

pub(crate) async fn execute(paths: &CliPaths, request: Request) -> Result<Value, &'static str> {
    let root = paths
        .config_root()
        .join(multiplex_store::CONSOLE_SESSIONS_DIR);
    if request.action == "list" {
        let sessions = multiplex_store::live_console_sessions(&root, paths.runtime_parent());
        return Ok(json!({"panes":sessions.iter().map(|live| json!({
            "id":live.record.session_id.to_string(), "sessionId":live.record.session_id.to_string(),
            "sessionName":live.record.title(), "command":live.record.program,
            "path":live.record.working_directory, "live":true,
            "width":80, "height":24, "windowIndex":0, "paneIndex":0,
            "pinned":root.join(live.record.session_id.to_string()).join("browser-pinned").is_file()
        })).collect::<Vec<_>>() }));
    }
    let id = request
        .id
        .as_deref()
        .ok_or("Terminal id required")?
        .parse::<uuid::Uuid>()
        .map_err(|_| "Invalid terminal id")?;
    let id = HostedSessionId::from_uuid(id);
    let session_dir = root.join(id.to_string());
    multiplex_store::console_session_generation(&root, paths.runtime_parent(), id)
        .ok_or("Terminal no longer exists")?;
    let record =
        multiplex_store::read_console_session(&session_dir).ok_or("Terminal no longer exists")?;
    let expected = multiplex_store::read_host_metadata(&session_dir)
        .map_err(|_| "Host metadata unavailable")?
        .host_instance_id;
    if request.action == "pin" {
        let pin = session_dir.join("browser-pinned");
        if request.pinned.ok_or("Pin value required")? {
            multiplex_store::SystemAtomicWriter
                .write(&pin, b"1")
                .map_err(|_| "Could not pin terminal")?;
        } else if let Err(error) = std::fs::remove_file(pin)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err("Could not unpin terminal");
        }
        return Ok(json!({"ok":true}));
    }
    if request.action == "rename" {
        let name = request.name.as_deref().ok_or("Name required")?.trim();
        let mut original = record;
        original.title_override = None;
        let title = if name.is_empty() {
            original.title()
        } else {
            name.to_string()
        };
        multiplex_store::rename_console_session(&session_dir, id, &title)
            .map_err(|_| "Could not rename terminal")?;
        return Ok(json!({"ok":true}));
    }
    if !matches!(
        request.action.as_str(),
        "snapshot" | "input" | "resize" | "kill"
    ) {
        return Err("Unknown action");
    }
    let cancel = CancellationToken::new();
    let mut nonce = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut client = HostClient::connect(
        LocalEndpoint::new(paths.runtime_parent().join(id.to_string()), id),
        ConnectOptions::local_read_only(id, nonce),
        &cancel,
    )
    .await
    .map_err(|_| "Terminal unavailable")?;
    if client.host_instance_id() != Some(expected) {
        return Err("Terminal host changed; refresh and retry");
    }
    // Keystrokes do not need a replay or terminal emulation before reaching the PTY.
    if request.action == "input" && !request.paste {
        send_input(
            &mut client,
            request.data.ok_or("Input required")?,
            false,
            false,
            &cancel,
        )
        .await?;
        client.disconnect();
        return Ok(json!({"ok":true}));
    }
    let state = client
        .get_state(&cancel)
        .await
        .map_err(|_| "Cannot read terminal state")?;
    if request.action == "kill" {
        client
            .stop(CommandId::new(), wire::StopMode::Force, &cancel)
            .await
            .map_err(|_| "Could not kill terminal")?;
        client.disconnect();
        return Ok(json!({"ok":true}));
    }
    if request.action == "input"
        && let Some(bracketed) = state.bracketed_paste
    {
        send_input(
            &mut client,
            request.data.ok_or("Input required")?,
            true,
            bracketed,
            &cancel,
        )
        .await?;
        client.disconnect();
        return Ok(json!({"ok":true}));
    }
    // Read-only attachment never claims a writer lease or resizes the running application.
    let outputs = client
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .map_err(|_| "Cannot capture terminal")?;
    let snapshot = client.take_last_snapshot();
    let fallback = read_saved_size(&session_dir.join("browser-viewport.json")).unwrap_or((80, 24));
    let mut size = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.viewport.as_ref())
        .or(state.viewport.as_ref())
        .map(|size| {
            (
                size.rows.clamp(1, 200) as u16,
                size.columns.clamp(1, 500) as u16,
            )
        })
        .unwrap_or((fallback.1 as u16, fallback.0 as u16));
    let mut parser = vt100::Parser::new(size.0, size.1, 200);
    if let Some(snapshot) = snapshot {
        parser.process(&snapshot.terminal_bytes);
    }
    for output in outputs {
        parser.process(&output.bytes);
    }
    if let Some(viewport) = state.viewport.as_ref() {
        size = (
            viewport.rows.clamp(1, 200) as u16,
            viewport.columns.clamp(1, 500) as u16,
        );
        parser.screen_mut().set_size(size.0, size.1);
    }
    if request.action == "snapshot" {
        let screen = parser.screen();
        let cursor = screen.cursor_position();
        let mut copy = screen.clone();
        copy.set_scrollback(200);
        let history = copy.scrollback();
        let mut lines = Vec::new();
        for offset in (1..=history).rev() {
            copy.set_scrollback(offset);
            lines.push(if request.plain {
                copy.rows(0, size.1).next().unwrap_or_default().into_bytes()
            } else {
                copy.rows_formatted(0, size.1).next().unwrap_or_default()
            });
        }
        copy.set_scrollback(0);
        if request.plain {
            lines.extend(copy.rows(0, size.1).map(String::into_bytes));
        } else {
            lines.extend(copy.rows_formatted(0, size.1));
        }
        let content = lines
            .iter()
            .map(|line| String::from_utf8_lossy(line))
            .collect::<Vec<_>>()
            .join("\n");
        client.disconnect();
        return Ok(
            json!({"id":id.to_string(),"content":content,"width":size.1,"height":size.0,"cursorX":cursor.1,"cursorY":cursor.0,"cursorVisible":!screen.hide_cursor()}),
        );
    }
    if request.action == "input" {
        send_input(
            &mut client,
            request.data.ok_or("Input required")?,
            request.paste,
            parser.screen().bracketed_paste(),
            &cancel,
        )
        .await?;
    } else {
        let auto_path = session_dir.join("browser-auto-size.json");
        let (cols, rows) = if request.auto {
            let stored = read_saved_size(&auto_path);
            stored.unwrap_or((u32::from(size.1), u32::from(size.0)))
        } else {
            let cols = request.cols.ok_or("Columns required")?;
            let rows = request.rows.ok_or("Rows required")?;
            if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
                return Err("Invalid terminal size");
            }
            if !auto_path.exists() {
                let saved = serde_json::to_vec(&(u32::from(size.1), u32::from(size.0)))
                    .map_err(|_| "Cannot save size")?;
                multiplex_store::SystemAtomicWriter
                    .write(&auto_path, &saved)
                    .map_err(|_| "Cannot save size")?;
            }
            (cols, rows)
        };
        if !(1..=500).contains(&cols) || !(1..=200).contains(&rows) {
            return Err("Invalid terminal size");
        }
        if !client
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .map_err(|_| "Terminal control unavailable")?
        {
            return Err("Another client controls this terminal");
        }
        client
            .resize(CommandId::new(), cols, rows, &cancel)
            .await
            .map_err(|_| "Could not resize terminal")?;
        let saved = serde_json::to_vec(&(cols, rows)).map_err(|_| "Cannot save terminal size")?;
        multiplex_store::SystemAtomicWriter
            .write(&session_dir.join("browser-viewport.json"), &saved)
            .map_err(|_| "Cannot save terminal size")?;
        if request.auto {
            let _ = std::fs::remove_file(auto_path);
        }
    }
    let _ = client
        .set_writer_lease(CommandId::new(), false, &cancel)
        .await;
    client.disconnect();
    Ok(json!({"ok":true}))
}
