mod common;

use std::sync::Arc;

use ourshell_core::ssh::{
    connect, AcceptAllUnknown, Auth, ConnectOptions, MemoryKnownHosts, Target,
};

use common::TestServer;

async fn connected_sftp(server: &TestServer) -> ourshell_core::ssh::Sftp {
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
    session.sftp().await.expect("open sftp subsystem")
}

#[tokio::test]
async fn sftp_write_then_read_round_trips() {
    let server = TestServer::start(None).await;
    let sftp = connected_sftp(&server).await;

    sftp.write_file("hello.txt", b"hello sftp")
        .await
        .expect("write_file");

    let data = sftp.read_file("hello.txt", 1024).await.expect("read_file");
    assert_eq!(data, b"hello sftp");

    // It really landed on disk, not just round-tripped through some cache.
    let on_disk = std::fs::read(server.sftp_root.path().join("hello.txt")).unwrap();
    assert_eq!(on_disk, b"hello sftp");
}

#[tokio::test]
async fn sftp_read_file_respects_max_bytes() {
    let server = TestServer::start(None).await;
    let sftp = connected_sftp(&server).await;

    sftp.write_file("big.txt", &vec![b'x'; 1000])
        .await
        .expect("write_file");

    let data = sftp.read_file("big.txt", 10).await.expect("read_file");
    assert_eq!(data.len(), 10);
}

#[tokio::test]
async fn sftp_list_dir_sees_written_files() {
    let server = TestServer::start(None).await;
    let sftp = connected_sftp(&server).await;

    sftp.write_file("a.txt", b"a").await.expect("write a");
    sftp.write_file("b.txt", b"bb").await.expect("write b");

    let entries = sftp.list_dir(".").await.expect("list_dir");
    let a = entries
        .iter()
        .find(|e| e.name == "a.txt")
        .expect("a.txt listed");
    assert!(!a.is_dir);
    assert_eq!(a.size, 1);

    let b = entries
        .iter()
        .find(|e| e.name == "b.txt")
        .expect("b.txt listed");
    assert_eq!(b.size, 2);
}

#[tokio::test]
async fn sftp_upload_and_download_round_trip_through_local_files() {
    let server = TestServer::start(None).await;
    let sftp = connected_sftp(&server).await;

    let local_dir = tempfile::tempdir().expect("local tempdir");
    let upload_src = local_dir.path().join("upload.txt");
    std::fs::write(&upload_src, b"upload me").unwrap();

    sftp.upload(&upload_src, "uploaded.txt")
        .await
        .expect("upload");
    assert_eq!(
        std::fs::read(server.sftp_root.path().join("uploaded.txt")).unwrap(),
        b"upload me"
    );

    let download_dst = local_dir.path().join("download.txt");
    sftp.download("uploaded.txt", &download_dst)
        .await
        .expect("download");
    assert_eq!(std::fs::read(&download_dst).unwrap(), b"upload me");
}
