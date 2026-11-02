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

/// Regression: an idle shell must stay open. The client used to reuse
/// `connect_timeout` as russh's `inactivity_timeout`, so a PTY with no
/// traffic was disconnected (`[process exited]`) once that timeout passed.
#[tokio::test]
async fn idle_pty_outlives_connect_timeout() {
    let server = TestServer::start(None).await;
    let target = Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Password("pw".to_string()),
    };
    let connect_timeout = Duration::from_secs(1);
    let (session, _) = connect(
        &[target],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions {
            connect_timeout,
            ..ConnectOptions::default()
        },
    )
    .await
    .expect("connect");

    let mut pty = session.open_pty("xterm", 80, 24).await.expect("open_pty");

    // Stay idle for three times the connect timeout, failing if the session
    // closes meanwhile.
    let idle = tokio::time::sleep(connect_timeout * 3);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = &mut idle => break,
            ev = pty.events.recv() => match ev {
                Some(PtyEvent::Data(_)) => {}
                Some(other) => panic!("idle pty got {other:?} instead of staying open"),
                None => panic!("idle pty was closed"),
            },
        }
    }

    pty.write(b"still here\n").await.expect("write after idle");
    let mut got = Vec::new();
    while got.len() < b"still here\n".len() {
        match tokio::time::timeout(Duration::from_secs(5), pty.events.recv()).await {
            Ok(Some(PtyEvent::Data(data))) => got.extend_from_slice(&data),
            Ok(Some(_)) => {}
            Ok(None) => panic!("pty closed before echoing input after idle"),
            Err(_) => panic!("timed out waiting for echo after idle"),
        }
    }
    assert_eq!(got, b"still here\n");
    pty.close().await.ok();
}
