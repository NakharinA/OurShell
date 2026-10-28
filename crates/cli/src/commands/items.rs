//! `host`, `identity` and `snippet` subcommands.

use std::collections::HashMap;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use ourshell_proto::agent::{method, ItemListParams, ItemPutParams, ItemRecord, ItemRef, TeamInfo};
use serde_json::{json, Map, Value};

use super::{connect_unlocked, friendly, read_secret};
use ourshell::client::AgentClient;

/// `vault_id -> display name` for every vault this account can open, so
/// listings can show a human name instead of a raw id.
async fn vault_names(c: &AgentClient) -> Result<HashMap<String, String>> {
    let teams: Vec<TeamInfo> = c
        .call(method::TEAM_LIST, &json!({}))
        .await
        .map_err(friendly)?;
    Ok(teams.into_iter().map(|t| (t.vault_id, t.name)).collect())
}

#[derive(Subcommand)]
pub enum HostCmd {
    /// Add a host.
    Add(HostArgs),
    /// List hosts.
    #[command(alias = "list")]
    Ls,
    /// Show one host.
    Show { host: String },
    /// Edit a host (only the given fields change).
    Edit {
        host: String,
        #[command(flatten)]
        args: HostEditArgs,
    },
    /// Remove a host.
    Rm { host: String },
}

#[derive(Args)]
pub struct HostArgs {
    /// Display name used with `connect`.
    pub name: String,
    /// Hostname or IP. `user@host:port` is accepted.
    pub address: String,
    #[command(flatten)]
    pub opts: HostEditArgs,
}

#[derive(Args, Default)]
pub struct HostEditArgs {
    #[arg(short, long)]
    pub port: Option<u16>,
    #[arg(short, long)]
    pub user: Option<String>,
    /// Identity name or id to log in with.
    #[arg(short, long)]
    pub identity: Option<String>,
    /// Jump host name (ProxyJump).
    #[arg(short = 'J', long)]
    pub jump: Option<String>,
    /// Comma-separated tags.
    #[arg(long)]
    pub tags: Option<String>,
    /// Group name or id.
    #[arg(long)]
    pub group: Option<String>,
    /// Allow AI tools (MCP) to see and use this host.
    #[arg(long)]
    pub ai_allowed: Option<bool>,
    /// Log in as this user for AI-initiated sessions.
    #[arg(long)]
    pub ai_user: Option<String>,
    #[arg(long)]
    pub notes: Option<String>,
    /// New hostname (edit only).
    #[arg(long)]
    pub hostname: Option<String>,
    /// New name (edit only).
    #[arg(long)]
    pub rename: Option<String>,
}

#[derive(Subcommand)]
pub enum IdentityCmd {
    /// Add an identity; prompts for a password unless --no-password.
    Add {
        name: String,
        #[arg(short, long)]
        user: Option<String>,
        /// Key name or id to use.
        #[arg(short, long)]
        key: Option<String>,
        #[arg(long)]
        no_password: bool,
    },
    #[command(alias = "list")]
    Ls,
    Rm {
        identity: String,
    },
}

#[derive(Subcommand)]
pub enum SnippetCmd {
    Add {
        name: String,
        script: String,
    },
    #[command(alias = "list")]
    Ls,
    Show {
        snippet: String,
    },
    Rm {
        snippet: String,
    },
}

/// Splits `user@host:port`.
pub fn parse_address(addr: &str) -> (Option<String>, String, Option<u16>) {
    let (user, rest) = match addr.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, addr),
    };
    // Bracketed IPv6: [::1]:22
    if let Some(stripped) = rest.strip_prefix('[') {
        if let Some((h, p)) = stripped.split_once(']') {
            let port = p.strip_prefix(':').and_then(|p| p.parse().ok());
            return (user, h.to_string(), port);
        }
    }
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => match p.parse() {
            Ok(port) => (user, h.to_string(), Some(port)),
            Err(_) => (user, rest.to_string(), None),
        },
        _ => (user, rest.to_string(), None),
    }
}

