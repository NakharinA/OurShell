//! SSH operations: resolving vault hosts into connection targets and serving
//! `pty.*`, `exec.run`, `sftp.*` and `forward.*`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ourshell_core::ssh::{
    self, Auth, ChainVerifier, ConnectOptions, ExecLimits, ForwardHandle, HostKeyVerifier,
    KnownHostsFile, MemoryKnownHosts, PtyEvent, Session, SshError, Target,
};
use ourshell_core::telnet;
use ourshell_core::vault::{Host, Identity, KnownHost, SshKey};
use ourshell_proto::agent::{
    event, method, DirEntry, ExecParams, ExecResult, FileData, ForwardInfo, ForwardKind,
    ForwardOpenParams, HostTarget, PtyDataEvent, PtyExitEvent, PtyOpenParams, PtyOpenResult,
    PtyResizeParams, PtyWriteParams, SessionRef, SftpPathParams, SftpWriteParams,
};
use ourshell_proto::rpc::{self, Notification, RpcError};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{err, parse, to_value, Conn, RpcResult, State};

const MAX_JUMPS: usize = 8;
pub const DEFAULT_MAX_OUTPUT: usize = 1 << 20;

struct PtyEntry {
    writer: PtyWriter,
    owner: usize,
    /// Keeps the SSH connection alive (telnet sessions own their socket).
    _session: Option<Session>,
}

/// The read side of an interactive session over either protocol. Its reader
/// task stops when this is dropped.
enum LivePty {
    Ssh(ssh::PtySession),
    Telnet(telnet::TelnetSession),
}

impl LivePty {
    async fn recv(&mut self) -> Option<PtyEvent> {
        match self {
            Self::Ssh(p) => p.events.recv().await,
            Self::Telnet(t) => t.events.recv().await,
        }
    }
}

/// The write side of an interactive session over either protocol.
#[derive(Clone)]
enum PtyWriter {
    Ssh(ssh::pty::PtyWriter),
    Telnet(telnet::TelnetWriter),
}

impl PtyWriter {
    async fn write(&self, data: &[u8]) -> Result<(), RpcError> {
        match self {
            Self::Ssh(w) => w.write(data).await.map_err(ssh_err),
            Self::Telnet(w) => w.write(data).await.map_err(telnet_err),
        }
    }

    async fn resize(&self, cols: u32, rows: u32) -> Result<(), RpcError> {
        match self {
            Self::Ssh(w) => w.resize(cols, rows).await.map_err(ssh_err),
            Self::Telnet(w) => w.resize(cols, rows).await.map_err(telnet_err),
        }
    }

    async fn close(&self) {
        match self {
            Self::Ssh(w) => {
                let _ = w.close().await;
            }
            Self::Telnet(w) => {
                let _ = w.close().await;
            }
        }
    }
}

struct ForwardEntry {
    info: ForwardInfo,
    handle: ForwardHandle,
    _session: Session,
}

/// SSH state kept by the agent: live PTYs and port forwards.
#[derive(Default)]
pub struct SshState {
    ptys: Mutex<HashMap<String, PtyEntry>>,
    forwards: Mutex<HashMap<String, ForwardEntry>>,
}

pub struct ExecOut {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    pub truncated: bool,
}

fn conn_key(conn: &Arc<Conn>) -> usize {
    Arc::as_ptr(conn) as usize
}

