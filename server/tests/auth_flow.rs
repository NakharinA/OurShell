mod common;

use axum::http::StatusCode;
use common::{as_session, login_user, new_device, register_user, request, setup, setup_with};
use ourshell_proto::sync::{
    CreateInviteRequest, CreateInviteResponse, TotpConfirm, TotpSetupResponse,
};

#[tokio::test]
async fn register_then_login_round_trip() {
    let Some(app) = setup().await else { return };
    let email = "alice@example.com";
    let password = "correct horse battery staple";

    let (status, body) =
        register_user(&app.router, email, password, None, new_device("laptop")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);
    assert!(!session.session_token.is_empty());
    assert_eq!(session.vaults.len(), 1);
    assert_eq!(session.vaults[0].kind, "personal");

    let (status, body) =
        login_user(&app.router, email, password, None, new_device("laptop-2")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let login_session = as_session(&body);
    assert_eq!(login_session.user_id, session.user_id);
    assert_eq!(login_session.org_id, session.org_id);
}

#[tokio::test]
async fn wrong_password_is_rejected() {
    let Some(app) = setup().await else { return };
    let email = "bob@example.com";
    let (status, body) =
        register_user(&app.router, email, "right-password", None, new_device("d1")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let (status, _) =
        login_user(&app.router, email, "wrong-password", None, new_device("d2")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invite_only_signup_requires_a_valid_unused_code() {
    let Some(app) = setup_with(|c| c.invite_only = true).await else {
        return;
    };

    // The very first user bootstraps the server regardless of invite_only
    // (nobody could have issued an invite yet) and becomes owner.
    let (status, body) = register_user(
        &app.router,
        "owner@example.com",
        "pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let owner = as_session(&body);

    // A second signup with no invite code is rejected.
    let (status, _) = register_user(
        &app.router,
        "nobody@example.com",
        "pw",
        None,
        new_device("d2"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Owner mints an invite.
    let (status, body) = request(
        &app.router,
        "POST",
        "/v1/orgs/invites",
        Some(&owner.session_token),
        Some(&CreateInviteRequest { role: None }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let invite: CreateInviteResponse = serde_json::from_value(body).unwrap();

    // Signing up with that invite succeeds.
    let (status, body) = register_user(
        &app.router,
        "invited@example.com",
        "pw",
        Some(&invite.code),
        new_device("d3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // The same (now-used) code is rejected for a further signup.
    let (status, _) = register_user(
        &app.router,
        "another@example.com",
        "pw",
        Some(&invite.code),
        new_device("d4"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn device_revoke_kills_its_session() {
    let Some(app) = setup().await else { return };
    let device = new_device("laptop");
    let (status, body) =
        register_user(&app.router, "carol@example.com", "pw", None, device.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);

    let (status, _) = request::<()>(
        &app.router,
        "GET",
        "/v1/devices",
        Some(&session.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request::<()>(
        &app.router,
        "POST",
        &format!("/v1/devices/{}/revoke", device.device_id),
        Some(&session.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request::<()>(
        &app.router,
        "GET",
        "/v1/devices",
        Some(&session.session_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked device's session must be dead"
    );
}

#[tokio::test]
async fn totp_flow_gates_login() {
    let Some(app) = setup().await else { return };
    let email = "dana@example.com";
    let password = "pw";
    let (status, body) = register_user(&app.router, email, password, None, new_device("d1")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let session = as_session(&body);

    let (status, body) = request::<()>(
        &app.router,
        "POST",
        "/v1/totp/setup",
        Some(&session.session_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let setup_resp: TotpSetupResponse = serde_json::from_value(body).unwrap();
    let secret = base32::decode(
        base32::Alphabet::Rfc4648 { padding: false },
        &setup_resp.secret_base32,
    )
    .expect("secret_base32 must decode");

    let gen_code = || {
        totp_rs::Builder::new()
            .with_secret(secret.clone())
            .build()
            .unwrap()
            .generate_current()
            .to_string()
    };

    let (status, _) = request(
        &app.router,
        "POST",
        "/v1/totp/confirm",
        Some(&session.session_token),
        Some(&TotpConfirm { code: gen_code() }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Login without a code is now rejected.
    let (status, _) = login_user(&app.router, email, password, None, new_device("d2")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Login with the current code succeeds.
    let (status, body) = login_user(
        &app.router,
        email,
        password,
        Some(&gen_code()),
        new_device("d2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let _ = as_session(&body);

    // Disabling requires a valid code too.
    let (status, _) = request(
        &app.router,
        "POST",
        "/v1/totp/disable",
        Some(&session.session_token),
        Some(&TotpConfirm { code: gen_code() }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Login no longer needs a code.
    let (status, _) = login_user(&app.router, email, password, None, new_device("d3")).await;
    assert_eq!(status, StatusCode::OK);
}