async fn resolve_id(c: &ourshell::client::AgentClient, kind: &str, name: &str) -> Result<String> {
    let rec: ItemRecord = c
        .call(
            method::ITEM_GET,
            &ItemRef {
                kind: kind.into(),
                id: name.into(),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("{kind} `{name}`: {}", friendly(e)))?;
    Ok(rec.id)
}

async fn apply_edits(
    c: &ourshell::client::AgentClient,
    obj: &mut Map<String, Value>,
    a: &HostEditArgs,
) -> Result<()> {
    let mut set = |k: &str, v: Value| {
        obj.insert(k.into(), v);
    };
    if let Some(p) = a.port {
        set("port", json!(p));
    }
    if let Some(u) = &a.user {
        set("username", json!(u));
    }
    if let Some(j) = &a.jump {
        set(
            "jump_host",
            if j.is_empty() { Value::Null } else { json!(j) },
        );
    }
    if let Some(t) = &a.tags {
        let tags: Vec<&str> = t
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        set("tags", json!(tags));
    }
    if let Some(b) = a.ai_allowed {
        set("ai_allowed", json!(b));
    }
    if let Some(u) = &a.ai_user {
        set("ai_user", if u.is_empty() { Value::Null } else { json!(u) });
    }
    if let Some(n) = &a.notes {
        set("notes", json!(n));
    }
    if let Some(h) = &a.hostname {
        set("hostname", json!(h));
    }
    if let Some(n) = &a.rename {
        set("name", json!(n));
    }
    if let Some(i) = &a.identity {
        let id = if i.is_empty() {
            Value::Null
        } else {
            json!(resolve_id(c, "identity", i).await?)
        };
        obj.insert("identity_id".into(), id);
    }
    if let Some(g) = &a.group {
        let id = if g.is_empty() {
            Value::Null
        } else {
            json!(resolve_id(c, "group", g).await?)
        };
        obj.insert("group_id".into(), id);
    }
    Ok(())
}

async fn list(c: &ourshell::client::AgentClient, kind: &str) -> Result<Vec<ItemRecord>> {
    c.call(method::ITEM_LIST, &ItemListParams { kind: kind.into() })
        .await
        .map_err(friendly)
}

async fn get(c: &ourshell::client::AgentClient, kind: &str, id: &str) -> Result<ItemRecord> {
    c.call(
        method::ITEM_GET,
        &ItemRef {
            kind: kind.into(),
            id: id.into(),
        },
    )
    .await
    .map_err(friendly)
}

async fn put(
    c: &ourshell::client::AgentClient,
    kind: &str,
    id: Option<String>,
    item: Value,
) -> Result<String> {
    let v: Value = c
        .call(
            method::ITEM_PUT,
            &ItemPutParams {
                kind: kind.into(),
                id,
                item,
            },
        )
        .await
        .map_err(friendly)?;
    Ok(v["id"].as_str().unwrap_or_default().to_string())
}

async fn rm(c: &ourshell::client::AgentClient, kind: &str, id: &str) -> Result<()> {
    c.call::<_, Value>(
        method::ITEM_DELETE,
        &ItemRef {
            kind: kind.into(),
            id: id.into(),
        },
    )
    .await
    .map_err(friendly)?;
    Ok(())
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub async fn host(cmd: HostCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        HostCmd::Add(a) => {
            let (user, hostname, port) = parse_address(&a.address);
            let mut obj = Map::new();
            obj.insert("name".into(), json!(a.name));
            obj.insert("hostname".into(), json!(hostname));
            obj.insert("port".into(), json!(port.unwrap_or(22)));
            if let Some(u) = user {
                obj.insert("username".into(), json!(u));
            }
            apply_edits(&c, &mut obj, &a.opts).await?;
            let id = put(&c, "host", None, Value::Object(obj)).await?;
            if json_out {
                println!("{}", json!({ "id": id }));
            } else {
                println!("Added host `{}` ({id}).", a.name);
            }
        }
        HostCmd::Ls => {
            let hosts = list(&c, "host").await?;
            if json_out {
                println!("{}", serde_json::to_string(&hosts)?);
                return Ok(());
            }
            if hosts.is_empty() {
                println!("No hosts. Add one with `ourshell host add <name> <user@host>`.");
                return Ok(());
            }
            let names = vault_names(&c).await.unwrap_or_default();
            println!(
                "{:<20} {:<32} {:<6} {:<12} {:<4} {:<12}",
                "NAME", "ADDRESS", "PORT", "USER", "AI", "VAULT"
            );
            for h in hosts {
                let i = &h.item;
                let vault = h
                    .vault_id
                    .as_deref()
                    .and_then(|v| names.get(v))
                    .map(String::as_str)
                    .unwrap_or("Personal");
                println!(
                    "{:<20} {:<32} {:<6} {:<12} {:<4} {:<12}",
                    s(i, "name"),
                    s(i, "hostname"),
                    i["port"].as_u64().unwrap_or(22),
                    s(i, "username"),
                    if i["ai_allowed"].as_bool().unwrap_or(false) {
                        "yes"
                    } else {
                        ""
                    },
                    vault
                );
            }
        }
        HostCmd::Show { host } => {
            let h = get(&c, "host", &host).await?;
            println!("{}", serde_json::to_string_pretty(&h)?);
        }
        HostCmd::Edit { host, args } => {
            let h = get(&c, "host", &host).await?;
            let Value::Object(mut obj) = h.item else {
                bail!("corrupt host record");
            };
            apply_edits(&c, &mut obj, &args).await?;
            put(&c, "host", Some(h.id), Value::Object(obj)).await?;
            if !json_out {
                println!("Updated host `{host}`.");
            }
        }
        HostCmd::Rm { host } => {
            rm(&c, "host", &host).await?;
            if !json_out {
                println!("Removed host `{host}`.");
            }
        }
    }
    Ok(())
}

pub async fn identity(cmd: IdentityCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        IdentityCmd::Add {
            name,
            user,
            key,
            no_password,
        } => {
            let password = if no_password {
                None
            } else {
                let p =
                    read_secret_named("OURSHELL_HOST_PASSWORD", "SSH password (empty for none): ")?;
                (!p.is_empty()).then_some(p)
            };
            let key_id = match key {
                Some(k) => Some(resolve_id(&c, "key", &k).await?),
                None => None,
            };
            let id = put(
                &c,
                "identity",
                None,
                json!({"name": name, "username": user, "password": password, "key_id": key_id}),
            )
            .await?;
            if json_out {
                println!("{}", json!({ "id": id }));
            } else {
                println!("Added identity `{name}`.");
            }
        }
        IdentityCmd::Ls => {
            let items = list(&c, "identity").await?;
            if json_out {
                println!("{}", serde_json::to_string(&items)?);
                return Ok(());
            }
            println!(
                "{:<20} {:<16} {:<8} {:<8}",
                "NAME", "USER", "PASSWORD", "KEY"
            );
            for r in items {
                let i = &r.item;
                println!(
                    "{:<20} {:<16} {:<8} {:<8}",
                    s(i, "name"),
                    s(i, "username"),
                    if i["password"].is_null() { "" } else { "yes" },
                    if i["key_id"].is_null() { "" } else { "yes" }
                );
            }
        }
        IdentityCmd::Rm { identity } => {
            rm(&c, "identity", &identity).await?;
            if !json_out {
                println!("Removed identity `{identity}`.");
            }
        }
    }
    Ok(())
}