/// `SHA256:<base64>` fingerprint of an OpenSSH public key line.
pub fn fingerprint(key_openssh: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let blob = key_openssh
        .split_whitespace()
        .nth(1)
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .unwrap_or_default();
    let digest = sha2::Sha256::digest(&blob);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

fn ssh_err(e: SshError) -> RpcError {
    match e {
        SshError::HostKeyMismatch { host_port } => RpcError {
            code: rpc::HOST_KEY_MISMATCH,
            message: format!(
                "HOST KEY MISMATCH for {host_port}: the server presented a different key than the one you trusted. \
                 This could be a man-in-the-middle attack. Refusing to connect."
            ),
            data: Some(json!({ "host_port": host_port })),
        },
        other => err(rpc::INTERNAL_ERROR, other.to_string()),
    }
}

fn telnet_err(e: telnet::TelnetError) -> RpcError {
    err(rpc::INTERNAL_ERROR, e.to_string())
}

fn telnet_unsupported(name: &str) -> RpcError {
    err(
        rpc::INVALID_PARAMS,
        format!("`{name}` is a telnet host: only interactive terminal sessions are supported"),
    )
}

/// Builds the hop list (jump hosts first) for a vault host.
async fn targets(
    state: &Arc<State>,
    host_ref: &str,
    for_ai: bool,
) -> Result<(String, Host, Vec<Target>), RpcError> {
    state
        .with_vault(|v| {
            let (id, host) = v.find::<Host>(host_ref)?;
            let mut chain = vec![(host.clone(), true)];
            let mut seen = vec![id.clone()];
            let mut cur = host.clone();
            while let Some(j) = cur.jump_host.clone().filter(|j| !j.is_empty()) {
                let (jid, jh) = v.find::<Host>(&j)?;
                if seen.contains(&jid) || seen.len() > MAX_JUMPS {
                    return Err(ourshell_core::vault::VaultError::NotFound(format!(
                        "jump host loop at `{j}`"
                    )));
                }
                seen.push(jid);
                chain.push((jh.clone(), false));
                cur = jh;
            }
            chain.reverse();
            let mut out = Vec::new();
            for (h, is_final) in chain {
                out.push(target_for(v, &h, for_ai && is_final)?);
            }
            Ok((id, host, out))
        })
        .await
}

fn target_for(
    v: &mut ourshell_core::vault::Vault,
    h: &Host,
    ai_override: bool,
) -> Result<Target, ourshell_core::vault::VaultError> {
    let ident = match h.identity_id.as_deref() {
        Some(id) => Some(v.get::<Identity>(id)?),
        None => None,
    };
    let mut user = h
        .username
        .clone()
        .or_else(|| ident.as_ref().and_then(|i| i.username.clone()))
        .unwrap_or_else(|| "root".into());
    if ai_override {
        if let Some(u) = h.ai_user.clone().filter(|u| !u.is_empty()) {
            user = u;
        }
    }
    let auth = match &ident {
        Some(Identity {
            key_id: Some(k), ..
        }) => {
            let key = v.get::<SshKey>(k)?;
            Auth::Key {
                private_key_pem: key.private_key.clone(),
                passphrase: key.passphrase.clone(),
            }
        }
        Some(Identity {
            password: Some(p), ..
        }) => Auth::Password(p.clone()),
        _ => Auth::Agent,
    };
    Ok(Target {
        host: h.hostname.clone(),
        port: h.port_or_default(),
        user,
        auth,
    })
}

/// Records the first unknown key so the caller can ask the user.
struct UnknownCatcher {
    accept: Option<String>,
    seen: std::sync::Mutex<Option<(String, String)>>,
}

impl ssh::OnUnknownHostKey for UnknownCatcher {
    fn decide<'a>(
        &'a self,
        host_port: &'a str,
        key: &'a str,
    ) -> ssh::known_hosts::UnknownHostFuture<'a> {
        // The user may confirm either the full key or its SHA256 fingerprint.
        let ok = self
            .accept
            .as_deref()
            .is_some_and(|a| a.trim() == key.trim() || a.trim() == fingerprint(key));
        if !ok {
            *self.seen.lock().unwrap() = Some((host_port.to_string(), key.to_string()));
        }
        Box::pin(async move { ok })
    }
}

/// Connects to a vault host (resolving jump hosts and trust) and returns the session.
pub async fn connect_host(
    state: &Arc<State>,
    target: &HostTarget,
    for_ai: bool,
    agent_forwarding: bool,
) -> Result<(String, Session), RpcError> {
    let (id, host, hops) = targets(state, &target.host, for_ai).await?;
    if host.is_telnet() {
        return Err(telnet_unsupported(&host.name));
    }
    let known: Vec<(String, String)> = state
        .with_vault(|v| v.list::<KnownHost>())
        .await?
        .into_iter()
        .map(|(_, k)| (k.host, k.key))
        .collect();
    let mut verifiers: Vec<Box<dyn HostKeyVerifier>> =
        vec![Box::new(MemoryKnownHosts::from_entries(known))];
    if let Some(f) = KnownHostsFile::user_default() {
        verifiers.push(Box::new(f));
    }
    let catcher = Arc::new(UnknownCatcher {
        // AI calls never trust new hosts; a human must connect once first.
        accept: if for_ai {
            None
        } else {
            target.accept_host_key.clone()
        },
        seen: Default::default(),
    });
    let res = ssh::connect(
        &hops,
        Arc::new(ChainVerifier::new(verifiers)),
        catcher.clone(),
        ConnectOptions {
            agent_forwarding,
            ..Default::default()
        },
    )
    .await;
    match res {
        Ok((session, accepted)) => {
            for (host, key) in accepted {
                let _ = state
                    .with_vault(|v| v.put(None, &KnownHost { host, key }).map(|_| ()))
                    .await;
            }
            Ok((id, session))
        }
        Err(SshError::HostKeyRejected) => {
            let seen = catcher.seen.lock().unwrap().clone();
            match seen {
                Some((host_port, key)) => Err(RpcError {
                    code: rpc::HOST_KEY_UNKNOWN,
                    message: format!(
                        "unknown host key for {host_port} ({}); confirm it and retry with accept_host_key",
                        fingerprint(&key)
                    ),
                    data: Some(json!({
                        "host_port": host_port,
                        "key": key,
                        "fingerprint": fingerprint(&key),
                    })),
                }),
                None => Err(ssh_err(SshError::HostKeyRejected)),
            }
        }
        Err(e) => Err(ssh_err(e)),
    }
}

