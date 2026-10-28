//! The local agent daemon: holds the unlocked vault and serves JSON-RPC over a local socket.
//!
//! All front-ends (CLI, desktop, MCP) go through this process, so keys are unlocked
//! in one place and every AI call passes the policy check.

mod ai;
mod items;
pub mod policy;
pub mod ssh_ops;
mod sync_ops;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use interprocess::local_socket::tokio::{prelude::*, Stream};
use interprocess::local_socket::ListenerOptions;
use ourshell_core::vault::{Vault, VaultError};
use ourshell_proto::agent::{
    event, method, ClientKind, HelloParams, HelloResult, UnlockParams, VaultInitParams,
    VaultInitResult, VaultStatus,
};
use ourshell_proto::rpc::{self, Notification, Request, Response, RpcError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::ipc;

pub const DEFAULT_AUTO_LOCK_SECS: u64 = 15 * 60;
/// Exit after this long with a locked vault and no connected clients.
pub const DEFAULT_IDLE_EXIT_SECS: u64 = 10 * 60;

#[derive(Clone, Debug)]
pub struct AgentOptions {
    pub vault_path: PathBuf,
    pub idle_exit: Option<Duration>,
}

pub struct VaultSlot {
    pub vault: Option<Vault>,
    pub auto_lock: Duration,
    pub last_activity: Instant,
}

/// Shared agent state.
pub struct State {
    pub opts: AgentOptions,
    pub vault: Mutex<VaultSlot>,
    pub events: broadcast::Sender<Notification>,
    pub clients: std::sync::atomic::AtomicUsize,
    pub last_disconnect: Mutex<Instant>,
    pub policy: std::sync::Mutex<policy::Policy>,
    pub ssh: ssh_ops::SshState,
    pub shutdown: tokio::sync::Notify,
}

/// Per-connection context.
pub struct Conn {
    pub kind: ClientKind,
    pub ai_session: Option<String>,
    pub ai_client_name: Option<String>,
    /// Events targeted at this connection only (e.g. its PTY output).
    pub tx: mpsc::UnboundedSender<Notification>,
}

pub type RpcResult = std::result::Result<Value, RpcError>;

pub fn err(code: i64, msg: impl Into<String>) -> RpcError {
    RpcError::new(code, msg)
}

pub fn parse<T: DeserializeOwned>(v: Value) -> std::result::Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| err(rpc::INVALID_PARAMS, e.to_string()))
}

pub fn to_value<T: Serialize>(v: T) -> RpcResult {
    serde_json::to_value(v).map_err(|e| err(rpc::INTERNAL_ERROR, e.to_string()))
}

pub fn vault_err(e: VaultError) -> RpcError {
    match e {
        VaultError::Locked => err(rpc::VAULT_LOCKED, "vault is locked"),
        VaultError::NotFound(id) => err(rpc::NOT_FOUND, format!("not found: {id}")),
        VaultError::WrongKind { .. } => err(rpc::NOT_FOUND, e.to_string()),
        VaultError::WrongPassword => err(rpc::INVALID_PARAMS, "wrong master password"),
        VaultError::Exists(what) => err(
            rpc::INVALID_PARAMS,
            format!("{} already exists", what.display()),
        ),
        other => err(rpc::INTERNAL_ERROR, other.to_string()),
    }
}

