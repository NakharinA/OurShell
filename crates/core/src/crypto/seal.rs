//! Sealing data to an X25519 public key (anonymous sender).
//!
//! Construction (same shape as HPKE base mode with one-shot export):
//! ephemeral X25519 → shared secret → HKDF-SHA256(salt = eph_pk ‖ recipient_pk,
//! info = "ourshell:seal:v1") → XChaCha20-Poly1305 key, encrypted in the v1 container.
//!
//! Wire format: `version(1) | eph_pk(32) | aead container`.

use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use super::{aead, random_bytes, CryptoError, Result, SymmetricKey};

const SEAL_VERSION: u8 = 1;
const INFO: &[u8] = b"ourshell:seal:v1";

fn derive(shared: &[u8; 32], eph_pk: &[u8; 32], recipient_pk: &[u8; 32]) -> Result<SymmetricKey> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph_pk);
    salt[32..].copy_from_slice(recipient_pk);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut okm = [0u8; 32];
    hk.expand(INFO, &mut okm).map_err(|_| CryptoError::Format)?;
    Ok(SymmetricKey::from_bytes(okm))
}

/// Encrypts `plaintext` so only the holder of `recipient`'s secret can read it.
pub fn seal(recipient: &PublicKey, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut eph_bytes = [0u8; 32];
    random_bytes(&mut eph_bytes)?;
    let eph = StaticSecret::from(eph_bytes);
    let eph_pk = PublicKey::from(&eph);
    let shared = eph.diffie_hellman(recipient);
    if !shared.was_contributory() {
        return Err(CryptoError::Format);
    }
    let key = derive(shared.as_bytes(), eph_pk.as_bytes(), recipient.as_bytes())?;
    let ct = aead::encrypt(&key, plaintext, eph_pk.as_bytes())?;
    let mut out = Vec::with_capacity(33 + ct.len());
    out.push(SEAL_VERSION);
    out.extend_from_slice(eph_pk.as_bytes());
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens a sealed box with the recipient's X25519 secret.
pub fn open(secret: &StaticSecret, sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 33 || sealed[0] != SEAL_VERSION {
        return Err(CryptoError::Format);
    }
    let mut eph = [0u8; 32];
    eph.copy_from_slice(&sealed[1..33]);
    let eph_pk = PublicKey::from(eph);
    let shared = secret.diffie_hellman(&eph_pk);
    if !shared.was_contributory() {
        return Err(CryptoError::Decrypt);
    }
    let my_pk = PublicKey::from(secret);
    let key = derive(shared.as_bytes(), &eph, my_pk.as_bytes())?;
    aead::decrypt(&key, &sealed[33..], &eph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (StaticSecret, PublicKey) {
        let mut b = [0u8; 32];
        random_bytes(&mut b).unwrap();
        let s = StaticSecret::from(b);
        let p = PublicKey::from(&s);
        (s, p)
    }

    #[test]
    fn round_trip() {
        let (s, p) = keypair();
        let sealed = seal(&p, b"vault key").unwrap();
        assert_eq!(open(&s, &sealed).unwrap(), b"vault key");
    }

    #[test]
    fn wrong_recipient_fails() {
        let (_, p) = keypair();
        let (s2, _) = keypair();
        let sealed = seal(&p, b"x").unwrap();
        assert_eq!(open(&s2, &sealed), Err(CryptoError::Decrypt));
    }

    #[test]
    fn tampered_ephemeral_fails() {
        let (s, p) = keypair();
        let mut sealed = seal(&p, b"x").unwrap();
        sealed[5] ^= 1;
        assert!(open(&s, &sealed).is_err());
    }
}
