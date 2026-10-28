//! Errors from talking to an OurShell sync server.

use crate::crypto::CryptoError;
use crate::vault::VaultError;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("network error talking to the sync server: {0}")]
    Http(#[from] reqwest::Error),
    /// A well-formed error response from the server (see
    /// `ourshell_proto::sync::ApiError`).
    #[error("server error ({status} {code}): {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Opaque(#[from] ourshell_proto::opaque::OpaqueError),
    #[error("malformed data from the sync server: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sync protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;
