//! SSH client: sessions, PTY, exec, SFTP, forwarding, jump hosts, known hosts.
//!
//! This module is independent of [`crate::vault`]: it takes already-resolved
//! connection targets and credentials ([`Target`] / [`Auth`]) as input, and
//! leaves persistence (saving newly trusted host keys, reading identities)
//! to the caller.
//!
//! The entry point is [`connect`], which dials a chain of one or more
//! [`Target`]s (all but the last are jump hosts, reached via `direct-tcpip`
//! ProxyJump-style) and returns a [`Session`] plus the list of host keys
//! that were newly accepted during this connection (so the caller can
//! persist them).

pub mod forward;
pub mod known_hosts;
pub mod pty;
pub mod session;
pub mod sftp;

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use russh::client::{self, ChannelOpenHandle, Handler, Msg};
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure};
use tokio::sync::{mpsc, Mutex as TokioMutex};
use zeroize::Zeroize;

pub use forward::ForwardHandle;
pub use known_hosts::{
    host_port, AcceptAllUnknown, ChainVerifier, HostKeyDecision, HostKeyVerifier, KnownHostsFile,
    MemoryKnownHosts, OnUnknownHostKey, RejectAllUnknown, UnknownHostFuture,
};
pub use pty::{PtyEvent, PtySession};
pub use session::{ExecLimits, ExecOutput, Session};
pub use sftp::{DirEntry, Sftp};

/// One hop in a connection: either the final host, or (when more than one
/// `Target` is passed to [`connect`]) a jump host reached first.
#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
}

impl Target {
    pub fn host_port(&self) -> String {
        host_port(&self.host, self.port)
    }
}

/// How to authenticate a [`Target`].
#[derive(Clone)]
pub enum Auth {
    Password(String),
    Key {
        /// OpenSSH-format private key (PEM), as stored in the vault.
        private_key_pem: String,
        passphrase: Option<String>,
    },
    /// Use the system ssh-agent (OpenSSH agent on Unix via `SSH_AUTH_SOCK`,
    /// or the OpenSSH-for-Windows named pipe `\\.\pipe\openssh-ssh-agent`
    /// on Windows).
    Agent,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Password(_) => f.write_str("Auth::Password(<redacted>)"),
            Auth::Key { .. } => f.write_str("Auth::Key(<redacted>)"),
            Auth::Agent => f.write_str("Auth::Agent"),
        }
    }
}

impl Drop for Auth {
    fn drop(&mut self) {
        match self {
            Auth::Password(p) => p.zeroize(),
            Auth::Key {
                private_key_pem,
                passphrase,
            } => {
                private_key_pem.zeroize();
                if let Some(p) = passphrase {
                    p.zeroize();
                }
            }
            Auth::Agent => {}
        }
    }
}

/// Options for [`connect`].
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    /// Applies to the TCP connect and the SSH handshake+auth of *each* hop.
    pub connect_timeout: Duration,
    /// Ask each hop's server to allow ssh-agent forwarding over sessions
    /// opened on it (exec / PTY). Best-effort: forwards to the local
    /// OpenSSH agent socket/pipe if one is reachable.
    pub agent_forwarding: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            agent_forwarding: false,
        }
    }
}

/// Errors from the ssh module.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("authentication failed")]
    Auth,
    #[error("host key was not accepted")]
    HostKeyRejected,
    #[error("host key for {host_port} does not match the trusted key on record")]
    HostKeyMismatch { host_port: String },
    #[error("channel error: {0}")]
    Channel(String),
    #[error("sftp error: {0}")]
    Sftp(String),
    #[error("operation timed out")]
    Timeout,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ssh protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, SshError>;

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Protocol(e.to_string())
    }
}

/// `Forwarded-tcpip` channels (remote port forwarding) arriving on a
/// session are routed by `(bind address, bind port)` to whichever
/// `forward_remote` call registered that key.
pub(crate) type ForwardRegistry =
    Arc<TokioMutex<HashMap<(String, u32), mpsc::UnboundedSender<Channel<Msg>>>>>;

#[derive(Default)]
struct HostKeyOutcome {
    accepted_key: StdMutex<Option<String>>,
    rejection: StdMutex<Option<RejectReason>>,
}

enum RejectReason {
    Mismatch,
    Declined,
}

