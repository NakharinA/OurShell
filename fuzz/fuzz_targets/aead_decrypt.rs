#![no_main]
//! The v1 ciphertext container parser must reject garbage without panicking.
use libfuzzer_sys::fuzz_target;
use ourshell_core::crypto::{aead, SymmetricKey};

fuzz_target!(|data: &[u8]| {
    let key = SymmetricKey::from_bytes([7u8; 32]);
    let _ = aead::decrypt(&key, data, b"fuzz");
});