fn read_secret_named(env: &str, prompt: &str) -> Result<String> {
    if let Ok(v) = std::env::var(env) {
        return Ok(v);
    }
    if std::env::var("OURSHELL_PASSWORD").is_ok() {
        // Non-interactive mode without a host password: none.
        return Ok(String::new());
    }
    read_secret(prompt)
}

pub async fn snippet(cmd: SnippetCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        SnippetCmd::Add { name, script } => {
            put(&c, "snippet", None, json!({"name": name, "script": script})).await?;
            if !json_out {
                println!("Added snippet `{name}`.");
            }
        }
        SnippetCmd::Ls => {
            let items = list(&c, "snippet").await?;
            if json_out {
                println!("{}", serde_json::to_string(&items)?);
                return Ok(());
            }
            for r in items {
                let first = s(&r.item, "script").lines().next().unwrap_or("");
                println!("{:<20} {}", s(&r.item, "name"), first);
            }
        }
        SnippetCmd::Show { snippet } => {
            let r = get(&c, "snippet", &snippet).await?;
            println!("{}", s(&r.item, "script"));
        }
        SnippetCmd::Rm { snippet } => {
            rm(&c, "snippet", &snippet).await?;
            if !json_out {
                println!("Removed snippet `{snippet}`.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_address;

    #[test]
    fn addresses() {
        assert_eq!(
            parse_address("root@10.0.0.1:2222"),
            (Some("root".into()), "10.0.0.1".into(), Some(2222))
        );
        assert_eq!(
            parse_address("example.com"),
            (None, "example.com".into(), None)
        );
        assert_eq!(parse_address("[::1]:22"), (None, "::1".into(), Some(22)));
        assert_eq!(parse_address("fe80::1"), (None, "fe80::1".into(), None));
    }
}
