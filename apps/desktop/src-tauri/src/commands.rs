//! Tauri commands exposed to the webview. These are intentionally thin: the only
//! thing the desktop app's Rust side knows how to do is forward a JSON-RPC call to
//! the agent and hand back whatever it said. All protocol knowledge (methods,
//! params, errors) lives in the TypeScript RPC client (`src/lib/rpc.ts`), which
//! mirrors `crates/proto/src/agent.rs` and `crates/proto/src/rpc.rs`.

use ourshell_proto::rpc::RpcError;
use serde_json::Value;
use tauri::State;

use crate::agent::AgentHandle;

/// Calls one JSON-RPC method on the agent and returns its `result` (or its
/// `error`, which the frontend matches on `.code` per `docs/agent-protocol.md`).
#[tauri::command]
pub async fn rpc_call(
    agent: State<'_, AgentHandle>,
    method: String,
    params: Value,
) -> Result<Value, RpcError> {
    agent.inner().clone().call(method, params).await
}
