#![no_main]
//! Sealed boxes come from the server; opening garbage must fail cleanly.
use libfuzzer_sys::fuzz_target;
use ourshell_core::crypto::keys::AccountKeys;
use ourshell_core::crypto::seal;
use std::sync::OnceLock;

static KEYS: OnceLock<AccountKeys> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let keys = KEYS.get_or_init(|| AccountKeys::generate().unwrap());
    let _ = seal::open(&keys.enc_secret, data);
});
