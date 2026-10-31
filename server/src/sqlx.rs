//! Stand-in for the `sqlx` facade crate: we depend on `sqlx-core` and
//! `sqlx-postgres` directly instead.
//!
//! Why: every integration feature of the `sqlx` facade crate (`chrono`,
//! `uuid`, `migrate`, `macros`, ...) has a *weak* `sqlx-sqlite?/...` edge
//! in its feature definition. Cargo's resolver still needs to find *some*
//! version of the optional `sqlx-sqlite` dependency compatible with those
//! edges even though the feature is never actually activated -- and that
//! conflicts (`links = "sqlite3"`) with `ourshell-core`'s bundled
//! `rusqlite` elsewhere in this workspace, making the whole workspace
//! unbuildable. `sqlx-postgres`'s own `chrono`/`uuid`/`migrate` features
//! have no such edge, so depending on it (and `sqlx-core`) directly
//! sidesteps the conflict entirely.
//!
//! This module re-exports just enough for the rest of this crate to read
//! exactly as if it were written against plain `sqlx` -- every other file
//! does `use crate::sqlx;` and then uses `sqlx::query(..)`,
//! `sqlx::PgPool`, etc. unchanged.

pub use sqlx_core::error::{DatabaseError, Error, Result};
pub use sqlx_core::migrate;
pub use sqlx_core::query::query;
pub use sqlx_core::query_as::query_as;
pub use sqlx_core::sql_str::AssertSqlSafe;
pub use sqlx_postgres::{PgPool, PgPoolOptions};
