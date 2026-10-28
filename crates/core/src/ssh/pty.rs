//! Interactive PTY sessions.

use std::sync::Arc;

use russh::client::Msg;
use russh::{ChannelMsg, ChannelWriteHalf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{Session, SshError};

/// One event from an interactive PTY session, delivered on
/// [`PtySession::events`].
#[derive(Debug)]
pub enum PtyEvent {
    /// Bytes from the remote process's stdout (or combined pty output).
    Data(Vec<u8>),
    /// Bytes from an extended data stream (usually stderr).
    ExtendedData(Vec<u8>),
    /// The remote process exited with this status, if reported.
    Exit(Option<u32>),
    /// The channel was closed; no more events will follow.
    Closed,
}

/// A live interactive PTY session.
///
/// The write half (`write`/`resize`/`close`) is cheap to clone (it is an
/// `Arc` under the hood via [`PtySession::writer`]) so it can be handed to
/// a different task than the one draining `events`, which is how an agent
/// that multiplexes many sessions would typically use this.
pub struct PtySession {
    write: Arc<ChannelWriteHalf<Msg>>,
    /// Incoming data/exit/close events. Take ownership with
    /// `std::mem::replace` or just read from `&mut session.events` if you
    /// don't need to move it into another task.
    pub events: mpsc::Receiver<PtyEvent>,
    reader_task: JoinHandle<()>,
}

impl PtySession {
    /// A cheaply cloneable handle for writing to / resizing / closing this
    /// PTY from any task.
    pub fn writer(&self) -> PtyWriter {
        PtyWriter {
            write: self.write.clone(),
        }
    }

    pub async fn write(&self, data: &[u8]) -> super::Result<()> {
        self.write
            .data_bytes(data.to_vec())
            .await
            .map_err(SshError::from)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> super::Result<()> {
        self.write
            .window_change(cols, rows, 0, 0)
            .await
            .map_err(SshError::from)
    }

    pub async fn close(&self) -> super::Result<()> {
        let _ = self.write.eof().await;
        self.write.close().await.map_err(SshError::from)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

/// A cheap, `Send + Clone` handle to the write side of a [`PtySession`].
#[derive(Clone)]
pub struct PtyWriter {
    write: Arc<ChannelWriteHalf<Msg>>,
}

impl PtyWriter {
    pub async fn write(&self, data: &[u8]) -> super::Result<()> {
        self.write
            .data_bytes(data.to_vec())
            .await
            .map_err(SshError::from)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> super::Result<()> {
        self.write
            .window_change(cols, rows, 0, 0)
            .await
            .map_err(SshError::from)
    }

    pub async fn close(&self) -> super::Result<()> {
        let _ = self.write.eof().await;
        self.write.close().await.map_err(SshError::from)
    }
}

impl Session {
    /// Opens an interactive PTY session (pty-req + shell) with the given
    /// terminal type and initial size.
    pub async fn open_pty(&self, term: &str, cols: u32, rows: u32) -> super::Result<PtySession> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::from)?;
        if self.agent_forwarding {
            let _ = channel.agent_forward(true).await;
        }
        channel
            .request_pty(true, term, cols, rows, 0, 0, &[])
            .await
            .map_err(SshError::from)?;
        channel.request_shell(true).await.map_err(SshError::from)?;

        let (mut read_half, write_half) = channel.split();
        let (tx, rx) = mpsc::channel(256);
        let reader_task = tokio::spawn(async move {
            loop {
                match read_half.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        if tx.send(PtyEvent::Data(data.to_vec())).await.is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if tx
                            .send(PtyEvent::ExtendedData(data.to_vec()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        if tx.send(PtyEvent::Exit(Some(exit_status))).await.is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        let _ = tx.send(PtyEvent::Closed).await;
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(PtySession {
            write: Arc::new(write_half),
            events: rx,
            reader_task,
        })
    }
}
