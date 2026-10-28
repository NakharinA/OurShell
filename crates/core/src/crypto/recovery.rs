//! Recovery key: 256 random bits shown to the user once as 24 BIP39 words.

use super::{CryptoError, Result, SymmetricKey};

pub fn to_words(key: &SymmetricKey) -> String {
    bip39::Mnemonic::from_entropy(key.as_bytes())
        .expect("32 bytes is valid BIP39 entropy")
        .to_string()
}

pub fn from_words(words: &str) -> Result<SymmetricKey> {
    let m = bip39::Mnemonic::parse_normalized(words.trim()).map_err(|_| CryptoError::Recovery)?;
    let entropy = m.to_entropy();
    let bytes: [u8; 32] = entropy.try_into().map_err(|_| CryptoError::Recovery)?;
    Ok(SymmetricKey::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = SymmetricKey::generate().unwrap();
        let words = to_words(&k);
        assert_eq!(words.split_whitespace().count(), 24);
        assert_eq!(from_words(&words).unwrap().as_bytes(), k.as_bytes());
    }

    #[test]
    fn rejects_garbage() {
        assert!(from_words("not a phrase").is_err());
    }
}
