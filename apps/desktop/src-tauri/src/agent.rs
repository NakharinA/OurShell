//! Bridge to the `ourshell agent` local socket.
//!
//! The desktop app never does crypto or SSH itself. It connects to the agent's
//! local socket (a Windows named pipe or a Unix domain socket), performs the
//! `hello` handshake, and from then on forwards JSON-RPC requests from the
//! webview and relays the agent's notifications back to it as a `agent-event`
//! Tauri event. If the socket is not reachable, it spawns the agent binary
//! (as a Tauri sidecar, falling back to `ourshell` on PATH, then to a dev
//! build under `target/debug`) and retries.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::GenericFilePath;
use ourshell_proto::agent::{method, ClientKind, HelloParams};
use ourshell_proto::rpc::{Request, RpcError, ServerMessage};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Error code used locally (not defined by the agent) for bridge-level failures
/// such as "could not reach the agent at all".
const BRIDGE_ERROR: i64 = -32000;

enum Cmd {
    Call {
        method: String,
        params: Value,
        resp: oneshot::Sender<Result<Value, RpcError>>,
    },
}

/// Handle given to Tauri commands; cheap to clone, forwards calls to the actor task.
#[derive(Clone)]
pub struct AgentHandle {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}

impl AgentHandle {
    /// Spawns the background actor that owns the connection and starts it connecting.
    pub fn spawn(app: AppHandle) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(actor_loop(app, cmd_rx));
        Self { cmd_tx }
    }

    /// Sends one JSON-RPC call to the agent and awaits its response.
    pub async fn call(&self, method: String, params: Value) -> Result<Value, RpcError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Call {
                method,
                params,
                resp: tx,
            })
            .map_err(|_| RpcError::new(BRIDGE_ERROR, "agent bridge is not running"))?;
        rx.await
            .map_err(|_| RpcError::new(BRIDGE_ERROR, "agent bridge dropped the response"))?
    }
}

/// Latest connection status, so a webview that (re)loads after the last
/// `agent-status` event (e.g. a dev-server reload) can still ask for it.
static LAST_STATUS: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// The most recent status sent to the webview.
pub fn last_status() -> Value {
    LAST_STATUS
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_else(|| serde_json::json!({ "connected": false, "detail": "starting" }))
}

fn emit_status(app: &AppHandle, connected: bool, detail: impl Into<String>) {
    let status = serde_json::json!({ "connected": connected, "detail": detail.into() });
    if let Ok(mut s) = LAST_STATUS.lock() {
        *s = Some(status.clone());
    }
    let _ = app.emit("agent-status", status);
}

