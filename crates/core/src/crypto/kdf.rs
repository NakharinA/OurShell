//! Argon2id master-key derivation.

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};

use super::{random_bytes, CryptoError, Result, SymmetricKey};

/// Argon2id parameters, stored next to the salt so they can be raised later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: [u8; 16],
}

impl KdfParams {
    /// Production defaults: 64 MiB, 3 passes, 1 lane, fresh random salt.
    pub fn new_default() -> Result<Self> {
        let mut salt = [0u8; 16];
        random_bytes(&mut salt)?;
        Ok(Self {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
            salt,
        })
    }

    /// Cheap parameters for tests only.
    #[doc(hidden)]
    pub fn insecure_for_tests() -> Self {
        Self {
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            salt: [7u8; 16],
        }
    }
}

/// Derives the 256-bit master key from the master password.
pub fn derive_master_key(password: &[u8], params: &KdfParams) -> Result<SymmetricKey> {
    let p = Params::new(params.m_cost_kib, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password, &params.salt, &mut out)
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    Ok(SymmetricKey::from_bytes(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_password_sensitive() {
        let p = KdfParams::insecure_for_tests();
        let a = derive_master_key(b"pw", &p).unwrap();
        let b = derive_master_key(b"pw", &p).unwrap();
        let c = derive_master_key(b"pw2", &p).unwrap();
        assert_eq!(a.as_bytes(), b.as_bytes());
        assert_ne!(a.as_bytes(), c.as_bytes());
    }

    #[test]
    fn salt_matters() {
        let p1 = KdfParams::insecure_for_tests();
        let mut p2 = p1.clone();
        p2.salt[0] ^= 1;
        assert_ne!(
            derive_master_key(b"pw", &p1).unwrap().as_bytes(),
            derive_master_key(b"pw", &p2).unwrap().as_bytes()
        );
    }
}
