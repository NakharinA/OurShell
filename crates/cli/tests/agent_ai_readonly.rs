//! The typed read-only AI tools (`ai.k8s_get` et al. and `ai.system_status`)
//! through a real agent: the per-host `ai_readonly_auto` opt-in, how it
//! interacts with dangerous mode and the kill switch, and that `run_command`
//! is unaffected by it.

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::process::{Command, Stdio};
use std::time::Duration;

use ourshell::client::{hello, AgentClient, CallError};
use ourshell_proto::agent::{
    event, method, ApprovalRequest, ClientKind, DangerousParams, DecideParams, ExecParams,
    ExecResult, HelloParams, HostTarget, ItemPutParams, SystemStatusParams, VaultInitParams,
};
use ourshell_proto::rpc;
use serde_json::{json, Value};

fn status(host: &str) -> SystemStatusParams {
    SystemStatusParams { host: host.into() }
}

fn exec(host: &str, cmd: &str) -> ExecParams {
    ExecParams {
        target: HostTarget {
            host: host.into(),
            accept_host_key: None,
        },
        command: cmd.into(),
        timeout_secs: Some(10),
        max_output: None,
    }
}

async fn next_request(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ourshell_proto::rpc::Notification>,
) -> ApprovalRequest {
    let fut = async {
        loop {
            let n = events.recv().await.unwrap();
            if n.method == event::APPROVAL_REQUEST {
                return serde_json::from_value::<ApprovalRequest>(n.params).unwrap();
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("no approval request")
}

/// Fails if an approval request shows up within the grace period -- used to
/// assert a call ran without waiting for a human.
async fn assert_no_request_within(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ourshell_proto::rpc::Notification>,
    dur: Duration,
) {
    let fut = async {
        loop {
            let n = events.recv().await.unwrap();
            if n.method == event::APPROVAL_REQUEST {
                panic!("unexpected approval request: {:?}", n.params);
            }
        }
    };
    let _ = tokio::time::timeout(dur, fut).await;
}

fn code(e: CallError) -> i64 {
    e.code().unwrap_or_else(|| panic!("transport error: {e}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn readonly_auto_opt_in_and_its_interactions() {
    let server = common::TestServer::start(None).await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(
        "OURSHELL_AGENT_SOCKET",
        format!("ourshell-ai-ro-test-{}", uuid::Uuid::new_v4().simple()),
    );
    let mut agent = Command::new(env!("CARGO_BIN_EXE_ourshell"))
        .args(["agent", "--no-idle-exit"])
        .env("OURSHELL_VAULT", dir.path().join("v.db"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut human = None;
    for _ in 0..50 {
        if let Ok(c) = AgentClient::connect_existing(hello(ClientKind::Cli)).await {
            human = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (human, mut events) = human.expect("agent did not start");
    let _: Value = human
        .call(method::EVENTS_SUBSCRIBE, &json!({}))
        .await
        .unwrap();

    let _: Value = human
        .call(
            method::VAULT_INIT,
            &VaultInitParams {
                password: "correct horse".into(),
                local_only: true,
            },
        )
        .await
        .unwrap();
    let ident: Value = human
        .call(
            method::ITEM_PUT,
            &ItemPutParams {
                kind: "identity".into(),
                id: None,
                item: json!({"name": "t", "username": "test", "password": "pw"}),
            },
        )
        .await
        .unwrap();
    // "ro": opted in to read-only auto-approval. "noro": AI-allowed but not
    // opted in. "hidden": not AI-allowed at all.
    for (name, ai_allowed, ai_readonly_auto) in [
        ("ro", true, true),
        ("noro", true, false),
        ("hidden", false, false),
    ] {
        let _: Value = human
            .call(
                method::ITEM_PUT,
                &ItemPutParams {
                    kind: "host".into(),
                    id: None,
                    item: json!({
                        "name": name, "hostname": "127.0.0.1", "port": server.addr.port(),
                        "identity_id": ident["id"], "ai_allowed": ai_allowed,
                        "ai_readonly_auto": ai_readonly_auto, "tags": [],
                    }),
                },
            )
            .await
            .unwrap();
    }
    let _: Value = human
        .call(
            method::HOSTKEY_TRUST,
            &json!({"host_port": server.host_port(), "key": server.host_key_openssh()}),
        )
        .await
        .unwrap();

    let (ai, _) = AgentClient::connect_existing(HelloParams {
        protocol: ourshell_proto::AGENT_PROTOCOL_VERSION,
        client: ClientKind::Mcp,
        ai_session: Some("sess-ro".into()),
        ai_client_name: Some("Test AI".into()),
    })
    .await
    .unwrap();

    // 1. Host WITHOUT ai_readonly_auto: a read-only call still waits for approval.
    let ai2 = ai.clone();
    let call = tokio::spawn(async move {
        ai2.call::<_, ExecResult>(method::AI_SYSTEM_STATUS, &status("noro"))
            .await
    });
    let req = next_request(&mut events).await;
    assert_eq!(req.host, "noro");
    assert_eq!(req.tool, "system_status");
    assert_eq!(req.detail, "uptime; df -h; free -m");
    let _: Value = human
        .call(
            method::AI_DECIDE,
            &DecideParams {
                approval_id: req.approval_id,
                approve: true,
            },
        )
        .await
        .unwrap();
    let out = call.await.unwrap().unwrap();
    assert!(out.stdout.contains("Mem:"));

    // 2. Host WITH ai_readonly_auto: runs immediately, no approval request.
    let call = tokio::spawn({
        let ai = ai.clone();
        async move {
            ai.call::<_, ExecResult>(method::AI_SYSTEM_STATUS, &status("ro"))
                .await
        }
    });
    assert_no_request_within(&mut events, Duration::from_millis(500)).await;
    let out = call.await.unwrap().unwrap();
    assert!(out.stdout.contains("Mem:"));

    // A k8s_get round-trip on the same host, to check the agent assembled the
    // exact command end to end (the fixture echoes back any `kubectl` command).
    let get_out: ExecResult = ai
        .call(
            method::AI_K8S_GET,
            &json!({"host": "ro", "kind": "pods", "namespace": "default"}),
        )
        .await
        .unwrap();
    assert_eq!(
        get_out.stdout.trim(),
        "ok: kubectl get 'pods' -n 'default' -o wide"
    );

    let audit: Vec<Value> = human
        .call(method::AI_AUDIT, &json!({"limit": 50}))
        .await
        .unwrap();
    let auto_ro = audit
        .iter()
        .find(|e| e["tool"] == "system_status" && e["host"] == "ro")
        .expect("missing auto-readonly audit entry");
    assert_eq!(auto_ro["mode"], "auto-readonly");
    assert_eq!(auto_ro["approver"], "policy");
    assert_eq!(auto_ro["outcome"], "ok");
    let asked = audit
        .iter()
        .find(|e| e["tool"] == "system_status" && e["host"] == "noro")
        .expect("missing approval audit entry");
    assert_eq!(asked["mode"], "approval");
    assert_eq!(asked["approver"], "user");

    // 3. Dangerous mode on "ro" takes priority over auto-readonly: mode is "dangerous".
    let _: Value = human
        .call(
            method::AI_DANGEROUS,
            &DangerousParams {
                ai_session: "sess-ro".into(),
                hosts: vec!["ro".into()],
                minutes: Some(5),
            },
        )
        .await
        .unwrap();
    let _out: ExecResult = ai
        .call(method::AI_SYSTEM_STATUS, &status("ro"))
        .await
        .unwrap();
    let audit: Vec<Value> = human
        .call(method::AI_AUDIT, &json!({"limit": 50}))
        .await
        .unwrap();
    assert!(audit
        .iter()
        .any(|e| e["tool"] == "system_status" && e["mode"] == "dangerous"));
    // Turn dangerous mode back off before the kill-switch check.
    let _: Value = human
        .call(
            method::AI_DANGEROUS,
            &DangerousParams {
                ai_session: "sess-ro".into(),
                hosts: vec!["ro".into()],
                minutes: Some(0),
            },
        )
        .await
        .unwrap();

    // 4. Hidden host: denied regardless of ai_readonly_auto.
    let e = ai
        .call::<_, ExecResult>(method::AI_SYSTEM_STATUS, &status("hidden"))
        .await
        .unwrap_err();
    assert_eq!(code(e), rpc::POLICY_DENIED);

    // 5. run_command on the auto-readonly host still asks for approval --
    // only the typed read-only tools skip it.
    let ai2 = ai.clone();
    let call = tokio::spawn(async move {
        ai2.call::<_, ExecResult>(method::AI_RUN, &exec("ro", "echo still-asks"))
            .await
    });
    let req = next_request(&mut events).await;
    assert_eq!(req.host, "ro");
    assert_eq!(req.tool, "run_command");
    let _: Value = human
        .call(
            method::AI_DECIDE,
            &DecideParams {
                approval_id: req.approval_id,
                approve: true,
            },
        )
        .await
        .unwrap();
    let out = call.await.unwrap().unwrap();
    assert_eq!(out.stdout.trim(), "still-asks");

    // 6. Kill switch: even the auto-readonly host is denied afterwards.
    let _: Value = human.call(method::AI_STOP, &json!({})).await.unwrap();
    let e = ai
        .call::<_, ExecResult>(method::AI_SYSTEM_STATUS, &status("ro"))
        .await
        .unwrap_err();
    assert_eq!(code(e), rpc::POLICY_DENIED);

    let _: Result<Value, _> = human.call(method::SHUTDOWN, &json!({})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = agent.kill();
    let _ = agent.wait();
}
