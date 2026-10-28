mod common;

use std::sync::Arc;

use ourshell_core::ssh::{
    connect, AcceptAllUnknown, Auth, ConnectOptions, MemoryKnownHosts, RejectAllUnknown, SshError,
    Target,
};

use common::{generate_key, TestServer};

fn password_target(server: &TestServer) -> Target {
    Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Password("pw".to_string()),
    }
}

#[tokio::test]
async fn password_auth_succeeds() {
    let server = TestServer::start(None).await;
    let targets = vec![password_target(&server)];

    let (session, accepted) = connect(
        &targets,
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect should succeed");

    assert_eq!(
        accepted,
        vec![(server.host_port(), server.host_key_openssh())]
    );
    session.disconnect().await.ok();
}

#[tokio::test]
async fn password_auth_fails_with_wrong_password() {
    let server = TestServer::start(None).await;
    let mut target = password_target(&server);
    target.auth = Auth::Password("not-the-password".to_string());

    let err = connect(
        &[target],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect_err("wrong password must be rejected");

    assert!(matches!(err, SshError::Auth));
}

#[tokio::test]
async fn key_auth_succeeds_for_the_registered_key() {
    let key = generate_key();
    let server = TestServer::start(Some(key.public.clone())).await;
    let target = Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Key {
            private_key_pem: key.private_pem.clone(),
            passphrase: None,
        },
    };

    let (session, _) = connect(
        &[target],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("key auth should succeed");
    session.disconnect().await.ok();
}

#[tokio::test]
async fn key_auth_fails_for_an_unregistered_key() {
    let registered = generate_key();
    let other = generate_key();
    let server = TestServer::start(Some(registered.public)).await;
    let target = Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Key {
            private_key_pem: other.private_pem,
            passphrase: None,
        },
    };

    let err = connect(
        &[target],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect_err("unregistered key must be rejected");
    assert!(matches!(err, SshError::Auth));
}

#[tokio::test]
async fn unknown_host_key_is_accepted_via_callback_and_reported() {
    let server = TestServer::start(None).await;
    let targets = vec![password_target(&server)];

    let (_, accepted) = connect(
        &targets,
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect should succeed once the callback accepts");

    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].0, server.host_port());
    assert_eq!(accepted[0].1, server.host_key_openssh());
}

#[tokio::test]
async fn unknown_host_key_declined_via_callback_is_refused() {
    let server = TestServer::start(None).await;
    let targets = vec![password_target(&server)];

    let err = connect(
        &targets,
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(RejectAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect_err("declining the unknown host key must refuse the connection");

    assert!(matches!(err, SshError::HostKeyRejected));
}

#[tokio::test]
async fn mismatched_host_key_is_always_refused() {
    let server = TestServer::start(None).await;
    let targets = vec![password_target(&server)];

    let mut known = MemoryKnownHosts::new();
    known.add(
        server.host_port(),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINotTheRealHostKeyAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );

    let err = connect(
        &targets,
        Arc::new(known),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect_err("a different recorded key must never be accepted, even via the callback");

    match err {
        SshError::HostKeyMismatch { host_port } => assert_eq!(host_port, server.host_port()),
        other => panic!("expected HostKeyMismatch, got {other:?}"),
    }
}
