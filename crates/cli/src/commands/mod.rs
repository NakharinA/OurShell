//! `ourshell` command-line interface.

mod items;
mod ssh;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ourshell::agent::{self, AgentOptions};
use ourshell::client::{hello, AgentClient};
use ourshell_core::vault::Vault;
use ourshell_proto::agent::{
    method, ClientKind, UnlockParams, VaultInitParams, VaultInitResult, VaultStatus,
};
use ourshell_proto::rpc;
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "ourshell",
    version,
    about = "Self-hostable, open-source SSH client"
)]
pub struct Cli {
    /// Print machine-readable JSON instead of tables.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create, unlock and lock the encrypted vault.
    #[command(subcommand)]
    Vault(VaultCmd),
    /// Manage saved hosts.
    #[command(subcommand)]
    Host(items::HostCmd),
    /// Manage identities (username + password and/or key).
    #[command(subcommand)]
    Identity(items::IdentityCmd),
    /// Manage snippets.
    #[command(subcommand)]
    Snippet(items::SnippetCmd),
    /// Open an interactive shell on a saved host.
    Connect {
        host: String,
        /// Forward your local ssh-agent to the host.
        #[arg(short = 'A', long)]
        agent_forwarding: bool,
    },
    /// Run one command on a saved host.
    Exec {
        host: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Transfer files with SFTP.
    #[command(subcommand)]
    Sftp(ssh::SftpCmd),
    /// Port forwarding.
    #[command(subcommand)]
    Forward(ssh::ForwardCmd),
    /// Generate, import and list SSH keys.
    #[command(subcommand)]
    Key(ssh::KeyCmd),
    /// Import hosts from ~/.ssh/config or a Termius CSV.
    #[command(subcommand)]
    Import(ssh::ImportCmd),
    /// Run the local agent in the foreground (normally started on demand).
    Agent {
        /// Vault file (default: ~/.ourshell/vault.db or $OURSHELL_VAULT).
        #[arg(long, env = "OURSHELL_VAULT")]
        vault: Option<PathBuf>,
        /// Keep running even when idle and locked.
        #[arg(long)]
        no_idle_exit: bool,
    },
}

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Create a new vault.
    Init {
        /// Never sync this vault to a server.
        #[arg(long)]
        local_only: bool,
    },
    /// Unlock the vault in the agent.
    Unlock {
        /// Auto-lock after this many idle minutes.
        #[arg(long, default_value_t = 15)]
        timeout: u64,
    },
    /// Lock the vault now.
    Lock,
    /// Show vault and agent status.
    Status,
    /// Stop the agent (locks the vault).
    StopAgent,
}

/// Reads a secret from `$OURSHELL_PASSWORD` (for scripts and tests) or the terminal.
pub fn read_secret(prompt: &str) -> Result<String> {
    if let Ok(p) = std::env::var("OURSHELL_PASSWORD") {
        return Ok(p);
    }
    rpassword::prompt_password(prompt).context("could not read password")
}

pub async fn connect() -> Result<AgentClient> {
    let (c, _events) = AgentClient::connect_or_spawn(hello(ClientKind::Cli)).await?;
    Ok(c)
}

/// Connects and makes sure the vault is unlocked, prompting if needed.
pub async fn connect_unlocked() -> Result<AgentClient> {
    let c = connect().await?;
    let st: VaultStatus = c.call(method::VAULT_STATUS, &json!({})).await?;
    if !st.exists {
        bail!("no vault yet; run `ourshell vault init`");
    }
    if !st.unlocked {
        unlock(&c, 15).await?;
    }
    Ok(c)
}

