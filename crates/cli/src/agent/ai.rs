//! `ai.*` handlers: the only methods MCP clients may call.
//!
//! Every action is checked against [`Policy`](super::policy::Policy), waits for a
//! human when required, runs through the same SSH code as the CLI, and is audited.

use std::sync::Arc;

use chrono::Utc;
use ourshell_core::vault::{AuditEntry, Host};
use ourshell_proto::agent::{
    event, method, AiHost, AiStatus, ApprovalRequest, AuditQuery, ClientKind, DangerousParams,
    DecideParams, ExecParams, ExecResult, FileData, K8sDescribeParams, K8sEventsParams,
    K8sGetParams, K8sLogsParams, SftpPathParams, SftpWriteParams, SystemStatusParams,
};
use ourshell_proto::rpc::{self, RpcError};
use serde_json::{json, Value};

use super::k8s::{self, K8sError};
use super::policy::{self, Decision, DEFAULT_DANGEROUS_MINUTES, DEFAULT_MAX_OUTPUT};
use super::{err, parse, ssh_ops, to_value, Conn, RpcResult, State};

/// Methods an MCP (AI) connection is allowed to call.
pub const AI_CALLABLE: &[&str] = &[
    method::AI_HOSTS,
    method::AI_RUN,
    method::AI_LIST_DIR,
    method::AI_READ_FILE,
    method::AI_WRITE_FILE,
    method::AI_K8S_GET,
    method::AI_K8S_DESCRIBE,
    method::AI_K8S_LOGS,
    method::AI_K8S_EVENTS,
    method::AI_SYSTEM_STATUS,
];

pub async fn dispatch(state: &Arc<State>, conn: &Arc<Conn>, m: &str, params: Value) -> RpcResult {
    let human = conn.kind != ClientKind::Mcp;
    match m {
        method::AI_HOSTS => to_value(ai_hosts(state).await?),
        method::AI_RUN => run(state, conn, parse(params)?).await,
        method::AI_LIST_DIR => list_dir(state, conn, parse(params)?).await,
        method::AI_READ_FILE => read_file(state, conn, parse(params)?).await,
        method::AI_WRITE_FILE => write_file(state, conn, parse(params)?).await,
        method::AI_K8S_GET => k8s_get(state, conn, parse(params)?).await,
        method::AI_K8S_DESCRIBE => k8s_describe(state, conn, parse(params)?).await,
        method::AI_K8S_LOGS => k8s_logs(state, conn, parse(params)?).await,
        method::AI_K8S_EVENTS => k8s_events(state, conn, parse(params)?).await,
        method::AI_SYSTEM_STATUS => system_status(state, conn, parse(params)?).await,
        // Everything below is for humans only; AI clients never reach it.
        _ if !human => Err(err(rpc::POLICY_DENIED, "not available to AI clients")),
        method::AI_APPROVALS => to_value(state.policy.lock().unwrap().pending()),
        method::AI_DECIDE => {
            let p: DecideParams = parse(params)?;
            let found = state
                .policy
                .lock()
                .unwrap()
                .resolve(&p.approval_id, p.approve);
            if !found {
                return Err(err(rpc::NOT_FOUND, "no such pending approval"));
            }
            state.emit(
                event::APPROVAL_RESOLVED,
                json!({"approval_id": p.approval_id, "approved": p.approve}),
            );
            Ok(json!({}))
        }
        method::AI_DANGEROUS => dangerous(state, parse(params)?).await,
        method::AI_STOP => {
            state.policy.lock().unwrap().stop_all();
            state.emit(event::AI_MODE, json!({"stopped": true}));
            Ok(json!({}))
        }
        method::AI_STATUS => {
            let p = state.policy.lock().unwrap();
            to_value(AiStatus {
                sessions: p.sessions(Utc::now()),
                pending: p.pending(),
            })
        }
        method::AI_AUDIT => {
            let q: AuditQuery = parse(params)?;
            let mut entries = state
                .with_vault(|v| v.list::<AuditEntry>())
                .await?
                .into_iter()
                .map(|(_, e)| e)
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| b.time.cmp(&a.time));
            entries.truncate(q.limit.unwrap_or(100));
            to_value(entries)
        }
        _ => Err(err(rpc::METHOD_NOT_FOUND, format!("unknown method `{m}`"))),
    }
}

