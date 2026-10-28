//! Client-side cryptography. See `docs/blob-format.md` for the byte formats.
//!
//! Only audited RustCrypto / dalek primitives are used; this module composes them.

pub mod aead;
pub mod kdf;
pub mod keys;
pub mod recovery;
pub mod seal;
pub mod sign;

use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("random number generator failed")]
    Rng,
    #[error("malformed or unsupported ciphertext")]
    Format,
    #[error("decryption failed (wrong key or tampered data)")]
    Decrypt,
    #[error("signature verification failed")]
    BadSignature,
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("invalid recovery phrase")]
    Recovery,
}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// Fills `buf` from the OS CSPRNG.
pub fn random_bytes(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|_| CryptoError::Rng)
}

/// A 256-bit symmetric key that is wiped from memory on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymmetricKey([u8; 32]);

impl SymmetricKey {
    pub fn generate() -> Result<Self> {
        let mut k = [0u8; 32];
        random_bytes(&mut k)?;
        Ok(Self(k))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SymmetricKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SymmetricKey(<redacted>)")
    }
}
