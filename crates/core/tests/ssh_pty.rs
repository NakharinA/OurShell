mod common;

use std::sync::Arc;
use std::time::Duration;

use ourshell_core::ssh::{
    connect, AcceptAllUnknown, Auth, ConnectOptions, MemoryKnownHosts, PtyEvent, Target,
};

use common::TestServer;

#[tokio::test]
async fn pty_echoes_input_and_accepts_resize() {
    let server = TestServer::start(None).await;
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

    let mut pty = session.open_pty("xterm", 80, 24).await.expect("open_pty");

    // Resizing must not error (the test fixture's fake shell has nothing
    // special to say about it, but the request itself must round-trip
    // cleanly over the channel).
    pty.resize(100, 40).await.expect("resize");

    pty.write(b"hello\n").await.expect("write");

    let mut got = Vec::new();
    while got.len() < b"hello\n".len() {
        match tokio::time::timeout(Duration::from_secs(5), pty.events.recv()).await {
            Ok(Some(PtyEvent::Data(data))) => got.extend_from_slice(&data),
            Ok(Some(_)) => {}
            Ok(None) => panic!("pty closed before echoing all input"),
            Err(_) => panic!("timed out waiting for the pty to echo input"),
        }
    }

    assert_eq!(got, b"hello\n");
    pty.close().await.ok();
}