async fn ai_hosts(state: &Arc<State>) -> Result<Vec<AiHost>, RpcError> {
    let hosts = state.with_vault(|v| v.list::<Host>()).await?;
    let mut out: Vec<AiHost> = hosts
        .into_iter()
        .filter(|(_, h)| h.ai_allowed)
        .map(|(_, h)| AiHost {
            name: h.name,
            tags: h.tags,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Resolves a host for AI use. Hidden hosts look exactly like missing ones.
async fn ai_host(state: &Arc<State>, name: &str) -> Result<(String, Host), RpcError> {
    let found = state.with_vault(|v| v.find::<Host>(name)).await;
    match found {
        Ok((id, h)) if h.ai_allowed => Ok((id, h)),
        _ => Err(err(
            rpc::POLICY_DENIED,
            format!("host `{name}` is not available to AI; call list_hosts"),
        )),
    }
}

struct Gate {
    host_id: String,
    host_name: String,
    mode: &'static str,
    approver: String,
}

/// Applies the policy and, when needed, waits for a human decision.
///
/// `read_only` is only ever `true` for the typed read-only AI tools
/// (`k8s_get`/`k8s_describe`/`k8s_logs`/`k8s_events`/`system_status`); it lets
/// a host that opted in via `ai_readonly_auto` skip the approval wait for
/// *those* tools specifically. Dangerous mode (checked first, via `decide`)
/// and the kill switch / `ai_allowed` gate (also via `decide`) are unchanged
/// and always take priority over this.
async fn gate(
    state: &Arc<State>,
    conn: &Conn,
    host: &str,
    tool: &str,
    detail: &str,
    read_only: bool,
) -> Result<Gate, RpcError> {
    let session = conn
        .ai_session
        .clone()
        .ok_or_else(|| err(rpc::POLICY_DENIED, "ai.* calls require an AI session"))?;
    let (host_id, h) = ai_host(state, host).await?;
    let decision = state
        .policy
        .lock()
        .unwrap()
        .decide(&session, &h.name, h.ai_allowed, Utc::now());
    let gate = |mode, approver: &str| Gate {
        host_id: host_id.clone(),
        host_name: h.name.clone(),
        mode,
        approver: approver.into(),
    };
    match decision {
        Decision::Deny(reason) => {
            audit(
                state, &session, &h.name, tool, detail, "approval", "-", "denied", None,
            )
            .await;
            Err(err(rpc::POLICY_DENIED, reason))
        }
        Decision::Allow => Ok(gate("dangerous", "dangerous-mode")),
        Decision::Ask if read_only && h.ai_readonly_auto => Ok(gate("auto-readonly", "policy")),
        Decision::Ask => {
            let req = ApprovalRequest {
                approval_id: uuid::Uuid::new_v4().to_string(),
                ai_session: session.clone(),
                ai_client_name: conn.ai_client_name.clone(),
                host: h.name.clone(),
                tool: tool.into(),
                detail: detail.into(),
                created_at: Utc::now().to_rfc3339(),
            };
            let rx = state.policy.lock().unwrap().request_approval(req.clone());
            state.emit(event::APPROVAL_REQUEST, to_value(&req)?);
            let answer = tokio::time::timeout(policy::APPROVAL_TIMEOUT, rx).await;
            match answer {
                Ok(Ok(true)) => Ok(gate("approval", "user")),
                Ok(_) => {
                    audit(
                        state, &session, &h.name, tool, detail, "approval", "user", "rejected",
                        None,
                    )
                    .await;
                    Err(err(rpc::APPROVAL_REJECTED, "the user rejected this action"))
                }
                Err(_) => {
                    state
                        .policy
                        .lock()
                        .unwrap()
                        .resolve(&req.approval_id, false);
                    state.emit(
                        event::APPROVAL_RESOLVED,
                        json!({"approval_id": req.approval_id, "approved": false, "timed_out": true}),
                    );
                    audit(
                        state, &session, &h.name, tool, detail, "approval", "-", "timeout", None,
                    )
                    .await;
                    Err(err(rpc::APPROVAL_TIMEOUT, "approval timed out"))
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn audit(
    state: &Arc<State>,
    session: &str,
    host: &str,
    tool: &str,
    detail: &str,
    mode: &str,
    approver: &str,
    outcome: &str,
    exit_code: Option<i32>,
) {
    let entry = AuditEntry {
        time: Utc::now().to_rfc3339(),
        ai_session: session.into(),
        host: host.into(),
        tool: tool.into(),
        detail: policy::redact(detail),
        mode: mode.into(),
        approver: approver.into(),
        outcome: outcome.into(),
        exit_code,
    };
    state.emit(
        event::AI_CALL,
        serde_json::to_value(&entry).unwrap_or_default(),
    );
    // The audit log lives in the encrypted vault; it is lost only if the vault is locked.
    let _ = state.with_vault(|v| v.put(None, &entry).map(|_| ())).await;
}

fn session(conn: &Conn) -> String {
    conn.ai_session.clone().unwrap_or_default()
}

async fn run(state: &Arc<State>, conn: &Arc<Conn>, p: ExecParams) -> RpcResult {
    let g = gate(
        state,
        conn,
        &p.target.host,
        "run_command",
        &p.command,
        false,
    )
    .await?;
    let max = p
        .max_output
        .unwrap_or(DEFAULT_MAX_OUTPUT)
        .min(DEFAULT_MAX_OUTPUT * 16);
    // Fetch a little more than the cap: redact the whole thing, then cut.
    let fetch = max + policy::REDACTION_SLACK;
    let res = ssh_ops::exec(state, &g.host_id, true, &p.command, p.timeout_secs, fetch).await;
    match res {
        Ok(r) => {
            audit(
                state,
                &session(conn),
                &g.host_name,
                "run_command",
                &p.command,
                g.mode,
                &g.approver,
                "ok",
                r.exit_code.map(|c| c as i32),
            )
            .await;
            let (stdout, t1) = policy::cap(policy::redact(&r.stdout), max);
            let (stderr, t2) = policy::cap(policy::redact(&r.stderr), max);
            to_value(ExecResult {
                stdout,
                stderr,
                exit_code: r.exit_code,
                truncated: r.truncated || t1 || t2,
            })
        }
        Err(e) => {
            audit(
                state,
                &session(conn),
                &g.host_name,
                "run_command",
                &p.command,
                g.mode,
                &g.approver,
                "error",
                None,
            )
            .await;
            Err(e)
        }
    }
}

async fn list_dir(state: &Arc<State>, conn: &Arc<Conn>, p: SftpPathParams) -> RpcResult {
    let g = gate(state, conn, &p.target.host, "list_dir", &p.path, false).await?;
    let res = ssh_ops::sftp_list(state, &g.host_id, true, &p.path).await;
    let outcome = if res.is_ok() { "ok" } else { "error" };
    audit(
        state,
        &session(conn),
        &g.host_name,
        "list_dir",
        &p.path,
        g.mode,
        &g.approver,
        outcome,
        None,
    )
    .await;
    to_value(res?)
}

async fn read_file(state: &Arc<State>, conn: &Arc<Conn>, p: SftpPathParams) -> RpcResult {
    let g = gate(state, conn, &p.target.host, "read_file", &p.path, false).await?;
    let max = p
        .max_bytes
        .unwrap_or(DEFAULT_MAX_OUTPUT)
        .min(DEFAULT_MAX_OUTPUT * 16);
    let res = ssh_ops::sftp_read(
        state,
        &g.host_id,
        true,
        &p.path,
        max + policy::REDACTION_SLACK,
    )
    .await;
    let outcome = if res.is_ok() { "ok" } else { "error" };
    audit(
        state,
        &session(conn),
        &g.host_name,
        "read_file",
        &p.path,
        g.mode,
        &g.approver,
        outcome,
        None,
    )
    .await;
    let mut f: FileData = res?;
    match std::str::from_utf8(&f.data) {
        Ok(text) => {
            let (red, cut) = policy::cap(policy::redact(text), max);
            f.data = red.into_bytes();
            f.truncated |= cut;
        }
        Err(_) => {
            // Binary: nothing to redact, just enforce the cap.
            if f.data.len() > max {
                f.data.truncate(max);
                f.truncated = true;
            }
        }
    }
    to_value(f)
}

async fn write_file(state: &Arc<State>, conn: &Arc<Conn>, p: SftpWriteParams) -> RpcResult {
    let detail = format!("{} ({} bytes)", p.path, p.data.len());
    let g = gate(state, conn, &p.target.host, "write_file", &detail, false).await?;
    let res = ssh_ops::sftp_write(state, &g.host_id, true, &p.path, &p.data).await;
    let outcome = if res.is_ok() { "ok" } else { "error" };
    audit(
        state,
        &session(conn),
        &g.host_name,
        "write_file",
        &detail,
        g.mode,
        &g.approver,
        outcome,
        None,
    )
    .await;
    res?;
    Ok(json!({}))
}

fn k8s_err(e: K8sError) -> RpcError {
    match &e {
        K8sError::Invalid(msg) => err(rpc::INVALID_PARAMS, msg.clone()),
        K8sError::ConfigmapDescribeDenied => err(rpc::POLICY_DENIED, e.to_string()),
    }
}

/// Runs an already-built, read-only remote command through the gate (which
/// may skip approval when the host opted in via `ai_readonly_auto`), then
/// audits, redacts and caps the result exactly like `run_command` does.
async fn exec_readonly(
    state: &Arc<State>,
    conn: &Arc<Conn>,
    host: &str,
    tool: &str,
    command: &str,
) -> RpcResult {
    let g = gate(state, conn, host, tool, command, true).await?;
    let max = DEFAULT_MAX_OUTPUT;
    let fetch = max + policy::REDACTION_SLACK;
    let res = ssh_ops::exec(state, &g.host_id, true, command, None, fetch).await;
    match res {
        Ok(r) => {
            audit(
                state,
                &session(conn),
                &g.host_name,
                tool,
                command,
                g.mode,
                &g.approver,
                "ok",
                r.exit_code.map(|c| c as i32),
            )
            .await;
            let (stdout, t1) = policy::cap(policy::redact(&r.stdout), max);
            let (stderr, t2) = policy::cap(policy::redact(&r.stderr), max);
            to_value(ExecResult {
                stdout,
                stderr,
                exit_code: r.exit_code,
                truncated: r.truncated || t1 || t2,
            })
        }
        Err(e) => {
            audit(
                state,
                &session(conn),
                &g.host_name,
                tool,
                command,
                g.mode,
                &g.approver,
                "error",
                None,
            )
            .await;
            Err(e)
        }
    }
}

async fn k8s_get(state: &Arc<State>, conn: &Arc<Conn>, p: K8sGetParams) -> RpcResult {
    let cmd =
        k8s::get_command(&p.kind, p.namespace.as_deref(), p.name.as_deref()).map_err(k8s_err)?;
    exec_readonly(state, conn, &p.host, "k8s_get", &cmd).await
}

async fn k8s_describe(state: &Arc<State>, conn: &Arc<Conn>, p: K8sDescribeParams) -> RpcResult {
    let cmd = k8s::describe_command(&p.kind, p.namespace.as_deref(), &p.name).map_err(k8s_err)?;
    exec_readonly(state, conn, &p.host, "k8s_describe", &cmd).await
}

async fn k8s_logs(state: &Arc<State>, conn: &Arc<Conn>, p: K8sLogsParams) -> RpcResult {
    let cmd = k8s::logs_command(
        &p.namespace,
        &p.pod,
        p.container.as_deref(),
        p.tail,
        p.since.as_deref(),
    )
    .map_err(k8s_err)?;
    exec_readonly(state, conn, &p.host, "k8s_logs", &cmd).await
}

async fn k8s_events(state: &Arc<State>, conn: &Arc<Conn>, p: K8sEventsParams) -> RpcResult {
    let cmd = k8s::events_command(&p.namespace).map_err(k8s_err)?;
    exec_readonly(state, conn, &p.host, "k8s_events", &cmd).await
}

async fn system_status(state: &Arc<State>, conn: &Arc<Conn>, p: SystemStatusParams) -> RpcResult {
    let cmd = k8s::system_status_command();
    exec_readonly(state, conn, &p.host, "system_status", cmd).await
}

async fn dangerous(state: &Arc<State>, p: DangerousParams) -> RpcResult {
    let minutes = p.minutes.unwrap_or(DEFAULT_DANGEROUS_MINUTES);
    // Dangerous mode can only cover AI-allowed hosts.
    let mut hosts = Vec::new();
    for name in &p.hosts {
        let (_, h) = ai_host(state, name).await?;
        hosts.push(h.name);
    }
    state
        .policy
        .lock()
        .unwrap()
        .set_dangerous(&p.ai_session, hosts.clone(), minutes, Utc::now())
        .map_err(|e| err(rpc::INVALID_PARAMS, e))?;
    state.emit(
        event::AI_MODE,
        json!({"ai_session": p.ai_session, "dangerous": minutes > 0 && !hosts.is_empty(), "hosts": hosts, "minutes": minutes}),
    );
    Ok(json!({}))
}
