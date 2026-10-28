//! Property tests: crypto round trips, tamper detection, and parsers that must
//! never panic on arbitrary input. (Fuzz targets with the same shape live in `fuzz/`.)

use ourshell_core::crypto::keys::{AccountKeys, ItemEnvelope};
use ourshell_core::crypto::{aead, seal, CryptoError, SymmetricKey};
use ourshell_core::import::{ssh_config, termius_csv};
use proptest::prelude::*;

fn key(bytes: [u8; 32]) -> SymmetricKey {
    SymmetricKey::from_bytes(bytes)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn aead_round_trip(k in any::<[u8; 32]>(), pt in proptest::collection::vec(any::<u8>(), 0..2048), aad in proptest::collection::vec(any::<u8>(), 0..64)) {
        let k = key(k);
        let ct = aead::encrypt(&k, &pt, &aad).unwrap();
        prop_assert_eq!(aead::decrypt(&k, &ct, &aad).unwrap(), pt);
    }

    #[test]
    fn aead_any_bit_flip_is_detected(k in any::<[u8; 32]>(), pt in proptest::collection::vec(any::<u8>(), 1..256), pos in any::<prop::sample::Index>(), bit in 0u8..8) {
        let k = key(k);
        let mut ct = aead::encrypt(&k, &pt, b"aad").unwrap();
        let i = pos.index(ct.len());
        ct[i] ^= 1 << bit;
        prop_assert!(aead::decrypt(&k, &ct, b"aad").is_err());
    }

    #[test]
    fn aead_decrypt_never_panics(k in any::<[u8; 32]>(), blob in proptest::collection::vec(any::<u8>(), 0..512)) {
        let r = aead::decrypt(&key(k), &blob, b"");
        prop_assert!(matches!(r, Err(CryptoError::Format) | Err(CryptoError::Decrypt) | Ok(_)));
    }

    #[test]
    fn seal_open_never_panics(blob in proptest::collection::vec(any::<u8>(), 0..512)) {
        let acct = AccountKeys::generate().unwrap();
        let _ = seal::open(&acct.enc_secret, &blob);
    }

    #[test]
    fn envelope_round_trip(pt in proptest::collection::vec(any::<u8>(), 0..1024), version in 1u64..u64::MAX, id in "[a-f0-9-]{1,36}", kind in "[a-z_]{1,12}") {
        let acct = AccountKeys::generate().unwrap();
        let vk = SymmetricKey::generate().unwrap();
        let env = ItemEnvelope::seal(&id, &kind, version, "org", "vk", &pt, &vk, &acct.sign_secret).unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let back: ItemEnvelope = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back.open(&vk, &acct.public().verifying().unwrap()).unwrap(), pt);
    }

    #[test]
    fn envelope_metadata_tampering_is_detected(pt in proptest::collection::vec(any::<u8>(), 0..64), field in 0usize..4) {
        let acct = AccountKeys::generate().unwrap();
        let vk = SymmetricKey::generate().unwrap();
        let mut env = ItemEnvelope::seal("id", "host", 5, "org", "vk", &pt, &vk, &acct.sign_secret).unwrap();
        match field {
            0 => env.id.push('x'),
            1 => env.kind = "key".into(),
            2 => env.version += 1,
            _ => env.org_id.push('x'),
        }
        prop_assert!(env.open(&vk, &acct.public().verifying().unwrap()).is_err());
    }

    #[test]
    fn ssh_config_parser_never_panics(text in "\\PC{0,400}") {
        let _ = ssh_config::parse_text(&text);
    }

    #[test]
    fn ssh_config_structured(alias in "[a-z][a-z0-9-]{0,10}", host in "[a-z0-9.]{1,20}", port in 1u16..) {
        let text = format!("Host {alias}\n  HostName {host}\n  Port {port}\n");
        let parsed = ssh_config::parse_text(&text);
        prop_assert_eq!(parsed.len(), 1);
        prop_assert_eq!(&parsed[0].alias, &alias);
    }

    #[test]
    fn termius_csv_parser_never_panics(text in "\\PC{0,400}") {
        let _ = termius_csv::parse_termius_csv(&text);
    }
}