impl State {
    pub fn new(opts: AgentOptions) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            opts,
            vault: Mutex::new(VaultSlot {
                vault: None,
                auto_lock: Duration::from_secs(DEFAULT_AUTO_LOCK_SECS),
                last_activity: Instant::now(),
            }),
            events,
            clients: Default::default(),
            last_disconnect: Mutex::new(Instant::now()),
            policy: Default::default(),
            ssh: Default::default(),
            shutdown: Default::default(),
        })
    }

    pub fn emit(&self, method: &str, params: Value) {
        let _ = self.events.send(Notification::new(method, params));
    }

    /// Counts as user activity for the auto-lock timer.
    pub async fn touch(&self) {
        self.vault.lock().await.last_activity = Instant::now();
    }

    /// Runs `f` with the unlocked vault, refreshing the auto-lock timer.
    pub async fn with_vault<T>(
        &self,
        f: impl FnOnce(&mut Vault) -> std::result::Result<T, VaultError>,
    ) -> std::result::Result<T, RpcError> {
        let mut slot = self.vault.lock().await;
        slot.last_activity = Instant::now();
        match slot.vault.as_mut() {
            Some(v) if v.is_unlocked() => f(v).map_err(vault_err),
            _ => Err(err(rpc::VAULT_LOCKED, "vault is locked")),
        }
    }

    async fn status(&self) -> VaultStatus {
        let slot = self.vault.lock().await;
        let exists = self.opts.vault_path.exists();
        let unlocked = slot.vault.as_ref().is_some_and(|v| v.is_unlocked());
        VaultStatus {
            exists,
            unlocked,
            path: self.opts.vault_path.display().to_string(),
            local_only: slot
                .vault
                .as_ref()
                .map(|v| v.header().local_only)
                .unwrap_or(true),
            auto_lock_in: unlocked.then(|| {
                slot.auto_lock
                    .saturating_sub(slot.last_activity.elapsed())
                    .as_secs()
            }),
        }
    }

    pub async fn lock(&self) {
        let mut slot = self.vault.lock().await;
        if let Some(v) = slot.vault.as_mut() {
            if v.is_unlocked() {
                v.lock();
                drop(slot);
                self.emit(event::VAULT_LOCKED, json!({}));
            }
        }
    }
}

/// Runs the agent until shutdown.
pub async fn run(opts: AgentOptions) -> Result<()> {
    let id = ipc::socket_id();
    let name = ipc::socket_name(&id)?;
    #[cfg(unix)]
    if std::path::Path::new(&id).exists() {
        // A stale socket file from a crashed agent; a live one would accept connections.
        if Stream::connect(ipc::socket_name(&id)?).await.is_ok() {
            anyhow::bail!("an agent is already running at {id}");
        }
        let _ = std::fs::remove_file(&id);
    }
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(|e| {
            anyhow::anyhow!("cannot listen on {id}: {e} (is an agent already running?)")
        })?;
    tracing::info!("agent listening on {id}");

    let state = State::new(opts);
    spawn_timers(state.clone());

    loop {
        tokio::select! {
            conn = listener.accept() => {
                match conn {
                    Ok(c) => {
                        let st = state.clone();
                        tokio::spawn(async move {
                            st.clients.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            if let Err(e) = handle_conn(st.clone(), c).await {
                                tracing::debug!("connection ended: {e}");
                            }
                            st.clients.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            *st.last_disconnect.lock().await = Instant::now();
                        });
                    }
                    Err(e) => tracing::warn!("accept failed: {e}"),
                }
            }
            _ = state.shutdown.notified() => break,
        }
    }
    state.lock().await;
    #[cfg(unix)]
    let _ = std::fs::remove_file(&id);
    Ok(())
}

/// `$OURSHELL_AUTOSYNC_SECS`, parsed once at startup: how often the agent
/// runs a background `sync.run` while unlocked and signed in. `0` disables
/// it (the default is still to sync -- `ourshell sync` or `account login`
/// users who want no background network traffic should set this to `0`).
fn autosync_interval() -> Option<Duration> {
    let secs: u64 = std::env::var("OURSHELL_AUTOSYNC_SECS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(300);
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn spawn_timers(state: Arc<State>) {
    let autosync = autosync_interval();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut last_autosync = Instant::now();
        loop {
            tick.tick().await;
            let expired = {
                let slot = state.vault.lock().await;
                slot.vault.as_ref().is_some_and(|v| v.is_unlocked())
                    && slot.last_activity.elapsed() >= slot.auto_lock
            };
            if expired {
                tracing::info!("auto-locking vault");
                state.lock().await;
            }
            if let Some(every) = autosync {
                if last_autosync.elapsed() >= every {
                    last_autosync = Instant::now();
                    sync_ops::autosync_tick(&state).await;
                }
            }
            if let Some(idle) = state.opts.idle_exit {
                let unlocked = state
                    .vault
                    .lock()
                    .await
                    .vault
                    .as_ref()
                    .is_some_and(|v| v.is_unlocked());
                let no_clients = state.clients.load(std::sync::atomic::Ordering::SeqCst) == 0;
                if !unlocked && no_clients && state.last_disconnect.lock().await.elapsed() >= idle {
                    tracing::info!("idle with locked vault; exiting");
                    state.shutdown.notify_one();
                    return;
                }
            }
        }
    });
}