async fn unlock(c: &AgentClient, minutes: u64) -> Result<()> {
    let password = read_secret("Master password: ")?;
    c.call::<_, serde_json::Value>(
        method::VAULT_UNLOCK,
        &UnlockParams {
            password,
            auto_lock_secs: Some(minutes * 60),
        },
    )
    .await?;
    Ok(())
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Agent {
            vault,
            no_idle_exit,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_env("OURSHELL_LOG")
                        .unwrap_or_else(|_| "info".into()),
                )
                .with_writer(std::io::stderr)
                .init();
            agent::run(AgentOptions {
                vault_path: vault.unwrap_or_else(Vault::default_path),
                idle_exit: (!no_idle_exit)
                    .then(|| std::time::Duration::from_secs(agent::DEFAULT_IDLE_EXIT_SECS)),
            })
            .await
        }
        Command::Vault(cmd) => vault_cmd(cmd, cli.json).await,
        Command::Host(cmd) => items::host(cmd, cli.json).await,
        Command::Identity(cmd) => items::identity(cmd, cli.json).await,
        Command::Snippet(cmd) => items::snippet(cmd, cli.json).await,
        Command::Connect {
            host,
            agent_forwarding,
        } => ssh::connect(host, agent_forwarding).await,
        Command::Exec {
            host,
            command,
            timeout,
        } => ssh::exec(host, command, timeout, cli.json).await,
        Command::Sftp(cmd) => ssh::sftp(cmd, cli.json).await,
        Command::Forward(cmd) => ssh::forward(cmd, cli.json).await,
        Command::Key(cmd) => ssh::key(cmd, cli.json).await,
        Command::Import(cmd) => ssh::import(cmd, cli.json).await,
    }
}

async fn vault_cmd(cmd: VaultCmd, json_out: bool) -> Result<()> {
    match cmd {
        VaultCmd::Init { local_only } => {
            let c = connect().await?;
            let st: VaultStatus = c.call(method::VAULT_STATUS, &json!({})).await?;
            if st.exists {
                bail!("a vault already exists at {}", st.path);
            }
            let password = read_secret("New master password: ")?;
            if std::env::var("OURSHELL_PASSWORD").is_err() {
                let again = read_secret("Repeat master password: ")?;
                if again != password {
                    bail!("passwords do not match");
                }
            }
            let res: VaultInitResult = c
                .call(
                    method::VAULT_INIT,
                    &VaultInitParams {
                        password,
                        local_only,
                    },
                )
                .await?;
            if json_out {
                println!("{}", serde_json::to_string(&res)?);
            } else {
                println!("Vault created at {}.\n", st.path);
                println!("RECOVERY PHRASE (shown once, write it down and keep it safe):\n");
                println!("    {}\n", res.recovery_phrase);
                println!(
                    "If you forget your master password AND lose this phrase, your data cannot be recovered."
                );
            }
        }
        VaultCmd::Unlock { timeout } => {
            let c = connect().await?;
            unlock(&c, timeout).await?;
            if !json_out {
                println!("Vault unlocked (auto-lock after {timeout} idle minutes).");
            }
        }
        VaultCmd::Lock => {
            let c = connect().await?;
            c.call::<_, serde_json::Value>(method::VAULT_LOCK, &json!({}))
                .await?;
            if !json_out {
                println!("Vault locked.");
            }
        }
        VaultCmd::Status => {
            let c = connect().await?;
            let st: VaultStatus = c.call(method::VAULT_STATUS, &json!({})).await?;
            if json_out {
                println!("{}", serde_json::to_string(&st)?);
            } else {
                println!("vault:    {}", st.path);
                println!("exists:   {}", st.exists);
                println!(
                    "state:    {}",
                    if st.unlocked { "unlocked" } else { "locked" }
                );
                if let Some(s) = st.auto_lock_in {
                    println!("autolock: in {}m{}s", s / 60, s % 60);
                }
            }
        }
        VaultCmd::StopAgent => match AgentClient::connect_existing(hello(ClientKind::Cli)).await {
            Ok((c, _)) => {
                let _ = c
                    .call::<_, serde_json::Value>(method::SHUTDOWN, &json!({}))
                    .await;
                println!("Agent stopped.");
            }
            Err(_) => println!("Agent is not running."),
        },
    }
    Ok(())
}

/// Turns an RPC "locked" error into a friendlier message.
pub fn friendly(e: ourshell::client::CallError) -> anyhow::Error {
    match e.code() {
        Some(rpc::VAULT_LOCKED) => anyhow::anyhow!("vault is locked; run `ourshell vault unlock`"),
        _ => e.into(),
    }
}
