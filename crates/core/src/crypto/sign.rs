//! Ed25519 signatures.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use super::{CryptoError, Result};

pub fn sign(key: &SigningKey, msg: &[u8]) -> [u8; 64] {
    key.sign(msg).to_bytes()
}

pub fn verify(key: &VerifyingKey, msg: &[u8], sig: &[u8]) -> Result<()> {
    let sig: [u8; 64] = sig.try_into().map_err(|_| CryptoError::BadSignature)?;
    key.verify(msg, &Signature::from_bytes(&sig))
        .map_err(|_| CryptoError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::random_bytes;

    #[test]
    fn sign_verify() {
        let mut seed = [0u8; 32];
        random_bytes(&mut seed).unwrap();
        let sk = SigningKey::from_bytes(&seed);
        let sig = sign(&sk, b"msg");
        verify(&sk.verifying_key(), b"msg", &sig).unwrap();
        assert_eq!(
            verify(&sk.verifying_key(), b"msh", &sig),
            Err(CryptoError::BadSignature)
        );
        assert_eq!(
            verify(&sk.verifying_key(), b"msg", &sig[..10]),
            Err(CryptoError::BadSignature)
        );
    }
}