fn target(host_id: &str) -> HostTarget {
    HostTarget {
        host: host_id.to_string(),
        accept_host_key: None,
    }
}

pub async fn exec(
    state: &Arc<State>,
    host_id: &str,
    for_ai: bool,
    command: &str,
    timeout_secs: Option<u64>,
    max_output: usize,
) -> Result<ExecOut, RpcError> {
    exec_target(
        state,
        &target(host_id),
        for_ai,
        command,
        timeout_secs,
        max_output,
    )
    .await
}

async fn exec_target(
    state: &Arc<State>,
    t: &HostTarget,
    for_ai: bool,
    command: &str,
    timeout_secs: Option<u64>,
    max_output: usize,
) -> Result<ExecOut, RpcError> {
    let (_, session) = connect_host(state, t, for_ai, false).await?;
    let out = session
        .exec(
            command,
            ExecLimits {
                timeout: Some(Duration::from_secs(
                    timeout_secs.unwrap_or(60).clamp(1, 3600),
                )),
                max_output_bytes: max_output,
            },
        )
        .await
        .map_err(ssh_err);
    let _ = session.disconnect().await;
    let out = out?;
    Ok(ExecOut {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        exit_code: out.exit_code,
        truncated: out.truncated,
    })
}

fn dir_entry(e: ssh::DirEntry) -> DirEntry {
    DirEntry {
        name: e.name,
        is_dir: e.is_dir,
        size: Some(e.size),
        modified: e
            .modified
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
    }
}

async fn with_sftp<T>(
    state: &Arc<State>,
    t: &HostTarget,
    for_ai: bool,
    f: impl for<'a> FnOnce(
        &'a ssh::Sftp,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ssh::Result<T>> + Send + 'a>,
    >,
) -> Result<T, RpcError> {
    let (_, session) = connect_host(state, t, for_ai, false).await?;
    let sftp = session.sftp().await.map_err(ssh_err)?;
    let res = f(&sftp).await.map_err(ssh_err);
    drop(sftp);
    let _ = session.disconnect().await;
    res
}

pub async fn sftp_list(
    state: &Arc<State>,
    host_id: &str,
    for_ai: bool,
    path: &str,
) -> Result<Vec<DirEntry>, RpcError> {
    sftp_list_target(state, &target(host_id), for_ai, path).await
}

async fn sftp_list_target(
    state: &Arc<State>,
    t: &HostTarget,
    for_ai: bool,
    path: &str,
) -> Result<Vec<DirEntry>, RpcError> {
    let path = path.to_string();
    let mut v: Vec<DirEntry> = with_sftp(state, t, for_ai, move |s| {
        Box::pin(async move { s.list_dir(&path).await })
    })
    .await?
    .into_iter()
    .map(dir_entry)
    .collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(v)
}

pub async fn sftp_read(
    state: &Arc<State>,
    host_id: &str,
    for_ai: bool,
    path: &str,
    max: usize,
) -> Result<FileData, RpcError> {
    sftp_read_target(state, &target(host_id), for_ai, path, max).await
}

async fn sftp_read_target(
    state: &Arc<State>,
    t: &HostTarget,
    for_ai: bool,
    path: &str,
    max: usize,
) -> Result<FileData, RpcError> {
    let path = path.to_string();
    // Read one byte past the cap to detect truncation.
    let mut data = with_sftp(state, t, for_ai, move |s| {
        Box::pin(async move { s.read_file(&path, max.saturating_add(1)).await })
    })
    .await?;
    let truncated = data.len() > max;
    data.truncate(max);
    Ok(FileData { data, truncated })
}

