//! Changing the master password always needs the current one, whether or
//! not the vault is unlocked (drives a real agent over its socket).

use std::process::{Command, Stdio};
use std::time::Duration;

use ourshell::client::{hello, AgentClient, CallError};
use ourshell_proto::agent::{
    method, AccountChangePasswordParams, ClientKind, UnlockParams, VaultInitParams,
};
use serde_json::{json, Value};

const OLD: &str = "correct horse battery";
const NEW: &str = "another long password";

async fn change(c: &AgentClient, old: Option<&str>, new: &str) -> Result<Value, CallError> {
    c.call(
        method::ACCOUNT_CHANGE_PASSWORD,
        &AccountChangePasswordParams {
            old_password: old.map(str::to_string),
            new_password: new.to_string(),
        },
    )
    .await
}

async fn unlock(c: &AgentClient, pw: &str) -> Result<Value, CallError> {
    c.call(
        method::VAULT_UNLOCK,
        &UnlockParams {
            password: pw.into(),
            auto_lock_secs: None,
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn change_password_requires_the_current_one() {
    let dir = tempfile::tempdir().unwrap();
    let socket = format!("ourshell-passwd-test-{}", uuid::Uuid::new_v4().simple());
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
    let (c, _events) = conn.expect("agent did not start");
    let _: Value = c
        .call(
            method::VAULT_INIT,
            &VaultInitParams {
                password: OLD.into(),
                local_only: true,
            },
        )
        .await
        .unwrap();

    // Unlocked vault: missing or wrong current password is refused.
    let e = change(&c, None, NEW).await.unwrap_err();
    assert!(e.to_string().contains("current master password"), "{e}");
    let e = change(&c, Some(""), NEW).await.unwrap_err();
    assert!(e.to_string().contains("current master password"), "{e}");
    let e = change(&c, Some("wrong password"), NEW).await.unwrap_err();
    assert!(e.to_string().contains("wrong master password"), "{e}");
    // ...and nothing changed: the old password still unlocks.
    let _: Value = c.call(method::VAULT_LOCK, &json!({})).await.unwrap();
    unlock(&c, OLD).await.expect("old password still works");

    // The right current password changes it.
    change(&c, Some(OLD), NEW)
        .await
        .expect("change with the current password");
    let _: Value = c.call(method::VAULT_LOCK, &json!({})).await.unwrap();
    assert!(
        unlock(&c, OLD).await.is_err(),
        "old password no longer works"
    );
    unlock(&c, NEW).await.expect("new password works");

    // Locked vault: same rules.
    let _: Value = c.call(method::VAULT_LOCK, &json!({})).await.unwrap();
    assert!(change(&c, Some("wrong password"), OLD).await.is_err());
    change(&c, Some(NEW), OLD)
        .await
        .expect("change while locked");
    let _: Value = c.call(method::VAULT_LOCK, &json!({})).await.unwrap();
    unlock(&c, OLD).await.expect("changed back");

    let _: Result<Value, _> = c.call(method::SHUTDOWN, &json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while agent.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let _ = agent.kill();
}
