//! Key hierarchy: account key pairs, vault key and per-item envelopes.
//!
//! See `docs/blob-format.md` for the AAD and signing layouts.

use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::{aead, random_bytes, seal, sign, CryptoError, Result, SymmetricKey};

const AAD_ACCOUNT_KEYS: &[u8] = b"ourshell:account-keys";
const AAD_RECOVERY: &[u8] = b"ourshell:recovery";
const SIGN_DOMAIN: &[u8] = b"ourshell:item:v1";

/// The account's X25519 (encryption) and Ed25519 (signing) key pairs.
pub struct AccountKeys {
    pub enc_secret: StaticSecret,
    pub sign_secret: SigningKey,
}

/// Public halves, safe to store anywhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountPublicKeys {
    pub enc_public: [u8; 32],
    pub sign_public: [u8; 32],
}

impl AccountKeys {
    pub fn generate() -> Result<Self> {
        let mut a = Zeroizing::new([0u8; 32]);
        let mut b = Zeroizing::new([0u8; 32]);
        random_bytes(a.as_mut())?;
        random_bytes(b.as_mut())?;
        Ok(Self {
            enc_secret: StaticSecret::from(*a),
            sign_secret: SigningKey::from_bytes(&b),
        })
    }

    pub fn public(&self) -> AccountPublicKeys {
        AccountPublicKeys {
            enc_public: PublicKey::from(&self.enc_secret).to_bytes(),
            sign_public: self.sign_secret.verifying_key().to_bytes(),
        }
    }

    /// Encrypts both secret keys under the master key.
    pub fn wrap(&self, master: &SymmetricKey) -> Result<Vec<u8>> {
        let mut buf = Zeroizing::new([0u8; 64]);
        buf[..32].copy_from_slice(self.enc_secret.as_bytes());
        buf[32..].copy_from_slice(&self.sign_secret.to_bytes());
        aead::encrypt(master, buf.as_ref(), AAD_ACCOUNT_KEYS)
    }

    pub fn unwrap(master: &SymmetricKey, wrapped: &[u8]) -> Result<Self> {
        let pt = Zeroizing::new(aead::decrypt(master, wrapped, AAD_ACCOUNT_KEYS)?);
        if pt.len() != 64 {
            return Err(CryptoError::Format);
        }
        let mut a = Zeroizing::new([0u8; 32]);
        let mut b = Zeroizing::new([0u8; 32]);
        a.copy_from_slice(&pt[..32]);
        b.copy_from_slice(&pt[32..]);
        Ok(Self {
            enc_secret: StaticSecret::from(*a),
            sign_secret: SigningKey::from_bytes(&b),
        })
    }
}

impl AccountPublicKeys {
    pub fn enc(&self) -> PublicKey {
        PublicKey::from(self.enc_public)
    }

    pub fn verifying(&self) -> Result<VerifyingKey> {
        VerifyingKey::from_bytes(&self.sign_public).map_err(|_| CryptoError::Format)
    }
}

/// Seals the vault key to an account's public encryption key.
pub fn seal_vault_key(recipient: &AccountPublicKeys, vault_key: &SymmetricKey) -> Result<Vec<u8>> {
    seal::seal(&recipient.enc(), vault_key.as_bytes())
}

pub fn open_vault_key(account: &AccountKeys, sealed: &[u8]) -> Result<SymmetricKey> {
    let pt = Zeroizing::new(seal::open(&account.enc_secret, sealed)?);
    let bytes: [u8; 32] = pt.as_slice().try_into().map_err(|_| CryptoError::Format)?;
    Ok(SymmetricKey::from_bytes(bytes))
}

/// Second wrap of the vault key, under the recovery key.
pub fn wrap_with_recovery(recovery: &SymmetricKey, vault_key: &SymmetricKey) -> Result<Vec<u8>> {
    aead::encrypt(recovery, vault_key.as_bytes(), AAD_RECOVERY)
}

pub fn unwrap_with_recovery(recovery: &SymmetricKey, wrapped: &[u8]) -> Result<SymmetricKey> {
    let pt = Zeroizing::new(aead::decrypt(recovery, wrapped, AAD_RECOVERY)?);
    let bytes: [u8; 32] = pt.as_slice().try_into().map_err(|_| CryptoError::Format)?;
    Ok(SymmetricKey::from_bytes(bytes))
}

/// An encrypted, signed vault item. Everything but the metadata is ciphertext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub org_id: String,
    pub key_id: String,
    #[serde(with = "b64")]
    pub wrapped_key: Vec<u8>,
    #[serde(with = "b64")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "b64")]
    pub signature: Vec<u8>,
}

fn push_field(out: &mut Vec<u8>, f: &[u8]) {
    out.extend_from_slice(&(f.len() as u32).to_be_bytes());
    out.extend_from_slice(f);
}

impl ItemEnvelope {
    fn item_aad(id: &str, kind: &str, version: u64) -> Vec<u8> {
        format!("{id}|{kind}|{version}").into_bytes()
    }

    fn key_aad(id: &str) -> Vec<u8> {
        format!("ourshell:item-key:{id}").into_bytes()
    }

