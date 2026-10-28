//! `POST /v1/account/password/start` + `/finish`: changing the current
//! user's password (or pushing recovered account key material) revokes
//! every other session and lets a later login use the new password only.

mod common;

use axum::http::StatusCode;
use common::{as_session, login_user, new_device, register_user, request};
use ourshell_proto::opaque;
use ourshell_proto::sync::{
    AccountPasswordFinish, AccountPasswordStart, AccountPasswordStartResponse,
};

#[tokio::test]
async fn password_change_revokes_other_sessions_and_takes_effect() {
    let Some(app) = common::setup().await else {
        return;
    };

    let (status, body) = register_user(
        &app.router,
        "owner@example.com",
        "old-pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let device1 = as_session(&body);

    // A second device for the same account.
    let (status, body) = login_user(
        &app.router,
        "owner@example.com",
        "old-pw",
        None,
        new_device("d2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let device2 = as_session(&body);

    // Device 1 changes the password.
    let (client_state, request_bytes) = opaque::client_register_start(b"new-pw").unwrap();
    let (status, body) = request(
        &app.router,
        "POST",
        "/v1/account/password/start",
        Some(&device1.session_token),
        Some(&AccountPasswordStart {
            registration_request: request_bytes,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let start: AccountPasswordStartResponse = serde_json::from_value(body).unwrap();

    let (upload_bytes, _export_key) =
        opaque::client_register_finish(client_state, b"new-pw", &start.registration_response)
            .unwrap();
    let (status, body) = request(
        &app.router,
        "POST",
        "/v1/account/password/finish",
        Some(&device1.session_token),
        Some(&AccountPasswordFinish {
            registration_id: start.registration_id,
            registration_upload: upload_bytes,
            enc_public: "enc-pub".to_string(),
            sign_public: "sign-pub".to_string(),
            kdf: "{\"new\":true}".to_string(),
            wrapped_account_keys: b"new-wrapped-account-keys".to_vec(),
            recovery_account_wrap: b"new-recovery-account-wrap".to_vec(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

    // Device 1's own session (the one that made the change) still works.
    let (status, _) = request::<()>(
        &app.router,
        "GET",
        "/v1/account/vaults",
        Some(&device1.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Device 2's session was revoked.
    let (status, _) = request::<()>(
        &app.router,
        "GET",
        "/v1/account/vaults",
        Some(&device2.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The old password no longer logs in...
    let (status, _) = login_user(
        &app.router,
        "owner@example.com",
        "old-pw",
        None,
        new_device("d3"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // ...but the new one does, and the account_keys row was updated too.
    let (status, body) = login_user(
        &app.router,
        "owner@example.com",
        "new-pw",
        None,
        new_device("d4"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);
    assert_eq!(session.account.kdf, "{\"new\":true}");
    assert_eq!(
        session.account.wrapped_account_keys,
        b"new-wrapped-account-keys"
    );
}

#[tokio::test]
async fn password_finish_rejects_unknown_or_foreign_registration_id() {
    let Some(app) = common::setup().await else {
        return;
    };

    let (status, body) = register_user(
        &app.router,
        "alice@example.com",
        "pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let alice = as_session(&body);

    let (status, body) =
        register_user(&app.router, "bob@example.com", "pw", None, new_device("d2")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let bob = as_session(&body);

    // Bob starts a password change...
    let (_client_state, request_bytes) = opaque::client_register_start(b"bob-new-pw").unwrap();
    let (status, body) = request(
        &app.router,
        "POST",
        "/v1/account/password/start",
        Some(&bob.session_token),
        Some(&AccountPasswordStart {
            registration_request: request_bytes,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let start: AccountPasswordStartResponse = serde_json::from_value(body).unwrap();

    // ...but Alice tries to finish it using her own session: refused, the
    // pending registration belongs to a different user.
    let (status, _) = request(
        &app.router,
        "POST",
        "/v1/account/password/finish",
        Some(&alice.session_token),
        Some(&AccountPasswordFinish {
            registration_id: start.registration_id,
            registration_upload: vec![0u8; 8],
            enc_public: "x".to_string(),
            sign_public: "y".to_string(),
            kdf: "{}".to_string(),
            wrapped_account_keys: b"whatever".to_vec(),
            recovery_account_wrap: Vec::new(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A made-up registration_id is refused too.
    let (status, _) = request(
        &app.router,
        "POST",
        "/v1/account/password/finish",
        Some(&bob.session_token),
        Some(&AccountPasswordFinish {
            registration_id: uuid::Uuid::new_v4().to_string(),
            registration_upload: vec![0u8; 8],
            enc_public: "x".to_string(),
            sign_public: "y".to_string(),
            kdf: "{}".to_string(),
            wrapped_account_keys: b"whatever".to_vec(),
            recovery_account_wrap: Vec::new(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn password_endpoints_require_auth() {
    let Some(app) = common::setup().await else {
        return;
    };
    let (status, _) = request(
        &app.router,
        "POST",
        "/v1/account/password/start",
        None,
        Some(&AccountPasswordStart {
            registration_request: vec![0u8; 8],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
