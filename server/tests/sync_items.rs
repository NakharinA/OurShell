mod common;

use axum::http::StatusCode;
use common::{as_session, new_device, register_user, request, setup};
use ourshell_proto::sync::{PullResponse, PushRequest, PushResponse, SyncItem};
use sqlx_core::query_as::query_as;
use uuid::Uuid;

fn item(vault_id: &str, id: &str, version: u64, plaintext: &str) -> SyncItem {
    SyncItem {
        id: id.to_string(),
        kind: "host".to_string(),
        version,
        vault_id: vault_id.to_string(),
        deleted: false,
        envelope: plaintext.to_string(),
        updated_at: None,
    }
}

#[tokio::test]
async fn push_pull_versioning_and_conflicts() {
    let Some(app) = setup().await else { return };
    let (status, body) =
        register_user(&app.router, "eve@example.com", "pw", None, new_device("d1")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);
    let vault_id = session.vaults[0].vault_id.clone();
    let token = session.session_token.clone();
    let item_id = Uuid::new_v4().to_string();

    // Pushing a brand-new item (any version) succeeds.
    let push_url = format!("/v1/vaults/{vault_id}/items");
    let (status, body) = request(
        &app.router,
        "POST",
        &push_url,
        Some(&token),
        Some(&PushRequest {
            items: vec![item(&vault_id, &item_id, 1, "plaintext-json-v1")],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let resp: PushResponse = serde_json::from_value(body).unwrap();
    assert_eq!(resp.results[0].status, "ok");

    // Pulling from cursor 0 sees it.
    let pull_url = format!("/v1/vaults/{vault_id}/items?since=0");
    let (status, body) = request::<()>(&app.router, "GET", &pull_url, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let pulled: PullResponse = serde_json::from_value(body).unwrap();
    assert_eq!(pulled.items.len(), 1);
    assert_eq!(pulled.items[0].envelope, "plaintext-json-v1");
    let cursor = pulled.cursor;
    assert!(cursor > 0);

    // Pulling again from that cursor sees nothing new.
    let pull_url2 = format!("/v1/vaults/{vault_id}/items?since={cursor}");
    let (status, body) = request::<()>(&app.router, "GET", &pull_url2, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let pulled2: PullResponse = serde_json::from_value(body).unwrap();
    assert!(pulled2.items.is_empty());

    // Pushing version 2 (current+1) succeeds.
    let (status, body) = request(
        &app.router,
        "POST",
        &push_url,
        Some(&token),
        Some(&PushRequest {
            items: vec![item(&vault_id, &item_id, 2, "plaintext-json-v2")],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let resp: PushResponse = serde_json::from_value(body).unwrap();
    assert_eq!(resp.results[0].status, "ok");

    // Pushing version 2 again (stale: current is now 2, so needs 3) conflicts
    // and returns the server's current copy.
    let (status, body) = request(
        &app.router,
        "POST",
        &push_url,
        Some(&token),
        Some(&PushRequest {
            items: vec![item(&vault_id, &item_id, 2, "stale-write")],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let resp: PushResponse = serde_json::from_value(body).unwrap();
    assert_eq!(resp.results[0].status, "conflict");
    let current = resp.results[0]
        .current
        .as_ref()
        .expect("conflict carries current");
    assert_eq!(current.version, 2);
    assert_eq!(current.envelope, "plaintext-json-v2");
}

#[tokio::test]
async fn pushed_envelopes_are_encrypted_at_rest() {
    let Some(app) = setup().await else { return };
    let (status, body) = register_user(
        &app.router,
        "frank@example.com",
        "pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);
    let vault_id = session.vaults[0].vault_id.clone();
    let item_id = Uuid::new_v4().to_string();
    let secret_marker = "super-secret-ssh-password-marker";

    let push_url = format!("/v1/vaults/{vault_id}/items");
    let (status, _) = request(
        &app.router,
        "POST",
        &push_url,
        Some(&session.session_token),
        Some(&PushRequest {
            items: vec![item(
                &vault_id,
                &item_id,
                1,
                &format!("{{\"password\":\"{secret_marker}\"}}"),
            )],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let vault_uuid = Uuid::parse_str(&vault_id).unwrap();
    let item_uuid = Uuid::parse_str(&item_id).unwrap();
    let (raw_envelope,): (Vec<u8>,) =
        query_as::<_, (Vec<u8>,)>("SELECT envelope FROM items WHERE vault_id = $1 AND id = $2")
            .bind(vault_uuid)
            .bind(item_uuid)
            .fetch_one(&app.pool)
            .await
            .unwrap();

    let raw_text = String::from_utf8_lossy(&raw_envelope);
    assert!(
        !raw_text.contains(secret_marker),
        "the DB column must never contain the plaintext envelope"
    );
    assert!(
        !raw_text.contains("password"),
        "the DB column must never contain the plaintext envelope JSON"
    );
}