/// Owns the connection for its lifetime; reconnects (and re-spawns the agent if
/// needed) forever, since a user can quit/restart the agent independently of the
/// desktop app.
async fn actor_loop(app: AppHandle, mut cmd_rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut backoff = Duration::from_millis(500);
    const MAX_BACKOFF: Duration = Duration::from_secs(10);

    loop {
        emit_status(&app, false, "connecting");
        match connect_with_spawn(&app).await {
            Ok(stream) => {
                backoff = Duration::from_millis(500);
                let mut reader = BufReader::new(&stream);
                match handshake(&stream, &mut reader).await {
                    Ok(()) => {
                        emit_status(&app, true, "connected");
                        if let Err(e) =
                            run_connection(&app, &stream, &mut reader, &mut cmd_rx).await
                        {
                            log::warn!("agent connection closed: {e}");
                        }
                    }
                    Err(e) => {
                        log::warn!("agent handshake failed: {e}");
                    }
                }
                emit_status(&app, false, "disconnected");
            }
            Err(e) => {
                emit_status(&app, false, format!("could not reach agent: {e}"));
            }
        }

        // If the app is shutting down, cmd_rx will be closed and we should stop
        // reconnecting forever.
        if cmd_rx.is_closed() {
            return;
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// The agent's local socket path, per `docs/agent-protocol.md`. Must match
/// `crates/cli/src/ipc.rs`, including the `OURSHELL_AGENT_SOCKET` override.
fn socket_path() -> String {
    if let Ok(s) = std::env::var("OURSHELL_AGENT_SOCKET") {
        if cfg!(windows) && !s.starts_with(r"\\") {
            return format!(r"\\.\pipe\{s}");
        }
        return s;
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "user".to_string());
        format!(r"\\.\pipe\ourshell-agent-{user}")
    }
    #[cfg(not(windows))]
    {
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            if !dir.is_empty() {
                return format!("{dir}/ourshell/agent.sock");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.ourshell/agent.sock")
    }
}

async fn try_connect() -> io::Result<LocalSocketStream> {
    let path = socket_path();
    let name = path.as_str().to_fs_name::<GenericFilePath>()?;
    LocalSocketStream::connect(name).await
}

/// Dev-only fallback: `../../../target/debug/ourshell(.exe)` relative to this crate
/// (`apps/desktop/src-tauri`), i.e. the root Cargo workspace's debug build output.
fn dev_build_path() -> PathBuf {
    let exe = if cfg!(windows) {
        "ourshell.exe"
    } else {
        "ourshell"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/debug")
        .join(exe)
}

/// Tries, in order: the bundled sidecar, `ourshell` on PATH, then the dev build
/// under the workspace `target/debug` directory.
fn spawn_agent(app: &AppHandle) {
    let shell = app.shell();

    if let Ok(cmd) = shell.sidecar("ourshell") {
        match cmd.args(["agent"]).spawn() {
            Ok(_) => {
                log::info!("spawned agent sidecar");
                return;
            }
            Err(e) => log::warn!("sidecar spawn failed: {e}"),
        }
    }

    match shell.command("ourshell").args(["agent"]).spawn() {
        Ok(_) => {
            log::info!("spawned agent from PATH");
            return;
        }
        Err(e) => log::warn!("spawning `ourshell` from PATH failed: {e}"),
    }

    let dev_path = dev_build_path();
    if dev_path.exists() {
        match shell
            .command(dev_path.to_string_lossy().to_string())
            .args(["agent"])
            .spawn()
        {
            Ok(_) => log::info!("spawned agent from workspace dev build"),
            Err(e) => log::warn!("spawning dev build agent failed: {e}"),
        }
    } else {
        log::warn!(
            "no agent binary found (sidecar, PATH, or {})",
            dev_path.display()
        );
    }
}

/// Connects to the agent socket, spawning it (trying all fallbacks) and retrying
/// for a few seconds if it isn't up yet.
async fn connect_with_spawn(app: &AppHandle) -> io::Result<LocalSocketStream> {
    if let Ok(s) = try_connect().await {
        return Ok(s);
    }

    spawn_agent(app);

    let mut last_err = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        match try_connect().await {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("could not connect to agent")))
}

async fn write_request(stream: &LocalSocketStream, req: &Request) -> io::Result<()> {
    let mut line = serde_json::to_string(req).map_err(io::Error::other)?;
    line.push('\n');
    let mut sender = stream;
    sender.write_all(line.as_bytes()).await?;
    sender.flush().await
}

/// First request on every connection, per `docs/agent-protocol.md`. Reads through the
/// same buffered reader `run_connection` will keep using, so no bytes the agent sent
/// right after the hello reply (pipelined) are lost with the buffer.
async fn handshake(
    stream: &LocalSocketStream,
    reader: &mut BufReader<&LocalSocketStream>,
) -> io::Result<()> {
    let params = HelloParams {
        protocol: ourshell_proto::AGENT_PROTOCOL_VERSION,
        client: ClientKind::Desktop,
        ai_session: None,
        ai_client_name: Some("OurShell Desktop".to_string()),
    };
    let params = serde_json::to_value(params).map_err(io::Error::other)?;
    write_request(stream, &Request::new(0, method::HELLO, params)).await?;
    expect_ok(reader, 0, "hello").await?;

    // The agent only broadcasts events (approval requests, vault lock, AI
    // mode changes) to connections that subscribe; per-session PTY output
    // arrives either way.
    write_request(
        stream,
        &Request::new(0, method::EVENTS_SUBSCRIBE, serde_json::json!({})),
    )
    .await?;
    expect_ok(reader, 0, "events.subscribe").await
}

/// Reads one handshake reply and checks it is a success for `id`.
async fn expect_ok(
    reader: &mut BufReader<&LocalSocketStream>,
    id: u64,
    what: &str,
) -> io::Result<()> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("agent closed the connection during {what}"),
        ));
    }
    let msg: ServerMessage = serde_json::from_str(line.trim_end()).map_err(io::Error::other)?;
    match msg {
        ServerMessage::Response(resp) if resp.id == id => {
            if let Some(err) = resp.error {
                return Err(io::Error::other(format!("{what}: {err}")));
            }
            Ok(())
        }
        _ => Err(io::Error::other(format!("unexpected reply to {what}"))),
    }
}

/// Runs the request/response + notification loop for one live connection.
/// Returns once the connection is lost (or the command channel is closed).
async fn run_connection(
    app: &AppHandle,
    stream: &LocalSocketStream,
    reader: &mut BufReader<&LocalSocketStream>,
    cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
) -> io::Result<()> {
    let next_id = AtomicU64::new(1);
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, RpcError>>> = HashMap::new();
    let mut line_buf = String::new();

    let result = loop {
        line_buf.clear();
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Cmd::Call { method, params, resp }) => {
                        let id = next_id.fetch_add(1, Ordering::SeqCst);
                        let req = Request::new(id, method, params);
                        if let Err(e) = write_request(stream, &req).await {
                            let _ = resp.send(Err(RpcError::new(BRIDGE_ERROR, format!("write failed: {e}"))));
                            break Err(e);
                        }
                        pending.insert(id, resp);
                    }
                    None => break Ok(()),
                }
            }
            n = reader.read_line(&mut line_buf) => {
                match n {
                    Ok(0) => {
                        break Err(io::Error::new(io::ErrorKind::UnexpectedEof, "agent closed the connection"));
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim_end();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<ServerMessage>(trimmed) {
                            Ok(ServerMessage::Response(resp)) => {
                                if let Some(tx) = pending.remove(&resp.id) {
                                    let result = match resp.error {
                                        Some(e) => Err(e),
                                        None => Ok(resp.result.unwrap_or(Value::Null)),
                                    };
                                    let _ = tx.send(result);
                                }
                            }
                            Ok(ServerMessage::Notification(note)) => {
                                let _ = app.emit(
                                    "agent-event",
                                    serde_json::json!({ "method": note.method, "params": note.params }),
                                );
                            }
                            Err(e) => {
                                log::warn!("ignoring malformed agent message: {e}");
                            }
                        }
                    }
                    Err(e) => break Err(e),
                }
            }
        }
    };

    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(RpcError::new(BRIDGE_ERROR, "disconnected from agent")));
    }

    result
}
