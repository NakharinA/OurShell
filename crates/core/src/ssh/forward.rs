//! Local and remote TCP port forwarding.

use russh::client::{self, Msg};
use russh::Channel;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{ClientHandler, ForwardRegistry, Session, SshError};

/// Extra bookkeeping needed to tear a remote forward (`forward_remote`)
/// down cleanly: cancel the server-side listener and drop our local
/// routing entry. Local forwards (`forward_local`) don't need this since
/// there's nothing on the server side to cancel.
struct RemoteForwardCleanup {
    handle: std::sync::Arc<client::Handle<ClientHandler>>,
    forwards: ForwardRegistry,
    address: String,
    port: u32,
}

/// A running port forward started by [`Session::forward_local`] or
/// [`Session::forward_remote`].
///
/// Dropping it stops the forwarding task; for a remote forward, prefer
/// `.stop().await` so the server is also told to cancel the listener.
pub struct ForwardHandle {
    bound: String,
    task: JoinHandle<()>,
    cleanup: Option<RemoteForwardCleanup>,
}

impl ForwardHandle {
    /// The address this forward is bound to: a local `ip:port` for
    /// `forward_local`, or `bind_address:port` on the remote host for
    /// `forward_remote`.
    pub fn bound_addr(&self) -> &str {
        &self.bound
    }

    /// Stops accepting new connections and, for a remote forward, asks the
    /// server to cancel the listener.
    pub async fn stop(mut self) {
        self.task.abort();
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup
                .handle
                .cancel_tcpip_forward(cleanup.address.clone(), cleanup.port)
                .await;
            cleanup
                .forwards
                .lock()
                .await
                .remove(&(cleanup.address, cleanup.port));
        }
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Session {
    /// Listens on `bind_addr` locally; each accepted connection opens a
    /// `direct-tcpip` channel to `remote_host:remote_port` and copies data
    /// bidirectionally (classic `ssh -L`).
    pub async fn forward_local(
        &self,
        bind_addr: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> super::Result<ForwardHandle> {
        let listener = TcpListener::bind(bind_addr).await.map_err(SshError::Io)?;
        let bound = listener.local_addr().map_err(SshError::Io)?.to_string();

        let handle = self.handle.clone();
        let remote_host = remote_host.to_string();
        let task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let handle = handle.clone();
                let remote_host = remote_host.clone();
                tokio::spawn(async move {
                    let channel = match handle
                        .channel_open_direct_tcpip(
                            remote_host.as_str(),
                            remote_port as u32,
                            peer.ip().to_string(),
                            peer.port() as u32,
                        )
                        .await
                    {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    let mut ssh_stream = channel.into_stream();
                    let mut tcp = stream;
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut ssh_stream).await;
                });
            }
        });

        Ok(ForwardHandle {
            bound,
            task,
            cleanup: None,
        })
    }

    /// Asks the final hop's server to listen on `remote_bind:remote_port`
    /// (pass `remote_port = 0` to let the server pick) and, for every
    /// incoming connection it reports, connects to `local_host:local_port`
    /// and copies data bidirectionally (classic `ssh -R`).
    pub async fn forward_remote(
        &self,
        remote_bind: &str,
        remote_port: u16,
        local_host: &str,
        local_port: u16,
    ) -> super::Result<ForwardHandle> {
        let bound_port = self
            .handle
            .tcpip_forward(remote_bind, remote_port as u32)
            .await
            .map_err(SshError::from)?;
        let actual_port = if remote_port == 0 {
            bound_port
        } else {
            remote_port as u32
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<Channel<Msg>>();
        self.forwards
            .lock()
            .await
            .insert((remote_bind.to_string(), actual_port), tx);

        let local_host_owned = local_host.to_string();
        let task = tokio::spawn(async move {
            while let Some(channel) = rx.recv().await {
                let local_host = local_host_owned.clone();
                tokio::spawn(async move {
                    if let Ok(mut tcp) = TcpStream::connect((local_host.as_str(), local_port)).await
                    {
                        let mut ssh_stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut ssh_stream).await;
                    }
                });
            }
        });

        Ok(ForwardHandle {
            bound: format!("{remote_bind}:{actual_port}"),
            task,
            cleanup: Some(RemoteForwardCleanup {
                handle: self.handle.clone(),
                forwards: self.forwards.clone(),
                address: remote_bind.to_string(),
                port: actual_port,
            }),
        })
    }
}
