//! SFTP: directory listing, small read/write helpers, and file transfer.

use std::path::Path;
use std::time::SystemTime;

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::AsyncWriteExt;

use super::{Session, SshError};

/// One entry returned by [`Sftp::list_dir`].
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// An open SFTP subsystem channel.
pub struct Sftp {
    inner: SftpSession,
}

fn sftp_err(e: impl std::fmt::Display) -> SshError {
    SshError::Sftp(e.to_string())
}

impl Sftp {
    pub async fn list_dir(&self, path: &str) -> super::Result<Vec<DirEntry>> {
        let entries = self.inner.read_dir(path).await.map_err(sftp_err)?;
        Ok(entries
            .map(|entry| {
                let meta = entry.metadata();
                DirEntry {
                    name: entry.file_name(),
                    is_dir: meta.is_dir(),
                    size: meta.len(),
                    modified: meta.modified().ok(),
                }
            })
            .collect())
    }

    /// Reads up to `max_bytes` of a remote file. If the file is larger, the
    /// result is silently truncated to `max_bytes`.
    pub async fn read_file(&self, path: &str, max_bytes: usize) -> super::Result<Vec<u8>> {
        let mut data = self.inner.read(path).await.map_err(sftp_err)?;
        if data.len() > max_bytes {
            data.truncate(max_bytes);
        }
        Ok(data)
    }

    /// Writes (creating or overwriting) a remote file in one shot.
    ///
    /// Note: this deliberately opens with `CREATE | TRUNCATE | WRITE`
    /// rather than using `SftpSession::write`'s own convenience method,
    /// which opens with `WRITE` only and so fails on files that don't
    /// already exist.
    pub async fn write_file(&self, path: &str, bytes: &[u8]) -> super::Result<()> {
        let mut file = self
            .inner
            .open_with_flags(
                path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(sftp_err)?;
        file.write_all(bytes).await.map_err(SshError::Io)?;
        // Wait for the server to acknowledge every write before closing, so a
        // failed write (disk full, permissions) is reported instead of lost.
        file.flush().await.map_err(SshError::Io)?;
        file.close().await.map_err(sftp_err)
    }

    /// Downloads a remote file to a local path, overwriting it if present.
    pub async fn download(&self, remote: &str, local_path: &Path) -> super::Result<()> {
        let data = self.inner.read(remote).await.map_err(sftp_err)?;
        tokio::fs::write(local_path, data)
            .await
            .map_err(SshError::Io)
    }

    /// Uploads a local file to a remote path, creating or overwriting it.
    pub async fn upload(&self, local_path: &Path, remote: &str) -> super::Result<()> {
        let data = tokio::fs::read(local_path).await.map_err(SshError::Io)?;
        self.write_file(remote, &data).await
    }
}

impl Session {
    /// Opens a fresh SFTP subsystem channel. Each call opens a new
    /// channel, so multiple `Sftp` handles can be used concurrently.
    pub async fn sftp(&self) -> super::Result<Sftp> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::from)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(SshError::from)?;
        let inner = SftpSession::new(channel.into_stream())
            .await
            .map_err(sftp_err)?;
        Ok(Sftp { inner })
    }
}
