//! OurShell core library: SSH sessions, client-side crypto and the encrypted vault.
//!
//! Every front-end (CLI, desktop, MCP) reaches this code through the local agent,
//! so the security-critical logic lives in exactly one place.

#![forbid(unsafe_code)]

pub mod crypto;
pub mod import;
pub mod keygen;
pub mod ssh;
pub mod sync;
pub mod vault;
