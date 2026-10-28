mod common;

use std::sync::Arc;
use std::time::Duration;

use ourshell_core::ssh::{
    connect, AcceptAllUnknown, Auth, ConnectOptions, ExecLimits, MemoryKnownHosts, Target,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use common::{EchoServer, TestServer};

fn password_target(server: &TestServer) -> Target {
    Target {
        host: server.addr.ip().to_string(),
        port: server.addr.port(),
        user: "test".to_string(),
        auth: Auth::Password("pw".to_string()),
    }
}

#[tokio::test]
async fn local_port_forward_reaches_the_remote_service() {
    let server = TestServer::start(None).await;
    let echo = EchoServer::start().await;

    let (session, _) = connect(
        &[password_target(&server)],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect");

    let forward = session
        .forward_local("127.0.0.1:0", &echo.addr.ip().to_string(), echo.addr.port())
        .await
        .expect("forward_local");

    let mut client = TcpStream::connect(forward.bound_addr())
        .await
        .expect("connect to local forward");
    client.write_all(b"ping through the tunnel").await.unwrap();

    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("read timed out")
        .expect("read");

    assert_eq!(&buf[..n], b"ping through the tunnel");
    forward.stop().await;
}

#[tokio::test]
async fn remote_port_forward_reaches_a_local_service() {
    let server = TestServer::start(None).await;
    let echo = EchoServer::start().await;

    let (session, _) = connect(
        &[password_target(&server)],
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect");

    let forward = session
        .forward_remote(
            "127.0.0.1",
            0,
            &echo.addr.ip().to_string(),
            echo.addr.port(),
        )
        .await
        .expect("forward_remote");

    // The fixture server runs in this same process on 127.0.0.1, so we can
    // dial the port it just started listening on directly, exactly as a
    // third party reaching the "remote" forwarded port would.
    let mut client = TcpStream::connect(forward.bound_addr())
        .await
        .expect("connect to remote forward");
    client.write_all(b"ping via remote forward").await.unwrap();

    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("read timed out")
        .expect("read");

    assert_eq!(&buf[..n], b"ping via remote forward");
    forward.stop().await;
}

#[tokio::test]
async fn jump_host_chain_reaches_the_final_host() {
    let hop1 = TestServer::start(None).await;
    let hop2 = TestServer::start(None).await;

    let targets = vec![password_target(&hop1), password_target(&hop2)];

    let (session, accepted) = connect(
        &targets,
        Arc::new(MemoryKnownHosts::new()),
        Arc::new(AcceptAllUnknown),
        ConnectOptions::default(),
    )
    .await
    .expect("connect through the jump host");

    // One accepted host key per hop.
    assert_eq!(accepted.len(), 2);
    assert_eq!(session.host_port(), hop2.host_port());

    let out = session
        .exec("echo through-the-jump", ExecLimits::default())
        .await
        .expect("exec on the final hop");
    assert_eq!(out.stdout, b"through-the-jump\n");
}
