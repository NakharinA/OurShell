#![no_main]
//! Sync API bodies, as parsed by both the server and the client.
use libfuzzer_sys::fuzz_target;
use ourshell_proto::sync::*;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<PushRequest>(data);
    let _ = serde_json::from_slice::<PushResponse>(data);
    let _ = serde_json::from_slice::<PullResponse>(data);
    let _ = serde_json::from_slice::<RegisterFinish>(data);
    let _ = serde_json::from_slice::<LoginFinish>(data);
    let _ = serde_json::from_slice::<SessionResponse>(data);
});