    /// Canonical bytes covered by the signature.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = SIGN_DOMAIN.to_vec();
        push_field(&mut out, self.id.as_bytes());
        push_field(&mut out, self.kind.as_bytes());
        out.extend_from_slice(&self.version.to_be_bytes());
        push_field(&mut out, self.org_id.as_bytes());
        push_field(&mut out, self.key_id.as_bytes());
        push_field(&mut out, &self.wrapped_key);
        push_field(&mut out, &self.ciphertext);
        out
    }

    /// Encrypts `plaintext` with a fresh item key, wraps it, and signs the result.
    #[allow(clippy::too_many_arguments)]
    pub fn seal(
        id: &str,
        kind: &str,
        version: u64,
        org_id: &str,
        key_id: &str,
        plaintext: &[u8],
        vault_key: &SymmetricKey,
        signer: &SigningKey,
    ) -> Result<Self> {
        let item_key = SymmetricKey::generate()?;
        let ciphertext = aead::encrypt(&item_key, plaintext, &Self::item_aad(id, kind, version))?;
        let wrapped_key = aead::encrypt(vault_key, item_key.as_bytes(), &Self::key_aad(id))?;
        let mut env = Self {
            id: id.into(),
            kind: kind.into(),
            version,
            org_id: org_id.into(),
            key_id: key_id.into(),
            wrapped_key,
            ciphertext,
            signature: Vec::new(),
        };
        env.signature = sign::sign(signer, &env.signing_bytes()).to_vec();
        Ok(env)
    }

    /// Verifies the signature, then decrypts.
    pub fn open(&self, vault_key: &SymmetricKey, verifier: &VerifyingKey) -> Result<Vec<u8>> {
        sign::verify(verifier, &self.signing_bytes(), &self.signature)?;
        let item_key = self.unwrap_item_key(vault_key)?;
        aead::decrypt(
            &item_key,
            &self.ciphertext,
            &Self::item_aad(&self.id, &self.kind, self.version),
        )
    }

    pub fn unwrap_item_key(&self, vault_key: &SymmetricKey) -> Result<SymmetricKey> {
        let pt = Zeroizing::new(aead::decrypt(
            vault_key,
            &self.wrapped_key,
            &Self::key_aad(&self.id),
        )?);
        let bytes: [u8; 32] = pt.as_slice().try_into().map_err(|_| CryptoError::Format)?;
        Ok(SymmetricKey::from_bytes(bytes))
    }

    /// Re-wraps this item's key under another vault key (used for sharing).
    pub fn rewrap(
        &self,
        from: &SymmetricKey,
        to: &SymmetricKey,
        to_key_id: &str,
        signer: &SigningKey,
    ) -> Result<Self> {
        let item_key = self.unwrap_item_key(from)?;
        let mut env = self.clone();
        env.key_id = to_key_id.into();
        env.wrapped_key = aead::encrypt(to, item_key.as_bytes(), &Self::key_aad(&self.id))?;
        env.signature = sign::sign(signer, &env.signing_bytes()).to_vec();
        Ok(env)
    }
}

pub(crate) mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kdf::{derive_master_key, KdfParams};

    #[test]
    fn account_keys_wrap_round_trip() {
        let master = derive_master_key(b"pw", &KdfParams::insecure_for_tests()).unwrap();
        let acct = AccountKeys::generate().unwrap();
        let wrapped = acct.wrap(&master).unwrap();
        let back = AccountKeys::unwrap(&master, &wrapped).unwrap();
        assert_eq!(acct.public(), back.public());
        let wrong = derive_master_key(b"nope", &KdfParams::insecure_for_tests()).unwrap();
        assert!(AccountKeys::unwrap(&wrong, &wrapped).is_err());
    }

    #[test]
    fn vault_key_seal_and_recovery() {
        let acct = AccountKeys::generate().unwrap();
        let vk = SymmetricKey::generate().unwrap();
        let sealed = seal_vault_key(&acct.public(), &vk).unwrap();
        assert_eq!(
            open_vault_key(&acct, &sealed).unwrap().as_bytes(),
            vk.as_bytes()
        );

        let rk = SymmetricKey::generate().unwrap();
        let w = wrap_with_recovery(&rk, &vk).unwrap();
        assert_eq!(
            unwrap_with_recovery(&rk, &w).unwrap().as_bytes(),
            vk.as_bytes()
        );
    }

    fn sample() -> (AccountKeys, SymmetricKey, ItemEnvelope) {
        let acct = AccountKeys::generate().unwrap();
        let vk = SymmetricKey::generate().unwrap();
        let env = ItemEnvelope::seal(
            "id1",
            "host",
            3,
            "org",
            "vk1",
            b"{\"h\":1}",
            &vk,
            &acct.sign_secret,
        )
        .unwrap();
        (acct, vk, env)
    }

    #[test]
    fn item_round_trip() {
        let (acct, vk, env) = sample();
        let vkey = acct.public().verifying().unwrap();
        assert_eq!(env.open(&vk, &vkey).unwrap(), b"{\"h\":1}");
        let json = serde_json::to_string(&env).unwrap();
        let back: ItemEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn item_version_bump_by_server_is_detected() {
        let (acct, vk, mut env) = sample();
        env.version = 4;
        let vkey = acct.public().verifying().unwrap();
        assert_eq!(env.open(&vk, &vkey), Err(CryptoError::BadSignature));
    }

    #[test]
    fn item_signed_by_other_account_is_rejected() {
        let (_, vk, env) = sample();
        let other = AccountKeys::generate().unwrap();
        assert_eq!(
            env.open(&vk, &other.public().verifying().unwrap()),
            Err(CryptoError::BadSignature)
        );
    }

    #[test]
    fn rewrap_for_sharing() {
        let (acct, vk, env) = sample();
        let team = SymmetricKey::generate().unwrap();
        let shared = env.rewrap(&vk, &team, "team1", &acct.sign_secret).unwrap();
        let vkey = acct.public().verifying().unwrap();
        assert_eq!(shared.open(&team, &vkey).unwrap(), b"{\"h\":1}");
        assert_eq!(shared.ciphertext, env.ciphertext);
    }
}
