//! Drives a real agent over its socket: PTY streaming and host-key trust.

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::process::{Command, Stdio};
use std::time::Duration;

use ourshell::client::{hello, AgentClient, CallError};
use ourshell_proto::agent::{
    event, method, ClientKind, HostTarget, ItemPutParams, PtyDataEvent, PtyOpenParams,
    PtyOpenResult, PtyWriteParams, VaultInitParams,
};
use ourshell_proto::rpc;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn pty_streams_through_agent() {
    let server = common::TestServer::start(None).await;
    let dir = tempfile::tempdir().unwrap();
    let socket = format!("ourshell-pty-test-{}", uuid::Uuid::new_v4().simple());
    // This test binary is the only user of these variables.
    std::env::set_var("OURSHELL_AGENT_SOCKET", &socket);
    let mut agent = Command::new(env!("CARGO_BIN_EXE_ourshell"))
        .args(["agent", "--no-idle-exit"])
        .env("OURSHELL_VAULT", dir.path().join("v.db"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut conn = None;
    for _ in 0..50 {
        if let Ok(c) = AgentClient::connect_existing(hello(ClientKind::Cli)).await {
            conn = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (c, mut events) = conn.expect("agent did not start");

    let _: Value = c
        .call(
            method::VAULT_INIT,
            &VaultInitParams {
                password: "correct horse".into(),
                local_only: true,
            },
        )
        .await
        .unwrap();
    let ident: Value = c
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
    let _: Value = c
        .call(
            method::ITEM_PUT,
            &ItemPutParams {
                kind: "host".into(),
                id: None,
                item: json!({"name": "box", "hostname": "127.0.0.1", "port": server.addr.port(), "identity_id": ident["id"]}),
            },
        )
        .await
        .unwrap();

    let mut open = PtyOpenParams {
        target: HostTarget {
            host: "box".into(),
            accept_host_key: None,
        },
        term: "xterm".into(),
        cols: 80,
        rows: 24,
        agent_forwarding: false,
    };
    // Unknown host key -> structured error carrying the fingerprint.
    let e = c
        .call::<_, PtyOpenResult>(method::PTY_OPEN, &open)
        .await
        .unwrap_err();
    let CallError::Rpc(e) = e else { panic!("{e}") };
    assert_eq!(e.code, rpc::HOST_KEY_UNKNOWN);
    let fp = e.data.unwrap()["fingerprint"].as_str().unwrap().to_string();
    assert!(fp.starts_with("SHA256:"));

    // Accepting by fingerprint works (the desktop app does this).
    open.target.accept_host_key = Some(fp);
    let opened: PtyOpenResult = c.call(method::PTY_OPEN, &open).await.unwrap();
    let _: Value = c
        .call(
            method::PTY_WRITE,
            &PtyWriteParams {
                session_id: opened.session_id.clone(),
                data: b"ping-123\n".to_vec(),
            },
        )
        .await
        .unwrap();

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains("ping-123") {
        let n = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("no PTY output")
            .expect("agent closed");
        if n.method == event::PTY_DATA {
            let d: PtyDataEvent = serde_json::from_value(n.params).unwrap();
            assert_eq!(d.session_id, opened.session_id);
            seen.extend(d.data);
        }
    }

    let _: Result<Value, _> = c.call(method::SHUTDOWN, &json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while agent.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let _ = agent.kill();
}
