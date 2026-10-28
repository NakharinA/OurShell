#![no_main]
//! Agent JSON-RPC framing and every request param type.
use libfuzzer_sys::fuzz_target;
use ourshell_proto::agent::*;
use ourshell_proto::rpc::{Request, ServerMessage};

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<ServerMessage>(data);
    if let Ok(req) = serde_json::from_slice::<Request>(data) {
        let p = req.params;
        let _ = serde_json::from_value::<HelloParams>(p.clone());
        let _ = serde_json::from_value::<PtyOpenParams>(p.clone());
        let _ = serde_json::from_value::<PtyWriteParams>(p.clone());
        let _ = serde_json::from_value::<ExecParams>(p.clone());
        let _ = serde_json::from_value::<SftpWriteParams>(p.clone());
        let _ = serde_json::from_value::<ForwardOpenParams>(p.clone());
        let _ = serde_json::from_value::<ItemPutParams>(p.clone());
        let _ = serde_json::from_value::<DangerousParams>(p);
    }
});
