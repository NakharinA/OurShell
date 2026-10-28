//! XChaCha20-Poly1305 ciphertext container (format v1).
//!
//! `version(1) | alg(1) | nonce(24) | ciphertext+tag`

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use super::{random_bytes, CryptoError, Result, SymmetricKey};

pub const VERSION: u8 = ourshell_proto::BLOB_VERSION;
pub const ALG_XCHACHA20POLY1305: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 2 + NONCE_LEN;

/// Encrypts `plaintext` under `key`, binding `aad`. Returns the v1 container.
pub fn encrypt(key: &SymmetricKey, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    random_bytes(&mut nonce)?;
    encrypt_with_nonce(key, &nonce, plaintext, aad)
}

/// Deterministic variant used only for test vectors.
pub(crate) fn encrypt_with_nonce(
    key: &SymmetricKey,
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let ct = cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.push(VERSION);
    out.push(ALG_XCHACHA20POLY1305);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypts a v1 container produced by [`encrypt`].
pub fn decrypt(key: &SymmetricKey, blob: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < HEADER_LEN + TAG_LEN || blob[0] != VERSION || blob[1] != ALG_XCHACHA20POLY1305 {
        return Err(CryptoError::Format);
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&blob[2..HEADER_LEN]);
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &blob[HEADER_LEN..],
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = SymmetricKey::generate().unwrap();
        let ct = encrypt(&k, b"hello", b"aad").unwrap();
        assert_eq!(ct.len(), HEADER_LEN + 5 + TAG_LEN);
        assert_eq!(decrypt(&k, &ct, b"aad").unwrap(), b"hello");
    }

    #[test]
    fn nonces_are_unique() {
        let k = SymmetricKey::generate().unwrap();
        let a = encrypt(&k, b"x", b"").unwrap();
        let b = encrypt(&k, b"x", b"").unwrap();
        assert_ne!(a[2..HEADER_LEN], b[2..HEADER_LEN]);
    }

    #[test]
    fn rejects_wrong_key_aad_and_tampering() {
        let k = SymmetricKey::generate().unwrap();
        let other = SymmetricKey::generate().unwrap();
        let mut ct = encrypt(&k, b"secret", b"aad").unwrap();
        assert_eq!(decrypt(&other, &ct, b"aad"), Err(CryptoError::Decrypt));
        assert_eq!(decrypt(&k, &ct, b"other"), Err(CryptoError::Decrypt));
        let last = ct.len() - 1;
        ct[last] ^= 1;
        assert_eq!(decrypt(&k, &ct, b"aad"), Err(CryptoError::Decrypt));
    }

    #[test]
    fn rejects_bad_format() {
        let k = SymmetricKey::generate().unwrap();
        assert_eq!(decrypt(&k, &[1, 1, 0], b""), Err(CryptoError::Format));
        let mut ct = encrypt(&k, b"x", b"").unwrap();
        ct[0] = 9;
        assert_eq!(decrypt(&k, &ct, b""), Err(CryptoError::Format));
    }

    #[test]
    fn fixed_vector() {
        // Pins the wire format: key = 0x00..1f, nonce = 0x40..57.
        let key = SymmetricKey::from_bytes(core::array::from_fn(|i| i as u8));
        let nonce: [u8; 24] = core::array::from_fn(|i| 0x40 + i as u8);
        let ct = encrypt_with_nonce(&key, &nonce, b"ourshell", b"v1").unwrap();
        assert_eq!(&ct[..2], &[1, 1]);
        assert_eq!(&ct[2..26], &nonce);
        assert_eq!(decrypt(&key, &ct, b"v1").unwrap(), b"ourshell");
    }
}
