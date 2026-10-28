//! Wire types shared by OurShell clients and the sync server.

/// Current version byte of the encrypted blob format (see `docs/blob-format.md`).
pub const BLOB_VERSION: u8 = 1;

/// Version of the local agent JSON-RPC protocol (see `docs/agent-protocol.md`).
pub const AGENT_PROTOCOL_VERSION: u32 = 1;
