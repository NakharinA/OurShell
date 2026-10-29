//! Shared test fixtures: an in-process SSH server (`TestServer`) used by the
//! `ssh_*` integration tests, plus a couple of small helpers.
//!
//! This is not itself a test binary (it's `tests/common/mod.rs`, which
//! Cargo's test harness skips); each `ssh_*.rs` test includes it with
//! `mod common;`.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Config, Handle, Msg, Server as RusshServer, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure, Pty, Sig};
use russh_sftp::protocol::{
    Attrs, Data, File as SftpFile, FileAttributes, Handle as SftpHandle, Name, OpenFlags, Status,
    StatusCode, Version,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A freshly generated Ed25519 key pair, in the shapes `ourshell_core::ssh`
/// and the test server need.
pub struct GeneratedKey {
    pub private_pem: String,
    pub public: PublicKey,
}

pub fn generate_key() -> GeneratedKey {
    let private = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .expect("generate test ed25519 key");
    let private_pem = private
        .to_openssh(LineEnding::LF)
        .expect("encode test key")
        .to_string();
    let public = private.public_key().clone();
    GeneratedKey {
        private_pem,
        public,
    }
}

/// A running in-process SSH server for tests. Dropping it stops the server.
pub struct TestServer {
    pub addr: SocketAddr,
    pub host_public_key: PublicKey,
    pub sftp_root: tempfile::TempDir,
    _task: JoinHandle<()>,
}

impl TestServer {
    /// Starts a server on `127.0.0.1` (OS-chosen port). Accepts password
    /// auth `test`/`pw`, and public-key auth for `accepted_client_key` (if
    /// given).
    pub async fn start(accepted_client_key: Option<PublicKey>) -> Self {
        let sftp_root = tempfile::tempdir().expect("tempdir");
        let host_key =
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("generate host key");
        let host_public_key = host_key.public_key().clone();

        let config = std::sync::Arc::new(Config {
            keys: vec![host_key],
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        let mut server = ServerImpl {
            accepted_client_key,
            sftp_root: sftp_root.path().to_path_buf(),
        };

        let task = tokio::spawn(async move {
            let running = server.run_on_socket(config, &listener);
            let _ = running.await;
        });

        Self {
            addr,
            host_public_key,
            sftp_root,
            _task: task,
        }
    }

    pub fn host_key_openssh(&self) -> String {
        self.host_public_key.to_openssh().expect("encode host key")
    }

    pub fn host_port(&self) -> String {
        format!("{}:{}", self.addr.ip(), self.addr.port())
    }
}

#[derive(Clone)]
struct ServerImpl {
    accepted_client_key: Option<PublicKey>,
    sftp_root: PathBuf,
}

impl RusshServer for ServerImpl {
    type Handler = ConnHandler;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        ConnHandler {
            accepted_client_key: self.accepted_client_key.clone(),
            sftp_root: self.sftp_root.clone(),
            channels: HashMap::new(),
            pty_channels: std::collections::HashSet::new(),
        }
    }
}

struct ConnHandler {
    accepted_client_key: Option<PublicKey>,
    sftp_root: PathBuf,
    /// Session channels, kept around only long enough for a possible
    /// `subsystem_request("sftp")` to claim one.
    channels: HashMap<ChannelId, Channel<Msg>>,
    pty_channels: std::collections::HashSet<ChannelId>,
}

impl russh::server::Handler for ConnHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == "test" && password == "pw" {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        match &self.accepted_client_key {
            Some(k) if k == key.public_key => Ok(Auth::Accept),
            _ => Ok(Auth::reject()),
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        match TcpStream::connect((host_to_connect, port_to_connect as u16)).await {
            Ok(tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut ssh_stream = channel.into_stream();
                    let mut tcp = tcp;
                    let _ = tokio::io::copy_bidirectional(&mut ssh_stream, &mut tcp).await;
                });
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pty_channels.insert(channel);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.pty_channels.contains(&channel) {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

    async fn signal(
        &mut self,
        _channel: ChannelId,
        _signal: Sig,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        session.channel_success(channel)?;
        let handle = session.handle();
        tokio::spawn(run_fake_command(cmd, channel, handle));
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            if let Some(chan) = self.channels.remove(&channel) {
                session.channel_success(channel)?;
                let root = self.sftp_root.clone();
                tokio::spawn(async move {
                    let handler = TestSftpHandler::new(root);
                    let _ = russh_sftp::server::run(chan.into_stream(), handler).await;
                });
                return Ok(());
            }
        }
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let bind = format!("{address}:{port}");
        let listener = match TcpListener::bind(&bind).await {
            Ok(l) => l,
            Err(_) => return Ok(false),
        };
        let bound_port = listener
            .local_addr()
            .map(|a| a.port() as u32)
            .unwrap_or(*port);
        *port = bound_port;

        let handle = session.handle();
        let address_owned = address.to_string();
        tokio::spawn(async move {
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let handle = handle.clone();
                let address = address_owned.clone();
                tokio::spawn(async move {
                    if let Ok(channel) = handle
                        .channel_open_forwarded_tcpip(
                            address,
                            bound_port,
                            peer.ip().to_string(),
                            peer.port() as u32,
                        )
                        .await
                    {
                        let mut ssh_stream = channel.into_stream();
                        let mut tcp = tcp;
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut ssh_stream).await;
                    }
                });
            }
        });
        Ok(true)
    }
}

/// A tiny fake shell for `exec` requests:
/// - `echo X` prints `X\n` to stdout and exits 0
/// - `exit N` exits with status `N`, no output
/// - `stderr-msg` prints to stderr and exits 1
/// - `big` prints 64 KiB of `A` to stdout, for truncation tests
/// - a command starting with `kubectl` echoes it back as `ok: <cmd>`, so a
///   test can assert on the exact command the agent built
/// - a command starting with `uptime` (the fixed `system_status` command)
///   prints canned `uptime`/`df`/`free`-shaped output
/// - anything else prints "unknown command" to stderr and exits 127
async fn run_fake_command(cmd: String, channel: ChannelId, handle: Handle) {
    let (status, stdout, stderr): (u32, Vec<u8>, Vec<u8>) =
        if let Some(rest) = cmd.strip_prefix("echo ") {
            (0, format!("{rest}\n").into_bytes(), Vec::new())
        } else if let Some(rest) = cmd.strip_prefix("exit ") {
            (rest.trim().parse().unwrap_or(0), Vec::new(), Vec::new())
        } else if cmd == "stderr-msg" {
            (1, Vec::new(), b"stderr message\n".to_vec())
        } else if cmd == "big" {
            (0, vec![b'A'; 64 * 1024], Vec::new())
        } else if cmd.starts_with("kubectl") {
            (0, format!("ok: {cmd}\n").into_bytes(), Vec::new())
        } else if cmd.starts_with("uptime") {
            (
                0,
                b" 12:00:00 up 1 day,  2:34,  1 user,  load average: 0.01, 0.02, 0.03\n\
                   Filesystem      Size  Used Avail Use% Mounted on\n\
                   /dev/sda1        20G   5G   15G  25% /\n\
                   total        used        free\n\
                   Mem:          2048         512        1536\n"
                    .to_vec(),
                Vec::new(),
            )
        } else {
            (
                127,
                Vec::new(),
                format!("unknown command: {cmd}\n").into_bytes(),
            )
        };

    if !stdout.is_empty() {
        let _ = handle.data(channel, stdout).await;
    }
    if !stderr.is_empty() {
        let _ = handle.extended_data(channel, 1, stderr).await;
    }
    // Same order as OpenSSH's sshd: EOF first, then exit-status, then close.
    let _ = handle.eof(channel).await;
    let _ = handle.exit_status_request(channel, status).await;
    let _ = handle.close(channel).await;
}

/// A filesystem-backed SFTP server `Handler`, rooted at a temp directory.
struct TestSftpHandler {
    root: PathBuf,
    next_handle: AtomicU64,
    files: HashMap<String, tokio::fs::File>,
    dirs: HashMap<String, VecDeque<(String, FileAttributes)>>,
}

impl TestSftpHandler {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            next_handle: AtomicU64::new(0),
            files: HashMap::new(),
            dirs: HashMap::new(),
        }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() || trimmed == "." {
            self.root.clone()
        } else {
            self.root.join(trimmed)
        }
    }

    fn new_handle(&self) -> String {
        self.next_handle.fetch_add(1, Ordering::SeqCst).to_string()
    }
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

