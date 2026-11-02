//! Exporting the database into a [`Bundle`]'s tables and loading them back,
//! with Postgres `COPY ... CSV HEADER` over sqlx-postgres's raw copy API.
//!
//! Tables are discovered from the catalog (every ordinary table in the
//! connection's schema except `_sqlx_migrations`), so tables added by later
//! migrations are included without touching this file.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail, ensure, Context, Result};
use futures::TryStreamExt;
use sqlx_postgres::PgConnection;

use super::format::TableInfo;
use crate::sqlx::{self, AssertSqlSafe, PgPool};

const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// What [`export`] read, in one consistent snapshot.
pub struct Export {
    pub migrations: Vec<i64>,
    /// `(table, CSV with header)` in foreign-key-safe load order.
    pub tables: Vec<(String, Vec<u8>)>,
    pub info: Vec<TableInfo>,
}

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

async fn app_tables(conn: &mut PgConnection) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relkind = 'r' AND c.relname <> $1 \
         ORDER BY c.relname",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

/// `(child, parent)` pairs: `child` has a foreign key to `parent`.
async fn foreign_keys(conn: &mut PgConnection) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT src.relname::text, dst.relname::text FROM pg_constraint k \
         JOIN pg_class src ON src.oid = k.conrelid \
         JOIN pg_class dst ON dst.oid = k.confrelid \
         JOIN pg_namespace n ON n.oid = src.relnamespace \
         WHERE k.contype = 'f' AND n.nspname = current_schema()",
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Parents before children (alphabetical among equals, so the order is
/// stable). Cycles and self-references fall back to alphabetical order.
pub fn load_order(tables: &[String], fks: &[(String, String)]) -> Vec<String> {
    let set: BTreeSet<&str> = tables.iter().map(String::as_str).collect();
    let mut deps: BTreeMap<&str, BTreeSet<&str>> =
        set.iter().map(|t| (*t, BTreeSet::new())).collect();
    for (child, parent) in fks {
        if child != parent && set.contains(child.as_str()) && set.contains(parent.as_str()) {
            deps.get_mut(child.as_str())
                .expect("child is a known table")
                .insert(parent.as_str());
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(tables.len());
    let mut done: BTreeSet<&str> = BTreeSet::new();
    while done.len() < set.len() {
        let ready: Vec<&str> = deps
            .iter()
            .filter(|(t, d)| !done.contains(*t) && d.iter().all(|p| done.contains(p)))
            .map(|(t, _)| *t)
            .collect();
        let batch = if ready.is_empty() {
            // A cycle: take the rest alphabetically.
            deps.keys()
                .filter(|t| !done.contains(*t))
                .copied()
                .collect()
        } else {
            ready
        };
        for t in batch {
            done.insert(t);
            out.push(t.to_string());
        }
    }
    out
}

async fn columns(conn: &mut PgConnection, table: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = $1 AND is_generated = 'NEVER' \
         ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

async fn exec(conn: &mut PgConnection, sql: String) -> Result<()> {
    sqlx::query(AssertSqlSafe(sql)).execute(&mut *conn).await?;
    Ok(())
}

/// Reads every application table in one read-only, repeatable-read
/// snapshot. Output is deterministic (rows sorted, UTC timestamps, hex
/// bytea), so an unchanged database exports byte-for-byte the same.
pub async fn export(pool: &PgPool) -> Result<Export> {
    let mut tx = pool.begin().await?;
    let conn: &mut PgConnection = &mut tx;
    exec(
        conn,
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY".into(),
    )
    .await?;
    for set in [
        "SET LOCAL TimeZone = 'UTC'",
        "SET LOCAL DateStyle = 'ISO, MDY'",
        "SET LOCAL IntervalStyle = 'postgres'",
        "SET LOCAL bytea_output = 'hex'",
        "SET LOCAL extra_float_digits = 3",
    ] {
        exec(conn, set.into()).await?;
    }
    let migrations: Vec<(i64,)> =
        sqlx::query_as("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&mut *conn)
            .await
            .context("can't read _sqlx_migrations")?;
    let fks = foreign_keys(conn).await?;
    let order = load_order(&app_tables(conn).await?, &fks);

    let mut tables = Vec::new();
    let mut info = Vec::new();
    for table in order {
        let cols = columns(conn, &table).await?;
        ensure!(!cols.is_empty(), "table {table} has no columns");
        let list = cols
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let order_by = cols
            .iter()
            .map(|c| format!("{}::text COLLATE \"C\"", quote_ident(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let t = quote_ident(&table);
        let (rows,): (i64,) = sqlx::query_as(AssertSqlSafe(format!("SELECT COUNT(*) FROM {t}")))
            .fetch_one(&mut *conn)
            .await?;
        let sql = format!(
            "COPY (SELECT {list} FROM {t} ORDER BY {order_by}) TO STDOUT WITH (FORMAT csv, HEADER true)"
        );
        let mut data = Vec::new();
        {
            let mut stream = conn.copy_out_raw(&sql).await?;
            while let Some(chunk) = stream.try_next().await? {
                data.extend_from_slice(&chunk);
            }
        }
        info.push(TableInfo {
            name: table.clone(),
            rows: rows as u64,
        });
        tables.push((table, data));
    }
    tx.commit().await?;
    Ok(Export {
        migrations: migrations.into_iter().map(|(v,)| v).collect(),
        tables,
        info,
    })
}

/// The first CSV record (the header row): column names.
pub fn csv_header(data: &[u8]) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    let mut cur = Vec::new();
    let mut quoted = false;
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if quoted {
            if b == b'"' {
                if data.get(i + 1) == Some(&b'"') {
                    cur.push(b'"');
                    i += 1;
                } else {
                    quoted = false;
                }
            } else {
                cur.push(b);
            }
        } else {
            match b {
                b'"' => quoted = true,
                b',' => fields.push(std::mem::take(&mut cur)),
                b'\n' => break,
                b'\r' => {}
                _ => cur.push(b),
            }
        }
        i += 1;
    }
    ensure!(!quoted, "unterminated quote in a CSV header");
    fields.push(cur);
    fields
        .into_iter()
        .map(|f| String::from_utf8(f).map_err(|_| anyhow!("CSV header isn't UTF-8")))
        .collect()
}

/// Replaces every application table's rows with the backup's. Runs on the
/// caller's transaction; columns are matched by name, so a backup from an
/// older schema (missing newer nullable/defaulted columns) loads fine.
/// Returns `(table, rows loaded)`.
pub async fn import(
    conn: &mut PgConnection,
    tables: &[(String, Vec<u8>)],
) -> Result<Vec<(String, u64)>> {
    let existing = app_tables(conn).await?;
    for (name, _) in tables {
        if !existing.contains(name) {
            bail!("the backup has a table {name:?} that this server's database doesn't");
        }
    }
    if !existing.is_empty() {
        let all = existing
            .iter()
            .map(|t| quote_ident(t))
            .collect::<Vec<_>>()
            .join(", ");
        exec(conn, format!("TRUNCATE {all} RESTART IDENTITY CASCADE")).await?;
    }
    let mut loaded = Vec::new();
    for (name, data) in tables {
        let header = csv_header(data).with_context(|| format!("table {name}"))?;
        let target = columns(conn, name).await?;
        for c in &header {
            if !target.contains(c) {
                bail!("the backup's table {name} has a column {c:?} that this server's database doesn't");
            }
        }
        let list = header
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "COPY {} ({list}) FROM STDIN WITH (FORMAT csv, HEADER true)",
            quote_ident(name)
        );
        let mut copy = conn.copy_in_raw(&sql).await?;
        copy.send(data.as_slice()).await?;
        let rows = copy
            .finish()
            .await
            .with_context(|| format!("loading table {name}"))?;
        loaded.push((name.clone(), rows));
    }
    // Serial / identity columns continue after the restored rows.
    let seqs: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text, \
                pg_get_serial_sequence(format('%I.%I', table_schema, table_name), column_name) \
         FROM information_schema.columns WHERE table_schema = current_schema() \
           AND pg_get_serial_sequence(format('%I.%I', table_schema, table_name), column_name) IS NOT NULL",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (table, column, seq) in seqs {
        sqlx::query(AssertSqlSafe(format!(
            "SELECT setval($1, COALESCE((SELECT MAX({}) FROM {}), 0) + 1, false)",
            quote_ident(&column),
            quote_ident(&table)
        )))
        .bind(seq)
        .execute(&mut *conn)
        .await?;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parents_load_first() {
        let tables = s(&["items", "orgs", "users", "vaults", "solo"]);
        let fks = vec![
            ("users".to_string(), "orgs".to_string()),
            ("vaults".to_string(), "orgs".to_string()),
            ("items".to_string(), "vaults".to_string()),
            ("items".to_string(), "orgs".to_string()),
            ("users".to_string(), "users".to_string()),
        ];
        assert_eq!(
            load_order(&tables, &fks),
            s(&["orgs", "solo", "users", "vaults", "items"])
        );
        // A cycle still yields every table once.
        let cyc = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ];
        assert_eq!(load_order(&s(&["b", "a", "c"]), &cyc), s(&["c", "a", "b"]));
    }

    #[test]
    fn reads_csv_headers() {
        assert_eq!(
            csv_header(b"id,\"we\"\"ird, name\",x\r\n1,2,3\n").unwrap(),
            s(&["id", "we\"ird, name", "x"])
        );
        assert_eq!(csv_header(b"only").unwrap(), s(&["only"]));
        assert!(csv_header(b"\"open").is_err());
    }
}
