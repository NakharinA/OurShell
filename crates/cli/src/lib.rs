//! OurShell CLI library: the local agent daemon and its client.
//!
//! The `ourshell` binary and the `ourshell-mcp` server both build on this crate.

pub mod agent;
pub mod client;
pub mod ipc;
pub mod update;