#[derive(Clone)]
pub(crate) struct ClientHandler {
    host_port: String,
    verifier: Arc<dyn HostKeyVerifier>,
    on_unknown: Arc<dyn OnUnknownHostKey>,
    outcome: Arc<HostKeyOutcome>,
    forwards: ForwardRegistry,
}

impl Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let key_openssh = server_public_key
            .public_key()
            .to_openssh()
            .unwrap_or_default();
        match self.verifier.check(&self.host_port, &key_openssh) {
            HostKeyDecision::Trusted => Ok(true),
            HostKeyDecision::Mismatch => {
                if let Ok(mut r) = self.outcome.rejection.lock() {
                    *r = Some(RejectReason::Mismatch);
                }
                Ok(false)
            }
            HostKeyDecision::Unknown => {
                let accept = self.on_unknown.decide(&self.host_port, &key_openssh).await;
                if accept {
                    if let Ok(mut k) = self.outcome.accepted_key.lock() {
                        *k = Some(key_openssh);
                    }
                    Ok(true)
                } else {
                    if let Ok(mut r) = self.outcome.rejection.lock() {
                        *r = Some(RejectReason::Declined);
                    }
                    Ok(false)
                }
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let key = (connected_address.to_string(), connected_port);
        let sender = self.forwards.lock().await.get(&key).cloned();
        match sender {
            Some(tx) => {
                reply.accept().await;
                let _ = tx.send(channel);
            }
            None => {
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
            }
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        reply.accept().await;
        tokio::spawn(pipe_agent_channel(channel));
        Ok(())
    }
}

/// Best-effort agent-forwarding backend: pipes a `auth-agent@openssh.com`
/// channel opened by a remote server through to the local OpenSSH agent
/// (Unix domain socket named by `SSH_AUTH_SOCK`, or the OpenSSH-for-Windows
/// named pipe). Silently does nothing if no local agent is reachable.
async fn pipe_agent_channel(channel: Channel<Msg>) {
    let mut ssh_stream = channel.into_stream();
    #[cfg(unix)]
    {
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            if let Ok(mut agent) = tokio::net::UnixStream::connect(sock).await {
                let _ = tokio::io::copy_bidirectional(&mut ssh_stream, &mut agent).await;
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut agent) = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(r"\\.\pipe\openssh-ssh-agent")
        {
            let _ = tokio::io::copy_bidirectional(&mut ssh_stream, &mut agent).await;
        }
    }
}

fn build_config(options: &ConnectOptions) -> client::Config {
    client::Config {
        inactivity_timeout: Some(options.connect_timeout),
        nodelay: true,
        ..Default::default()
    }
}

/// Connects through a chain of one or more [`Target`]s. All but the last
/// entry are jump hosts (ProxyJump): a `direct-tcpip` channel is opened on
/// each hop to reach the next, and a fresh SSH session is layered over that
/// channel's stream.
///
/// Returns the resulting [`Session`] (bound to the *last* hop) together
/// with the `(host_port, key_openssh)` pairs that were newly accepted via
/// `on_unknown` during this call, so the caller can persist them (e.g. to
/// the vault's `KnownHost` items).
pub async fn connect(
    targets: &[Target],
    verifier: Arc<dyn HostKeyVerifier>,
    on_unknown: Arc<dyn OnUnknownHostKey>,
    options: ConnectOptions,
) -> Result<(Session, Vec<(String, String)>)> {
    let Some((first, rest)) = targets.split_first() else {
        return Err(SshError::Connect("no targets given".into()));
    };

    let config = Arc::new(build_config(&options));
    let forwards: ForwardRegistry = Arc::new(TokioMutex::new(HashMap::new()));
    let mut accepted = Vec::new();

    let first_host_port = first.host_port();
    let outcome = Arc::new(HostKeyOutcome::default());
    let handler = ClientHandler {
        host_port: first_host_port.clone(),
        verifier: verifier.clone(),
        on_unknown: on_unknown.clone(),
        outcome: outcome.clone(),
        forwards: forwards.clone(),
    };

    let mut handle = connect_hop(
        client::connect(config.clone(), (first.host.as_str(), first.port), handler),
        options.connect_timeout,
        &outcome,
        &first_host_port,
    )
    .await?;
    authenticate(&mut handle, &first.user, &first.auth).await?;
    if let Some(k) = outcome.accepted_key.lock().ok().and_then(|mut g| g.take()) {
        accepted.push((first_host_port.clone(), k));
    }

    let mut jump_chain = Vec::new();
    let mut current_host_port = first_host_port;

    for next in rest {
        let next_host_port = host_port(&next.host, next.port);

        let channel = handle
            .channel_open_direct_tcpip(next.host.clone(), next.port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| SshError::Connect(format!("jump to {next_host_port} failed: {e}")))?;
        let stream = channel.into_stream();

        let outcome = Arc::new(HostKeyOutcome::default());
        let hop_handler = ClientHandler {
            host_port: next_host_port.clone(),
            verifier: verifier.clone(),
            on_unknown: on_unknown.clone(),
            outcome: outcome.clone(),
            forwards: forwards.clone(),
        };

        let mut inner = connect_hop(
            client::connect_stream(config.clone(), stream, hop_handler),
            options.connect_timeout,
            &outcome,
            &next_host_port,
        )
        .await?;
        authenticate(&mut inner, &next.user, &next.auth).await?;
        if let Some(k) = outcome.accepted_key.lock().ok().and_then(|mut g| g.take()) {
            accepted.push((next_host_port.clone(), k));
        }

        jump_chain.push(handle);
        handle = inner;
        current_host_port = next_host_port;
    }

    let session = Session::new(
        handle,
        jump_chain,
        forwards,
        current_host_port,
        options.agent_forwarding,
    );
    Ok((session, accepted))
}

/// Awaits a `client::connect`/`client::connect_stream` future under a
/// timeout, translating the generic "disconnected during kex" error that
/// russh raises when `check_server_key` returns `false` into a precise
/// [`SshError::HostKeyMismatch`] / [`SshError::HostKeyRejected`] using the
/// side-channel recorded in `outcome`.
async fn connect_hop<F>(
    fut: F,
    timeout: Duration,
    outcome: &HostKeyOutcome,
    host_port: &str,
) -> Result<client::Handle<ClientHandler>>
where
    F: std::future::Future<Output = std::result::Result<client::Handle<ClientHandler>, SshError>>,
{
    let result = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| SshError::Timeout)?;
    match result {
        Ok(handle) => Ok(handle),
        Err(err) => {
            let reason = outcome.rejection.lock().ok().and_then(|mut g| g.take());
            match reason {
                Some(RejectReason::Mismatch) => Err(SshError::HostKeyMismatch {
                    host_port: host_port.to_string(),
                }),
                Some(RejectReason::Declined) => Err(SshError::HostKeyRejected),
                None => Err(err),
            }
        }
    }
}

