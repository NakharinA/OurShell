//! Wire types shared by OurShell clients and the sync server.

pub mod agent;
pub mod rpc;
pub mod sync;

/// Current version byte of the encrypted blob format (see `docs/blob-format.md`).
pub const BLOB_VERSION: u8 = 1;

/// Version of the local agent JSON-RPC protocol (see `docs/agent-protocol.md`).
pub const AGENT_PROTOCOL_VERSION: u32 = 1;

/// Serde helper: `Vec<u8>` as standard base64.
pub mod b64 {
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

    pub fn encode(v: &[u8]) -> String {
        STANDARD.encode(v)
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
        STANDARD.decode(s)
    }
}
