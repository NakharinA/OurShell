//! Drives a real agent over its socket against a minimal telnet server.

use std::process::{Command, Stdio};
use std::time::Duration;

use ourshell::client::{hello, AgentClient, CallError};
use ourshell_proto::agent::{
    event, method, ClientKind, ExecParams, HostTarget, ItemPutParams, PtyDataEvent, PtyOpenParams,
    PtyOpenResult, PtyWriteParams, VaultInitParams,
};
use ourshell_proto::rpc;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IAC: u8 = 255;
const WILL: u8 = 251;
const DO: u8 = 253;
const ECHO: u8 = 1;
const NAWS: u8 = 31;

/// Accepts one connection: asks for NAWS, offers ECHO, prints a prompt, then
/// replies once the client has typed `ping` followed by the telnet newline.
/// Returns everything the client sent.
async fn fake_telnet_server(listener: tokio::net::TcpListener) -> Vec<u8> {
    let (mut s, _) = listener.accept().await.unwrap();
    s.write_all(&[IAC, DO, NAWS, IAC, WILL, ECHO])
        .await
        .unwrap();
    s.write_all(b"login: ").await.unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while !got.windows(6).any(|w| w == b"ping\r\n") {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0, "client hung up early");
        got.extend_from_slice(&buf[..n]);
    }
    s.write_all(b"pong-ok\r\n").await.unwrap();
    got
}

#[tokio::test(flavor = "multi_thread")]
async fn telnet_pty_through_agent() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(fake_telnet_server(listener));

    let dir = tempfile::tempdir().unwrap();
    let socket = format!("ourshell-telnet-test-{}", uuid::Uuid::new_v4().simple());
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
    let _: Value = c
        .call(
            method::ITEM_PUT,
            &ItemPutParams {
                kind: "host".into(),
                id: None,
                item: json!({"name": "switch", "hostname": "127.0.0.1", "port": port, "protocol": "telnet", "ai_allowed": true}),
            },
        )
        .await
        .unwrap();
    let target = HostTarget {
        host: "switch".into(),
        accept_host_key: None,
    };

    // SSH-only operations refuse telnet hosts up front.
    let e = c
        .call::<_, Value>(
            method::EXEC_RUN,
            &ExecParams {
                target: target.clone(),
                command: "uptime".into(),
                timeout_secs: None,
                max_output: None,
            },
        )
        .await
        .unwrap_err();
    let CallError::Rpc(e) = e else { panic!("{e}") };
    assert_eq!(e.code, rpc::INVALID_PARAMS);
    assert!(e.message.contains("telnet"), "{}", e.message);

    // Telnet hosts are never offered to AI, even with ai_allowed set.
    let ai: Vec<Value> = c.call(method::AI_HOSTS, &json!({})).await.unwrap();
    assert!(ai.is_empty(), "{ai:?}");

    let opened: PtyOpenResult = c
        .call(
            method::PTY_OPEN,
            &PtyOpenParams {
                target,
                term: "xterm".into(),
                cols: 100,
                rows: 30,
                agent_forwarding: false,
            },
        )
        .await
        .unwrap();

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut wrote = false;
    while !String::from_utf8_lossy(&seen).contains("pong-ok") {
        if !wrote && String::from_utf8_lossy(&seen).contains("login: ") {
            // A terminal sends a bare CR for Enter.
            let _: Value = c
                .call(
                    method::PTY_WRITE,
                    &PtyWriteParams {
                        session_id: opened.session_id.clone(),
                        data: b"ping\r".to_vec(),
                    },
                )
                .await
                .unwrap();
            wrote = true;
        }
        let n = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("no telnet output")
            .expect("agent closed");
        if n.method == event::PTY_DATA {
            let d: PtyDataEvent = serde_json::from_value(n.params).unwrap();
            assert_eq!(d.session_id, opened.session_id);
            seen.extend(d.data);
        }
    }
    // Negotiation bytes never reach the terminal.
    assert!(!seen.contains(&IAC), "{seen:?}");

    let sent = server.await.unwrap();
    // The client accepted ECHO and reported its window size (100x30).
    assert!(sent.windows(3).any(|w| w == [IAC, DO, ECHO]));
    assert!(sent.windows(4).any(|w| w == [0, 100, 0, 30]));

    let _: Result<Value, _> = c.call(method::SHUTDOWN, &json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while agent.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let _ = agent.kill();
}