async fn handle_conn(state: Arc<State>, stream: Stream) -> Result<()> {
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r).lines();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        while let Some(buf) = out_rx.recv().await {
            if w.write_all(&buf).await.is_err() {
                break;
            }
            let _ = w.flush().await;
        }
    });
    let send = {
        let out_tx = out_tx.clone();
        move |v: &dyn erased::Ser| {
            if let Ok(mut b) = v.to_json() {
                b.push(b'\n');
                let _ = out_tx.send(b);
            }
        }
    };

    // Handshake.
    let first = match lines.next_line().await? {
        Some(l) => l,
        None => return Ok(()),
    };
    let req: Request = match serde_json::from_str(&first) {
        Ok(r) => r,
        Err(e) => {
            send(&Response::err(0, err(rpc::PARSE_ERROR, e.to_string())));
            return Ok(());
        }
    };
    if req.method != method::HELLO {
        send(&Response::err(
            req.id,
            err(rpc::HANDSHAKE_REQUIRED, "first call must be `hello`"),
        ));
        return Ok(());
    }
    let hello: HelloParams = match parse(req.params) {
        Ok(h) => h,
        Err(e) => {
            send(&Response::err(req.id, e));
            return Ok(());
        }
    };
    if hello.protocol != ourshell_proto::AGENT_PROTOCOL_VERSION {
        send(&Response::err(
            req.id,
            err(
                rpc::INVALID_REQUEST,
                format!("unsupported protocol v{}", hello.protocol),
            ),
        ));
        return Ok(());
    }
    if hello.client == ClientKind::Mcp && hello.ai_session.is_none() {
        send(&Response::err(
            req.id,
            err(rpc::INVALID_PARAMS, "MCP clients must pass ai_session"),
        ));
        return Ok(());
    }
    send(&Response::ok(
        req.id,
        serde_json::to_value(HelloResult {
            protocol: ourshell_proto::AGENT_PROTOCOL_VERSION,
            agent_version: env!("CARGO_PKG_VERSION").into(),
        })?,
    ));

    if let Some(s) = &hello.ai_session {
        state
            .policy
            .lock()
            .unwrap()
            .register_session(s, hello.ai_client_name.clone());
        state.emit(event::AI_MODE, json!({"ai_session": s, "connected": true}));
    }
    let (ntx, mut nrx) = mpsc::unbounded_channel::<Notification>();
    let conn = Arc::new(Conn {
        kind: hello.client,
        ai_session: hello.ai_session,
        ai_client_name: hello.ai_client_name,
        tx: ntx,
    });

    // Forward targeted notifications.
    {
        let send = send.clone();
        tokio::spawn(async move {
            while let Some(n) = nrx.recv().await {
                send(&n);
            }
        });
    }

    let subscribed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let send = send.clone();
        let mut events = state.events.subscribe();
        let subscribed = subscribed.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(n) => {
                        if subscribed.load(std::sync::atomic::Ordering::Relaxed) {
                            send(&n);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                send(&Response::err(0, err(rpc::PARSE_ERROR, e.to_string())));
                continue;
            }
        };
        if req.method == method::EVENTS_SUBSCRIBE {
            subscribed.store(true, std::sync::atomic::Ordering::Relaxed);
            send(&Response::ok(req.id, json!({"subscribed": true})));
            continue;
        }
        let st = state.clone();
        let c = conn.clone();
        let send = send.clone();
        // Each request runs concurrently so a pending approval doesn't block the connection.
        tokio::spawn(async move {
            let id = req.id;
            let resp = match dispatch(&st, &c, &req.method, req.params).await {
                Ok(v) => Response::ok(id, v),
                Err(e) => Response::err(id, e),
            };
            send(&resp);
        });
    }
    drop(send);
    drop(out_tx);
    writer.abort();
    // Per-connection resources (PTYs, etc.) are released by their owners.
    connection_closed(&state, &conn).await;
    Ok(())
}

/// Hook for cleaning up per-connection resources.
async fn connection_closed(state: &Arc<State>, conn: &Arc<Conn>) {
    if let Some(s) = &conn.ai_session {
        // The AI session ends with its connection; so does any dangerous mode.
        state.policy.lock().unwrap().end_session(s);
        state.emit(event::AI_MODE, json!({"ai_session": s, "connected": false}));
    }
    ssh_ops::connection_closed(state, conn).await;
}

