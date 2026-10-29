//! Every method declared in `ourshell_proto::agent::method` must reach a handler.
//! (Team/org methods once fell through to METHOD_NOT_FOUND unnoticed.)

use std::process::{Command, Stdio};
use std::time::Duration;

use ourshell::client::{hello, AgentClient};
use ourshell_proto::agent::{method, ClientKind, VaultInitParams};
use ourshell_proto::rpc;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn sync_team_and_org_methods_are_routed() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(
        "OURSHELL_AGENT_SOCKET",
        format!("ourshell-routing-test-{}", uuid::Uuid::new_v4().simple()),
    );
    let mut agent = Command::new(env!("CARGO_BIN_EXE_ourshell"))
        .args(["agent", "--no-idle-exit"])
        .env("OURSHELL_VAULT", dir.path().join("v.db"))
        .env("OURSHELL_AUTOSYNC_SECS", "0")
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
    let (c, _) = conn.expect("agent did not start");
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

    // Local-only answers work without a server.
    let teams: Vec<Value> = c.call(method::TEAM_LIST, &json!({})).await.unwrap();
    assert!(teams.iter().any(|t| t["kind"] == "personal"), "{teams:?}");

    // The rest need a sync session; they must fail for that reason, not as unknown methods.
    for m in [
        method::TEAM_MEMBERS,
        method::TEAM_CREATE,
        method::TEAM_ADD_MEMBER,
        method::TEAM_REMOVE_MEMBER,
        method::TEAM_SHARE,
        method::ORG_INVITE,
        method::ORG_USERS,
        method::ORG_DISABLE_USER,
        method::ACCOUNT_STATUS,
        method::SYNC_RUN,
    ] {
        let params = json!({"team": "x", "email": "a@b.c", "name": "x", "members": [],
                            "item_id": "x", "user_id": "x"});
        match c.call_value(m, params).await {
            Ok(_) => {}
            Err(e) => assert_ne!(
                e.code(),
                Some(rpc::METHOD_NOT_FOUND),
                "`{m}` is not routed to a handler: {e}"
            ),
        }
    }

    let _: Result<Value, _> = c.call(method::SHUTDOWN, &json!({})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = agent.kill();
    let _ = agent.wait();
}
