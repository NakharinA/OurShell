//! SSH key generation and import.
//!
//! Built on `russh::keys`, which re-exports the `ssh-key` crate used by `russh`
//! itself. We deliberately avoid depending on `ssh-key` directly so that our
//! version always matches the one `russh` was compiled against.

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData, RsaKeypair};
use russh::keys::ssh_key::rand_core::{TryCryptoRng, TryRng};
use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey};
use russh::keys::{decode_secret_key, Error as KeysError};
use zeroize::Zeroizing;

/// Key algorithm to generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAlgorithm {
    Ed25519,
    /// RSA with the given modulus size in bits. 2048, 3072 (default) and 4096
    /// are accepted; anything else is rejected.
    Rsa {
        bits: usize,
    },
}

impl Default for KeyAlgorithm {
    fn default() -> Self {
        KeyAlgorithm::Rsa { bits: 3072 }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeygenError {
    #[error("unsupported RSA key size: {0} bits (use 2048, 3072 or 4096)")]
    UnsupportedKeySize(usize),
    #[error("the private key is encrypted; a passphrase is required")]
    PassphraseRequired,
    #[error("wrong passphrase for this private key")]
    WrongPassphrase,
    #[error("not a recognized private key: {0}")]
    InvalidKey(String),
    #[error(transparent)]
    Ssh(#[from] russh::keys::ssh_key::Error),
    #[error(transparent)]
    Keys(#[from] KeysError),
}

pub type Result<T> = std::result::Result<T, KeygenError>;

/// A freshly generated key pair.
#[derive(Debug)]
pub struct GeneratedKey {
    /// OpenSSH-format private key, encrypted if a passphrase was supplied.
    pub private_key_openssh: Zeroizing<String>,
    /// OpenSSH-format public key line (`ssh-ed25519 AAAA... comment`).
    pub public_key_openssh: String,
    /// `SHA256:...` fingerprint of the public key.
    pub fingerprint_sha256: String,
}

impl GeneratedKey {
    /// Builds the vault item for this key. `passphrase` is stored alongside
    /// the (possibly encrypted) private key so the vault can offer it back
    /// at connect time; pass `None` if the key is unencrypted.
    pub fn to_vault_item(&self, name: &str, passphrase: Option<&str>) -> crate::vault::SshKey {
        crate::vault::SshKey {
            name: name.to_string(),
            private_key: self.private_key_openssh.to_string(),
            public_key: self.public_key_openssh.clone(),
            passphrase: passphrase.map(str::to_string),
        }
    }
}

/// A key imported from existing private key text.
#[derive(Debug)]
pub struct ImportedKey {
    /// The private key text exactly as supplied (OpenSSH or legacy PEM,
    /// encrypted or not).
    pub private_key_openssh: Zeroizing<String>,
    /// OpenSSH-format public key line, derived from the private key.
    pub public_key_openssh: String,
    /// `SHA256:...` fingerprint of the public key.
    pub fingerprint_sha256: String,
    /// Algorithm identifier, e.g. `ssh-ed25519` or `ssh-rsa`.
    pub algorithm: String,
    /// Whether the supplied private key text was encrypted.
    pub was_encrypted: bool,
}

impl ImportedKey {
    /// Builds the vault item for this key. See [`GeneratedKey::to_vault_item`].
    pub fn to_vault_item(&self, name: &str, passphrase: Option<&str>) -> crate::vault::SshKey {
        crate::vault::SshKey {
            name: name.to_string(),
            private_key: self.private_key_openssh.to_string(),
            public_key: self.public_key_openssh.clone(),
            passphrase: passphrase.map(str::to_string),
        }
    }
}

/// Generates a new key pair.
///
/// When `passphrase` is `Some`, the returned private key is encrypted in
/// OpenSSH format (AES-256-CTR / bcrypt-pbkdf).
pub fn generate(
    alg: KeyAlgorithm,
    comment: &str,
    passphrase: Option<&str>,
) -> Result<GeneratedKey> {
    let mut rng = OsCryptoRng;
    let key_data = match alg {
        KeyAlgorithm::Ed25519 => KeypairData::from(Ed25519Keypair::random(&mut rng)),
        KeyAlgorithm::Rsa { bits } => {
            if !matches!(bits, 2048 | 3072 | 4096) {
                return Err(KeygenError::UnsupportedKeySize(bits));
            }
            KeypairData::from(RsaKeypair::random(&mut rng, bits)?)
        }
    };
    let private = PrivateKey::new(key_data, comment)?;
    let public_key_openssh = private.public_key().to_openssh()?;
    let fingerprint_sha256 = private.fingerprint(HashAlg::Sha256).to_string();

    let private = match passphrase {
        Some(pass) if !pass.is_empty() => private.encrypt(&mut rng, pass)?,
        _ => private,
    };
    let private_key_openssh = private.to_openssh(LineEnding::LF)?;

    Ok(GeneratedKey {
        private_key_openssh,
        public_key_openssh,
        fingerprint_sha256,
    })
}

/// Imports a private key from text.
///
/// Accepts OpenSSH private keys (`-----BEGIN OPENSSH PRIVATE KEY-----`,
/// encrypted or not) and, on a best-effort basis, legacy PEM-encoded RSA
/// (PKCS#1) and PKCS#8 keys, as supported by `russh::keys::decode_secret_key`.
///
/// A wrong passphrase on an encrypted key yields [`KeygenError::WrongPassphrase`],
/// distinct from a malformed/garbage input ([`KeygenError::InvalidKey`]).
pub fn import(private_key_text: &str, passphrase: Option<&str>) -> Result<ImportedKey> {
    let text = private_key_text.trim();
    if text.is_empty() {
        return Err(KeygenError::InvalidKey("empty input".into()));
    }

    let key = match decode_secret_key(text, passphrase) {
        Ok(key) => key,
        Err(KeysError::KeyIsEncrypted) => return Err(KeygenError::PassphraseRequired),
        Err(e) => {
            // The key may genuinely be corrupt, or `passphrase` may simply be
            // wrong. Disambiguate by checking whether decoding without any
            // passphrase reports the key as encrypted: if so, a passphrase
            // was supplied but didn't work.
            if passphrase.is_some() && is_encrypted_without_passphrase(text) {
                return Err(KeygenError::WrongPassphrase);
            }
            return Err(KeygenError::InvalidKey(e.to_string()));
        }
    };

    let was_encrypted = decode_secret_key(text, None).is_err();
    let public_key_openssh = key.public_key().to_openssh()?;
    let fingerprint_sha256 = key.fingerprint(HashAlg::Sha256).to_string();
    let algorithm = key.algorithm().as_str().to_string();

    Ok(ImportedKey {
        private_key_openssh: Zeroizing::new(text.to_string()),
        public_key_openssh,
        fingerprint_sha256,
        algorithm,
        was_encrypted,
    })
}

fn is_encrypted_without_passphrase(text: &str) -> bool {
    matches!(
        decode_secret_key(text, None),
        Err(KeysError::KeyIsEncrypted)
    )
}

/// Best-effort public key extraction, even when the key is encrypted and the
/// passphrase is unknown. OpenSSH-format private keys store their public key
/// in cleartext alongside the (possibly encrypted) private portion, so this
/// can succeed without decrypting anything. Legacy PEM formats don't expose
/// the public key this way, so this returns `None` for those.
pub fn peek_public_key(private_key_text: &str) -> Option<String> {
    let text = private_key_text.trim();
    PrivateKey::from_openssh(text)
        .ok()?
        .public_key()
        .to_openssh()
        .ok()
}

/// A CSPRNG adapter over the OS RNG (via `getrandom`), implementing the
/// `rand_core` traits that `ssh-key`'s key generation and encryption APIs
/// require. We avoid a direct `rand_core` dependency by implementing these
/// through `russh::keys::ssh_key::rand_core`, which is guaranteed to be the
/// exact version `ssh-key` was built against.
struct OsCryptoRng;

impl TryRng for OsCryptoRng {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> std::result::Result<u32, Self::Error> {
        let mut buf = [0u8; 4];
        getrandom::fill(&mut buf).expect("OS RNG failure");
        Ok(u32::from_le_bytes(buf))
    }

    fn try_next_u64(&mut self) -> std::result::Result<u64, Self::Error> {
        let mut buf = [0u8; 8];
        getrandom::fill(&mut buf).expect("OS RNG failure");
        Ok(u64::from_le_bytes(buf))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> std::result::Result<(), Self::Error> {
        getrandom::fill(dst).expect("OS RNG failure");
        Ok(())
    }
}

impl TryCryptoRng for OsCryptoRng {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_ed25519() {
        let key = generate(KeyAlgorithm::Ed25519, "test@ourshell", None).unwrap();
        assert!(key.public_key_openssh.starts_with("ssh-ed25519 "));
        assert!(key.fingerprint_sha256.starts_with("SHA256:"));
        assert!(key
            .private_key_openssh
            .contains("-----BEGIN OPENSSH PRIVATE KEY-----"));
    }

    #[test]
    fn generate_rsa_2048_is_slow_but_works() {
        // RSA generation in a debug build is slow; stick to the smallest
        // accepted size to keep the test suite fast.
        let key = generate(KeyAlgorithm::Rsa { bits: 2048 }, "rsa@ourshell", None).unwrap();
        assert!(key.public_key_openssh.starts_with("ssh-rsa "));
        assert!(key.fingerprint_sha256.starts_with("SHA256:"));
    }

    #[test]
    fn generate_rejects_bad_rsa_size() {
        let err = generate(KeyAlgorithm::Rsa { bits: 1024 }, "x", None).unwrap_err();
        assert!(matches!(err, KeygenError::UnsupportedKeySize(1024)));
    }

    #[test]
    fn round_trip_import_unencrypted() {
        let generated = generate(KeyAlgorithm::Ed25519, "roundtrip@ourshell", None).unwrap();
        let imported = import(&generated.private_key_openssh, None).unwrap();
        assert_eq!(imported.public_key_openssh, generated.public_key_openssh);
        assert_eq!(imported.fingerprint_sha256, generated.fingerprint_sha256);
        assert_eq!(imported.algorithm, "ssh-ed25519");
        assert!(!imported.was_encrypted);
    }

    #[test]
    fn import_encrypted_with_right_passphrase() {
        let generated =
            generate(KeyAlgorithm::Ed25519, "enc@ourshell", Some("correct horse")).unwrap();
        let imported = import(&generated.private_key_openssh, Some("correct horse")).unwrap();
        assert!(imported.was_encrypted);
        assert_eq!(imported.public_key_openssh, generated.public_key_openssh);
    }

    #[test]
    fn import_encrypted_with_wrong_passphrase() {
        let generated =
            generate(KeyAlgorithm::Ed25519, "enc@ourshell", Some("correct horse")).unwrap();
        let err = import(&generated.private_key_openssh, Some("wrong password")).unwrap_err();
        assert!(matches!(err, KeygenError::WrongPassphrase));
    }

    #[test]
    fn import_encrypted_without_passphrase() {
        let generated =
            generate(KeyAlgorithm::Ed25519, "enc@ourshell", Some("correct horse")).unwrap();
        let err = import(&generated.private_key_openssh, None).unwrap_err();
        assert!(matches!(err, KeygenError::PassphraseRequired));
    }

    #[test]
    fn import_garbage_is_invalid() {
        let err = import("not a key at all", None).unwrap_err();
        assert!(matches!(err, KeygenError::InvalidKey(_)));
    }

    #[test]
    fn peek_public_key_works_without_passphrase() {
        let generated = generate(
            KeyAlgorithm::Ed25519,
            "peek@ourshell",
            Some("correct horse"),
        )
        .unwrap();
        let pub_line = peek_public_key(&generated.private_key_openssh).unwrap();
        // The comment lives inside the encrypted section of an OpenSSH key,
        // so peeking without the passphrase recovers the key material but
        // not the comment.
        let same_key_material = |a: &str, b: &str| {
            a.split_whitespace()
                .take(2)
                .eq(b.split_whitespace().take(2))
        };
        assert!(same_key_material(&pub_line, &generated.public_key_openssh));
    }

    #[test]
    fn to_vault_item_round_trip() {
        let generated = generate(KeyAlgorithm::Ed25519, "vault@ourshell", None).unwrap();
        let item = generated.to_vault_item("my-key", None);
        assert_eq!(item.name, "my-key");
        assert_eq!(item.public_key, generated.public_key_openssh);
        assert!(item.passphrase.is_none());
    }
}
