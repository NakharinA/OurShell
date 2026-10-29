//! The [`Session`] handle returned by [`crate::ssh::connect`], and the
//! plain `exec` (run one command, collect output) API.

use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::ChannelMsg;

use super::{ClientHandler, ForwardRegistry, SshError};

/// Limits applied to [`Session::exec`].
#[derive(Clone, Copy, Debug)]
pub struct ExecLimits {
    /// `None` disables the timeout.
    pub timeout: Option<Duration>,
    /// Combined cap on bytes collected for stdout (and, separately,
    /// stderr); once hit, further output is dropped and `truncated` is set.
    pub max_output_bytes: usize,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
            max_output_bytes: 1 << 20, // 1 MiB
        }
    }
}

/// The result of [`Session::exec`].
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<u32>,
    /// `true` if stdout or stderr hit `max_output_bytes` and was cut short.
    pub truncated: bool,
}

/// A connected SSH session (the last hop of a possibly multi-hop
/// [`crate::ssh::connect`] call).
///
/// Cheap to clone: every clone shares the same underlying connection, so a
/// `Session` can be handed to multiple tasks (e.g. one per PTY/exec/SFTP
/// user) without re-authenticating.
#[derive(Clone)]
pub struct Session {
    pub(crate) handle: Arc<client::Handle<ClientHandler>>,
    /// Earlier hops in a jump chain, kept alive only so the nested
    /// connections they carry aren't torn down; never used directly.
    #[allow(dead_code)]
    pub(crate) jump_chain: Arc<Vec<client::Handle<ClientHandler>>>,
    pub(crate) forwards: ForwardRegistry,
    pub(crate) host_port: String,
    /// Whether [`crate::ssh::ConnectOptions::agent_forwarding`] was
    /// requested for this connection; consulted by [`Session::open_pty`]
    /// and [`Session::exec`] to decide whether to ask the server for an
    /// agent-forwarding channel.
    pub(crate) agent_forwarding: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("host_port", &self.host_port)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn new(
        handle: client::Handle<ClientHandler>,
        jump_chain: Vec<client::Handle<ClientHandler>>,
        forwards: ForwardRegistry,
        host_port: String,
        agent_forwarding: bool,
    ) -> Self {
        Self {
            handle: Arc::new(handle),
            jump_chain: Arc::new(jump_chain),
            forwards,
            host_port,
            agent_forwarding,
        }
    }

    /// `"host:port"` of the final hop this session is connected to.
    pub fn host_port(&self) -> &str {
        &self.host_port
    }

    /// Runs one command and collects its output. Use [`Session::open_pty`]
    /// for interactive sessions.
    pub async fn exec(&self, command: &str, limits: ExecLimits) -> super::Result<ExecOutput> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::from)?;
        if self.agent_forwarding {
            let _ = channel.agent_forward(true).await;
        }
        channel.exec(true, command).await.map_err(SshError::from)?;

        let mut out = ExecOutput::default();
        let collect = async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        push_capped(
                            &mut out.stdout,
                            &data,
                            limits.max_output_bytes,
                            &mut out.truncated,
                        );
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        push_capped(
                            &mut out.stderr,
                            &data,
                            limits.max_output_bytes,
                            &mut out.truncated,
                        );
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        out.exit_code = Some(exit_status);
                    }
                    // OpenSSH sends EOF *before* exit-status, so keep reading
                    // until the channel actually closes.
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        };

        match limits.timeout {
            Some(d) => {
                tokio::time::timeout(d, collect)
                    .await
                    .map_err(|_| SshError::Timeout)?;
            }
            None => collect.await,
        }

        Ok(out)
    }

    /// Sends a disconnect to the final hop. Earlier jump hops (if any) are
    /// torn down when this `Session` (and all its clones) are dropped.
    pub async fn disconnect(&self) -> super::Result<()> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .map_err(SshError::from)
    }
}

fn push_capped(buf: &mut Vec<u8>, data: &[u8], cap: usize, truncated: &mut bool) {
    if buf.len() >= cap {
        if !data.is_empty() {
            *truncated = true;
        }
        return;
    }
    let remaining = cap - buf.len();
    if data.len() > remaining {
        buf.extend_from_slice(&data[..remaining]);
        *truncated = true;
    } else {
        buf.extend_from_slice(data);
    }
}
