//! Server-side "second layer" encryption at rest (see `docs/blob-format.md`
//! and the Encryption & key design section of the project plan).
//!
//! This guards stolen backups/disks only. It uses the exact same v1
//! container (`version | alg | nonce | ciphertext+tag`) as the client-side
//! blob format so the two are easy to reason about together, but it is a
//! second, independent layer: the server's key never unlocks client data,
//! it only protects `items.envelope` and `totp.secret` against someone who
//! only has the database.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use thiserror::Error;

const VERSION: u8 = ourshell_proto::BLOB_VERSION;
const ALG_XCHACHA20POLY1305: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 2 + NONCE_LEN;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AtRestError {
    #[error("malformed or unsupported at-rest ciphertext")]
    Format,
    #[error("at-rest decryption failed (wrong key, wrong AAD, or tampered data)")]
    Decrypt,
}

/// Encrypts `plaintext` under the server master key, binding `aad` (row
/// identity, e.g. `b"item:<vault_id>:<id>"`).
pub fn encrypt(master_key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(master_key.into());
    // Encryption with a fresh random 24-byte nonce cannot fail.
    let ct = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption is infallible for valid inputs");
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.push(VERSION);
    out.push(ALG_XCHACHA20POLY1305);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Decrypts a container produced by [`encrypt`].
pub fn decrypt(master_key: &[u8; 32], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, AtRestError> {
    if blob.len() < HEADER_LEN + TAG_LEN || blob[0] != VERSION || blob[1] != ALG_XCHACHA20POLY1305 {
        return Err(AtRestError::Format);
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&blob[2..HEADER_LEN]);
    let cipher = XChaCha20Poly1305::new(master_key.into());
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &blob[HEADER_LEN..],
                aad,
            },
        )
        .map_err(|_| AtRestError::Decrypt)
}

/// SHA-256 of a bearer/session token. Only the hash is ever stored.
pub fn hash_token(token: &str) -> Vec<u8> {
    use sha2_for_tokens::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().to_vec()
}

/// Generates a random, URL-safe opaque session/invite token.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

use base64::Engine;

// sha2 is only needed for token hashing; keep the import scoped so it's
// obvious this has nothing to do with the OPAQUE/TripleDH SHA-512 usage
// pulled in transitively via ourshell-proto's `opaque` feature.
mod sha2_for_tokens {
    pub use sha2::{Digest, Sha256};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = [7u8; 32];
        let ct = encrypt(&key, b"hello at rest", b"aad");
        assert_eq!(decrypt(&key, &ct, b"aad").unwrap(), b"hello at rest");
    }

    #[test]
    fn wrong_key_or_aad_fails() {
        let key = [7u8; 32];
        let other = [9u8; 32];
        let ct = encrypt(&key, b"secret", b"aad");
        assert_eq!(decrypt(&other, &ct, b"aad"), Err(AtRestError::Decrypt));
        assert_eq!(decrypt(&key, &ct, b"other-aad"), Err(AtRestError::Decrypt));
    }

    #[test]
    fn ciphertext_never_contains_plaintext() {
        let key = [1u8; 32];
        let plaintext = b"{\"super\":\"secret-json\"}";
        let ct = encrypt(&key, plaintext, b"aad");
        assert!(!ct
            .windows(plaintext.len())
            .any(|w| w == plaintext.as_slice()));
    }

    #[test]
    fn tokens_hash_deterministically_and_differ() {
        let a = hash_token("token-a");
        let b = hash_token("token-a");
        let c = hash_token("token-b");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn random_tokens_are_unique() {
        assert_ne!(random_token(), random_token());
    }
}