async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    user: &str,
    auth: &Auth,
) -> Result<()> {
    let success = match auth {
        Auth::Password(password) => handle
            .authenticate_password(user, password.clone())
            .await?
            .success(),
        Auth::Key {
            private_key_pem,
            passphrase,
        } => {
            let key = russh::keys::decode_secret_key(private_key_pem, passphrase.as_deref())
                .map_err(|_| SshError::Auth)?;
            let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
            handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg))
                .await?
                .success()
        }
        Auth::Agent => return authenticate_with_agent(handle, user).await,
    };
    if success {
        Ok(())
    } else {
        Err(SshError::Auth)
    }
}

async fn authenticate_with_agent(
    handle: &mut client::Handle<ClientHandler>,
    user: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        let mut agent = AgentClient::connect_env()
            .await
            .map_err(|e| SshError::Unsupported(format!("ssh-agent unavailable: {e}")))?;
        try_agent_identities(handle, user, &mut agent).await
    }
    #[cfg(windows)]
    {
        let mut agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .map_err(|e| SshError::Unsupported(format!("ssh-agent unavailable: {e}")))?;
        try_agent_identities(handle, user, &mut agent).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (handle, user);
        Err(SshError::Unsupported(
            "ssh-agent auth is only supported on Unix and Windows".into(),
        ))
    }
}

/// Tries each identity the agent offers, in order, until one authenticates
/// or the list is exhausted.
#[cfg(any(unix, windows))]
async fn try_agent_identities<S>(
    handle: &mut client::Handle<ClientHandler>,
    user: &str,
    agent: &mut AgentClient<S>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|_| SshError::Auth)?;
    for identity in identities {
        let public_key = identity.public_key().into_owned();
        if let Ok(result) = handle
            .authenticate_publickey_with(user, public_key, None, agent)
            .await
        {
            if result.success() {
                return Ok(());
            }
        }
    }
    Err(SshError::Auth)
}
