mod common;

use axum::http::StatusCode;
use common::{as_session, new_device, register_user, request, setup};
use ourshell_proto::sync::{
    AddVaultMemberRequest, CreateVaultRequest, IdResponse, PublicKeysResponse, PullResponse,
    PushRequest, SyncItem, VaultMemberSeal, VaultMembersResponse,
};
use uuid::Uuid;

fn item(vault_id: &str, id: &str, plaintext: &str) -> SyncItem {
    SyncItem {
        id: id.to_string(),
        kind: "host".to_string(),
        version: 1,
        vault_id: vault_id.to_string(),
        deleted: false,
        envelope: plaintext.to_string(),
        updated_at: None,
    }
}

#[tokio::test]
async fn team_vault_create_share_and_pull_by_member() {
    let Some(app) = setup().await else { return };

    let (status, body) = register_user(
        &app.router,
        "owner@example.com",
        "pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let a = as_session(&body);

    let (status, body) = register_user(
        &app.router,
        "member@example.com",
        "pw",
        None,
        new_device("d2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let b = as_session(&body);

    let (status, body) = register_user(
        &app.router,
        "outsider@example.com",
        "pw",
        None,
        new_device("d3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let outsider = as_session(&body);

    // A looks up B's public keys to seal the team vault key to them.
    let (status, body) = request::<()>(
        &app.router,
        "GET",
        "/v1/users/member@example.com/public-keys",
        Some(&a.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let b_keys: PublicKeysResponse = serde_json::from_value(body).unwrap();
    assert_eq!(b_keys.user_id, b.user_id);

    // A creates a team vault, sealing a key for both itself and B.
    let vault_id = Uuid::new_v4().to_string();
    let (status, body) = request(
        &app.router,
        "POST",
        "/v1/vaults",
        Some(&a.session_token),
        Some(&CreateVaultRequest {
            vault_id: vault_id.clone(),
            key_id: "team-key-1".to_string(),
            name: "Ops Team".to_string(),
            members: vec![
                VaultMemberSeal {
                    user_id: a.user_id.clone(),
                    sealed_vault_key: b"sealed-for-a".to_vec(),
                },
                VaultMemberSeal {
                    user_id: b.user_id.clone(),
                    sealed_vault_key: b"sealed-for-b".to_vec(),
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let created: IdResponse = serde_json::from_value(body).unwrap();
    assert_eq!(created.id, vault_id);

    // A pushes an item into the team vault.
    let push_url = format!("/v1/vaults/{vault_id}/items");
    let item_id = Uuid::new_v4().to_string();
    let (status, _) = request(
        &app.router,
        "POST",
        &push_url,
        Some(&a.session_token),
        Some(&PushRequest {
            items: vec![item(&vault_id, &item_id, "team-secret-json")],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // B, as a member, can pull it.
    let pull_url = format!("/v1/vaults/{vault_id}/items?since=0");
    let (status, body) =
        request::<()>(&app.router, "GET", &pull_url, Some(&b.session_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let pulled: PullResponse = serde_json::from_value(body).unwrap();
    assert_eq!(pulled.items.len(), 1);
    assert_eq!(pulled.items[0].envelope, "team-secret-json");

    // The outsider is not a member and is refused.
    let (status, _) = request::<()>(
        &app.router,
        "GET",
        &pull_url,
        Some(&outsider.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Any member can list the vault's current members (needed to accept
    // their item signatures, and to reseal a rotated key to each of
    // them).
    let members_url = format!("/v1/vaults/{vault_id}/members");
    let (status, body) = request::<()>(
        &app.router,
        "GET",
        &members_url,
        Some(&b.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let members: VaultMembersResponse = serde_json::from_value(body).unwrap();
    let mut emails: Vec<&str> = members.members.iter().map(|m| m.email.as_str()).collect();
    emails.sort_unstable();
    assert_eq!(emails, vec!["member@example.com", "owner@example.com"]);

    // A non-member is refused.
    let (status, _) = request::<()>(
        &app.router,
        "GET",
        &members_url,
        Some(&outsider.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // B adds the outsider as a new member.
    let add_url = format!("/v1/vaults/{vault_id}/members");
    let (status, _) = request(
        &app.router,
        "POST",
        &add_url,
        Some(&b.session_token),
        Some(&AddVaultMemberRequest {
            user_id: outsider.user_id.clone(),
            key_id: "team-key-1".to_string(),
            sealed_vault_key: b"sealed-for-outsider".to_vec(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Now the former outsider can pull too.
    let (status, body) = request::<()>(
        &app.router,
        "GET",
        &pull_url,
        Some(&outsider.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let pulled: PullResponse = serde_json::from_value(body).unwrap();
    assert_eq!(pulled.items.len(), 1);

    // A removes the (former) outsider.
    let remove_url = format!("/v1/vaults/{vault_id}/members/{}", outsider.user_id);
    let (status, _) = request::<()>(
        &app.router,
        "DELETE",
        &remove_url,
        Some(&a.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request::<()>(
        &app.router,
        "GET",
        &pull_url,
        Some(&outsider.session_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "removed member must lose access"
    );
}

#[tokio::test]
async fn admin_endpoints_require_admin_role() {
    let Some(app) = setup().await else { return };
    let (status, body) = register_user(
        &app.router,
        "owner2@example.com",
        "pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let owner = as_session(&body);

    let (status, body) = register_user(
        &app.router,
        "plain@example.com",
        "pw",
        None,
        new_device("d2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let plain_member = as_session(&body);

    // A plain member cannot list org users or create invites.
    let (status, _) = request::<()>(
        &app.router,
        "GET",
        "/v1/orgs/users",
        Some(&plain_member.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The owner can.
    let (status, body) = request::<()>(
        &app.router,
        "GET",
        "/v1/orgs/users",
        Some(&owner.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}
