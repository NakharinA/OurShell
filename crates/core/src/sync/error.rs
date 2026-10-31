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
    /// OPAQUE rejected the login. By design it can't tell which part was
    /// wrong: the server answers unknown emails with a decoy, so a wrong
    /// password, a mistyped email and a missing account look the same.
    #[error("wrong email or password, or this server has no account with that email")]
    InvalidCredentials,
}

pub type Result<T> = std::result::Result<T, SyncError>;
