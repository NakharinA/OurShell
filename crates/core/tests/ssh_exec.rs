mod common;

use std::sync::Arc;

use ourshell_core::ssh::{
    connect, AcceptAllUnknown, Auth, ConnectOptions, ExecLimits, MemoryKnownHosts, Session, Target,
};

use common::TestServer;

async fn connected_session(server: &TestServer) -> Session {
    let target = Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Password("pw".to_string()),
    };
    let (session, _) = connect(
        &[target],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect");
    session
}

#[tokio::test]
async fn exec_echo_returns_stdout_and_exit_code() {
    let server = TestServer::start(None).await;
    let session = connected_session(&server).await;

    let out = session
        .exec("echo hello", ExecLimits::default())
        .await
        .expect("exec");

    assert_eq!(out.stdout, b"hello\n");
    assert_eq!(out.stderr, b"");
    assert_eq!(out.exit_code, Some(0));
    assert!(!out.truncated);
}

#[tokio::test]
async fn exec_reports_nonzero_exit_code() {
    let server = TestServer::start(None).await;
    let session = connected_session(&server).await;

    let out = session
        .exec("exit 7", ExecLimits::default())
        .await
        .expect("exec");

    assert_eq!(out.exit_code, Some(7));
    assert!(out.stdout.is_empty());
}

#[tokio::test]
async fn exec_captures_stderr_separately_from_stdout() {
    let server = TestServer::start(None).await;
    let session = connected_session(&server).await;

    let out = session
        .exec("stderr-msg", ExecLimits::default())
        .await
        .expect("exec");

    assert_eq!(out.stderr, b"stderr message\n");
    assert!(out.stdout.is_empty());
    assert_eq!(out.exit_code, Some(1));
}

#[tokio::test]
async fn exec_truncates_output_past_the_configured_limit() {
    let server = TestServer::start(None).await;
    let session = connected_session(&server).await;

    let limits = ExecLimits {
        max_output_bytes: 100,
        ..ExecLimits::default()
    };
    let out = session.exec("big", limits).await.expect("exec");

    assert_eq!(out.stdout.len(), 100);
    assert!(out.truncated);
    assert_eq!(out.exit_code, Some(0));
}

#[tokio::test]
async fn exec_unknown_command_reports_stderr_and_127() {
    let server = TestServer::start(None).await;
    let session = connected_session(&server).await;

    let out = session
        .exec("this-is-not-a-real-command", ExecLimits::default())
        .await
        .expect("exec");

    assert_eq!(out.exit_code, Some(127));
    assert!(!out.stderr.is_empty());
}
