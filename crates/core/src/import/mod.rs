//! Importers: `~/.ssh/config` and Termius CSV.
//!
//! Both importers follow the same shape: a pure parser turns input text into
//! a list of parsed entries, `plan_*` turns that into an [`ImportPlan`] (no
//! I/O), and `apply` commits the plan into a [`crate::vault::Vault`],
//! reading any referenced key files from disk along the way and returning an
//! [`ImportReport`] describing what happened.

pub mod ssh_config;
pub mod termius_csv;

use crate::vault::VaultError;

/// Summary of what an importer did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub hosts_added: usize,
    /// Names of hosts that already existed in the vault and were left alone.
    pub hosts_skipped: Vec<String>,
    pub keys_added: usize,
    pub identities_added: usize,
    pub groups_added: usize,
    pub warnings: Vec<String>,
}

/// Options shared by both importers' `apply` steps.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Passphrase to try when decrypting imported private key material
    /// (identity files, inline keys). If it doesn't decrypt the key (or no
    /// passphrase is given for an encrypted key), the key is still imported
    /// with an empty passphrase, to be supplied later at connect time; see
    /// [`ImportReport::warnings`].
    pub key_passphrase: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ImportError>;
