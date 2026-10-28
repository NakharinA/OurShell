#![no_main]
//! Item envelopes arrive from the sync server; parsing and opening must never panic.
use libfuzzer_sys::fuzz_target;
use ourshell_core::crypto::keys::{AccountKeys, ItemEnvelope};
use ourshell_core::crypto::SymmetricKey;
use std::sync::OnceLock;

static KEYS: OnceLock<AccountKeys> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let keys = KEYS.get_or_init(|| AccountKeys::generate().unwrap());
    if let Ok(env) = serde_json::from_slice::<ItemEnvelope>(data) {
        let vk = SymmetricKey::from_bytes([1u8; 32]);
        let _ = env.signing_bytes();
        let _ = env.open(&vk, &keys.public().verifying().unwrap());
    }
});