pub async fn sftp_write(
    state: &Arc<State>,
    host_id: &str,
    for_ai: bool,
    path: &str,
    data: &[u8],
) -> Result<(), RpcError> {
    sftp_write_target(state, &target(host_id), for_ai, path, data).await
}

async fn sftp_write_target(
    state: &Arc<State>,
    t: &HostTarget,
    for_ai: bool,
    path: &str,
    data: &[u8],
) -> Result<(), RpcError> {
    let path = path.to_string();
    let data = data.to_vec();
    with_sftp(state, t, for_ai, move |s| {
        Box::pin(async move { s.write_file(&path, &data).await })
    })
    .await
}

/// Human-facing SSH methods (`pty.*`, `exec.run`, `sftp.*`, `forward.*`).
pub async fn dispatch(state: &Arc<State>, conn: &Arc<Conn>, m: &str, params: Value) -> RpcResult {
    match m {
        method::PTY_OPEN => pty_open(state, conn, parse(params)?).await,
        method::PTY_WRITE => {
            let p: PtyWriteParams = parse(params)?;
            let w = pty_writer(state, &p.session_id).await?;
            w.write(&p.data).await?;
            state.touch().await;
            Ok(json!({}))
        }
        method::PTY_RESIZE => {
            let p: PtyResizeParams = parse(params)?;
            let w = pty_writer(state, &p.session_id).await?;
            w.resize(p.cols, p.rows).await?;
            Ok(json!({}))
        }
        method::PTY_CLOSE => {
            let p: SessionRef = parse(params)?;
            if let Some(e) = state.ssh.ptys.lock().await.remove(&p.session_id) {
                e.writer.close().await;
            }
            Ok(json!({}))
        }
        method::EXEC_RUN => {
            let p: ExecParams = parse(params)?;
            let max = p.max_output.unwrap_or(DEFAULT_MAX_OUTPUT);
            let r = exec_target(state, &p.target, false, &p.command, p.timeout_secs, max).await?;
            to_value(ExecResult {
                stdout: r.stdout,
                stderr: r.stderr,
                exit_code: r.exit_code,
                truncated: r.truncated,
            })
        }
        method::SFTP_LIST => {
            let p: SftpPathParams = parse(params)?;
            to_value(sftp_list_target(state, &p.target, false, &p.path).await?)
        }
        method::SFTP_READ => {
            let p: SftpPathParams = parse(params)?;
            let max = p.max_bytes.unwrap_or(256 << 20);
            to_value(sftp_read_target(state, &p.target, false, &p.path, max).await?)
        }
        method::SFTP_WRITE => {
            let p: SftpWriteParams = parse(params)?;
            sftp_write_target(state, &p.target, false, &p.path, &p.data).await?;
            Ok(json!({}))
        }
        method::FORWARD_OPEN => forward_open(state, parse(params)?).await,
        method::FORWARD_LIST => {
            let mut v: Vec<ForwardInfo> = state
                .ssh
                .forwards
                .lock()
                .await
                .values()
                .map(|f| f.info.clone())
                .collect();
            v.sort_by(|a, b| a.id.cmp(&b.id));
            to_value(v)
        }
        method::FORWARD_CLOSE => {
            let p: SessionRef = parse(params)?;
            let entry = state.ssh.forwards.lock().await.remove(&p.session_id);
            match entry {
                Some(f) => {
                    f.handle.stop().await;
                    Ok(json!({}))
                }
                None => Err(err(rpc::NOT_FOUND, "no such forward")),
            }
        }
        method::HOSTKEY_TRUST => {
            // Explicitly trust a key without connecting (e.g. pasted from a provider console).
            let p: Value = params;
            let host = p["host_port"].as_str().unwrap_or_default().to_string();
            let key = p["key"].as_str().unwrap_or_default().to_string();
            if host.is_empty() || key.is_empty() {
                return Err(err(rpc::INVALID_PARAMS, "host_port and key are required"));
            }
            state
                .with_vault(|v| v.put(None, &KnownHost { host, key }).map(|_| ()))
                .await?;
            Ok(json!({}))
        }
        _ => Err(err(rpc::METHOD_NOT_FOUND, format!("unknown method `{m}`"))),
    }
}