/// Routes one request.
pub async fn dispatch(
    state: &Arc<State>,
    conn: &Arc<Conn>,
    method: &str,
    params: Value,
) -> RpcResult {
    // AI clients may only call the AI tool methods.
    if conn.kind == ClientKind::Mcp && !ai::AI_CALLABLE.contains(&method) {
        return Err(err(
            rpc::POLICY_DENIED,
            "MCP clients may only call AI tool methods",
        ));
    }
    match method {
        method::VAULT_STATUS => to_value(state.status().await),
        method::VAULT_INIT => vault_init(state, parse(params)?).await,
        method::VAULT_UNLOCK => vault_unlock(state, parse(params)?).await,
        method::VAULT_LOCK => {
            state.lock().await;
            Ok(json!({}))
        }
        method::SHUTDOWN => {
            state.shutdown.notify_one();
            Ok(json!({}))
        }
        m if m.starts_with("ai.") => ai::dispatch(state, conn, m, params).await,
        m if m.starts_with("pty.")
            || m.starts_with("sftp.")
            || m.starts_with("forward.")
            || m == method::EXEC_RUN
            || m == method::HOSTKEY_TRUST =>
        {
            ssh_ops::dispatch(state, conn, m, params).await
        }
        m if m.starts_with("item.") || m.starts_with("key.") || m.starts_with("import.") => {
            items::dispatch(state, m, params).await
        }
        m if m.starts_with("account.") || m.starts_with("sync.") || m == method::VAULT_RECOVER => {
            sync_ops::dispatch(state, m, params).await
        }
        _ => Err(err(
            rpc::METHOD_NOT_FOUND,
            format!("unknown method `{method}`"),
        )),
    }
}

async fn vault_init(state: &Arc<State>, p: VaultInitParams) -> RpcResult {
    if p.password.len() < 8 {
        return Err(err(
            rpc::INVALID_PARAMS,
            "master password must be at least 8 characters",
        ));
    }
    let path = state.opts.vault_path.clone();
    let created =
        tokio::task::spawn_blocking(move || Vault::create(&path, &p.password, p.local_only))
            .await
            .map_err(|e| err(rpc::INTERNAL_ERROR, e.to_string()))?
            .map_err(|e| match e {
                VaultError::Exists(_) => err(rpc::INVALID_PARAMS, e.to_string()),
                other => vault_err(other),
            })?;
    let phrase = created.recovery_phrase.to_string();
    let mut slot = state.vault.lock().await;
    slot.vault = Some(created.vault);
    slot.last_activity = Instant::now();
    drop(slot);
    state.emit(event::VAULT_UNLOCKED, json!({}));
    to_value(VaultInitResult {
        recovery_phrase: phrase,
    })
}

async fn vault_unlock(state: &Arc<State>, p: UnlockParams) -> RpcResult {
    let path = state.opts.vault_path.clone();
    let mut slot = state.vault.lock().await;
    let mut vault = match slot.vault.take() {
        Some(v) => v,
        None => Vault::open(&path).map_err(|e| match e {
            VaultError::Missing(_) => {
                err(rpc::NOT_FOUND, "no vault yet; run `ourshell vault init`")
            }
            other => vault_err(other),
        })?,
    };
    let pw = p.password;
    // Argon2 is CPU heavy; keep it off the async workers.
    let res = tokio::task::block_in_place(|| vault.unlock(&pw));
    slot.vault = Some(vault);
    res.map_err(vault_err)?;
    slot.auto_lock = Duration::from_secs(p.auto_lock_secs.unwrap_or(DEFAULT_AUTO_LOCK_SECS));
    slot.last_activity = Instant::now();
    drop(slot);
    state.emit(event::VAULT_UNLOCKED, json!({}));
    Ok(json!({}))
}

/// Object-safe "serialize to JSON" so one closure can send responses and notifications.
mod erased {
    pub trait Ser {
        fn to_json(&self) -> serde_json::Result<Vec<u8>>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_json(&self) -> serde_json::Result<Vec<u8>> {
            serde_json::to_vec(self)
        }
    }
}