impl russh_sftp::server::Handler for TestSftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let shown = if path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        Ok(Name {
            id,
            files: vec![SftpFile::dummy(&shown)],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<SftpHandle, Self::Error> {
        let dir_path = self.resolve(&path);
        let mut entries = VecDeque::new();
        let mut rd = tokio::fs::read_dir(&dir_path)
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;
        loop {
            match rd.next_entry().await {
                Ok(Some(entry)) => {
                    let meta = entry.metadata().await.map_err(|_| StatusCode::Failure)?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    entries.push_back((name, FileAttributes::from(&meta)));
                }
                Ok(None) => break,
                Err(_) => return Err(StatusCode::Failure),
            }
        }
        let handle = self.new_handle();
        self.dirs.insert(handle.clone(), entries);
        Ok(SftpHandle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let entries = self.dirs.get_mut(&handle).ok_or(StatusCode::Failure)?;
        if entries.is_empty() {
            return Err(StatusCode::Eof);
        }
        let files = entries
            .drain(..)
            .map(|(name, attrs)| SftpFile::new(name, attrs))
            .collect();
        Ok(Name { id, files })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        self.dirs.remove(&handle);
        Ok(ok_status(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<SftpHandle, Self::Error> {
        let path = self.resolve(&filename);
        let std_opts: std::fs::OpenOptions = pflags.into();
        let file = tokio::fs::OpenOptions::from(std_opts)
            .open(&path)
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;
        let handle = self.new_handle();
        self.files.insert(handle.clone(), file);
        Ok(SftpHandle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::Failure)?;
        let mut buf = vec![0u8; len as usize];
        let n = file.read(&mut buf).await.map_err(|_| StatusCode::Failure)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::Failure)?;
        file.write_all(&data)
            .await
            .map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = tokio::fs::metadata(self.resolve(&path))
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        tokio::fs::create_dir_all(self.resolve(&path))
            .await
            .map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        tokio::fs::remove_file(self.resolve(&filename))
            .await
            .map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }
}

/// A plain TCP echo server, used as the destination for local-port-forward
/// tests (so the test doesn't depend on any real external service).
pub struct EchoServer {
    pub addr: SocketAddr,
    _task: JoinHandle<()>,
}

impl EchoServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    loop {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if stream.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        Self { addr, _task: task }
    }
}