async fn pty_writer(state: &Arc<State>, id: &str) -> Result<PtyWriter, RpcError> {
    state
        .ssh
        .ptys
        .lock()
        .await
        .get(id)
        .map(|e| e.writer.clone())
        .ok_or_else(|| err(rpc::NOT_FOUND, "no such PTY session"))
}

async fn pty_open(state: &Arc<State>, conn: &Arc<Conn>, p: PtyOpenParams) -> RpcResult {
    let (_, host) = state.with_vault(|v| v.find::<Host>(&p.target.host)).await?;
    let (writer, mut events, session) = if host.is_telnet() {
        let t = telnet::connect(
            &host.hostname,
            host.port_or_default(),
            &p.term,
            p.cols.max(1),
            p.rows.max(1),
        )
        .await
        .map_err(telnet_err)?;
        (PtyWriter::Telnet(t.writer()), LivePty::Telnet(t), None)
    } else {
        let (_, session) = connect_host(state, &p.target, false, p.agent_forwarding).await?;
        let pty = session
            .open_pty(&p.term, p.cols.max(1), p.rows.max(1))
            .await
            .map_err(ssh_err)?;
        (
            PtyWriter::Ssh(pty.writer()),
            LivePty::Ssh(pty),
            Some(session),
        )
    };
    let session_id = uuid::Uuid::new_v4().to_string();
    state.ssh.ptys.lock().await.insert(
        session_id.clone(),
        PtyEntry {
            writer,
            owner: conn_key(conn),
            _session: session,
        },
    );
    let tx = conn.tx.clone();
    let st = state.clone();
    let sid = session_id.clone();
    tokio::spawn(async move {
        let mut exit = None;
        while let Some(ev) = events.recv().await {
            match ev {
                PtyEvent::Data(d) | PtyEvent::ExtendedData(d) => {
                    let n = Notification::new(
                        event::PTY_DATA,
                        serde_json::to_value(PtyDataEvent {
                            session_id: sid.clone(),
                            data: d,
                        })
                        .unwrap_or_default(),
                    );
                    if tx.send(n).is_err() {
                        break;
                    }
                }
                PtyEvent::Exit(code) => exit = code,
                PtyEvent::Closed => break,
            }
        }
        st.ssh.ptys.lock().await.remove(&sid);
        let _ = tx.send(Notification::new(
            event::PTY_EXIT,
            serde_json::to_value(PtyExitEvent {
                session_id: sid,
                exit_code: exit,
            })
            .unwrap_or_default(),
        ));
    });
    to_value(PtyOpenResult { session_id })
}

async fn forward_open(state: &Arc<State>, p: ForwardOpenParams) -> RpcResult {
    let (host, port) = p
        .dest
        .rsplit_once(':')
        .and_then(|(h, port)| {
            port.parse::<u16>()
                .ok()
                .map(|port| (h.trim_matches(['[', ']']).to_string(), port))
        })
        .ok_or_else(|| err(rpc::INVALID_PARAMS, "dest must be host:port"))?;
    let (host_id, session) = connect_host(state, &p.target, false, false).await?;
    let handle = match p.kind {
        ForwardKind::Local => session.forward_local(&p.bind, &host, port).await,
        ForwardKind::Remote => {
            let (rb, rp) = p
                .bind
                .rsplit_once(':')
                .and_then(|(h, port)| port.parse::<u16>().ok().map(|port| (h.to_string(), port)))
                .ok_or_else(|| err(rpc::INVALID_PARAMS, "bind must be host:port"))?;
            session.forward_remote(&rb, rp, &host, port).await
        }
    }
    .map_err(ssh_err)?;
    let id = uuid::Uuid::new_v4().to_string();
    let info = ForwardInfo {
        id: id.clone(),
        host: p.target.host.clone(),
        kind: p.kind,
        bind: handle.bound_addr().to_string(),
        dest: p.dest.clone(),
    };
    let _ = host_id;
    state.ssh.forwards.lock().await.insert(
        id,
        ForwardEntry {
            info: info.clone(),
            handle,
            _session: session,
        },
    );
    to_value(info)
}

/// Closes PTYs owned by a disconnected client. Forwards outlive connections.
pub async fn connection_closed(state: &Arc<State>, conn: &Arc<Conn>) {
    let me = conn_key(conn);
    let mut ptys = state.ssh.ptys.lock().await;
    let ids: Vec<String> = ptys
        .iter()
        .filter(|(_, e)| e.owner == me)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        if let Some(e) = ptys.remove(&id) {
            e.writer.close().await;
        }
    }
}
