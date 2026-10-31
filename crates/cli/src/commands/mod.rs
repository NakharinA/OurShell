//! `ourshell` command-line interface.

mod account;
mod ai;
mod items;
mod org;
mod ssh;
mod team;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ourshell::agent::{self, AgentOptions};
use ourshell::client::{hello, AgentClient};
use ourshell::update;
use ourshell_core::vault::Vault;
use ourshell_proto::agent::{
    method, AccountChangePasswordParams, AccountStatus, ClientKind, SyncParams, SyncReport,
    UnlockParams, VaultInitParams, VaultInitResult, VaultRecoverParams, VaultStatus,
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
    /// Manage host groups (assign hosts with `host add/edit --group`).
    #[command(subcommand)]
    Group(items::GroupCmd),
    /// Approve AI requests, dangerous mode, kill switch and audit log.
    #[command(subcommand)]
    Ai(ai::AiCmd),
    /// Sign up, sign in on a new device, and manage the synced account.
    #[command(subcommand)]
    Account(account::AccountCmd),
    /// Create and manage team vaults.
    #[command(subcommand)]
    Team(team::TeamCmd),
    /// Org administration: invites and user management (admin/owner only).
    #[command(subcommand)]
    Org(org::OrgCmd),
    /// Push local changes and pull remote ones from the sync server.
    Sync,
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
    /// Check for, download and install the latest `ourshell` release from
    /// GitHub (https://github.com/NakharinA/OurShell/releases). Updates
    /// never come from anywhere else.
    Update {
        /// Only report whether an update is available; install nothing.
        /// Exits with code 10 if a newer release exists (handy for scripts),
        /// 0 otherwise.
        #[arg(long)]
        check: bool,
        /// Install without the confirmation prompt.
        #[arg(long)]
        yes: bool,
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
    /// Reset the master password using the recovery phrase shown at `vault init`.
    Recover,
    /// Change the master password (also updates the sync server if signed in).
    Passwd,
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
        Command::Group(cmd) => items::group(cmd, cli.json).await,
        Command::Ai(cmd) => ai::run(cmd, cli.json).await,
        Command::Account(cmd) => account::run(cmd, cli.json).await,
        Command::Team(cmd) => team::run(cmd, cli.json).await,
        Command::Org(cmd) => org::run(cmd, cli.json).await,
        Command::Sync => {
            let c = connect_unlocked().await?;
            let report: SyncReport = c
                .call(method::SYNC_RUN, &SyncParams { server: None })
                .await
                .map_err(friendly)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "pushed {} / pulled {} / conflicts {}",
                    report.pushed, report.pulled, report.conflicts
                );
            }
            Ok(())
        }
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
        Command::Update { check, yes } => {
            if check {
                let available = update::run_check(cli.json).await?;
                if available {
                    std::process::exit(update::EXIT_UPDATE_AVAILABLE);
                }
                Ok(())
            } else {
                update::run_update(yes).await
            }
        }
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
        VaultCmd::Recover => {
            let c = connect().await?;
            let phrase = if let Ok(p) = std::env::var("OURSHELL_RECOVERY_PHRASE") {
                p
            } else {
                print!("Recovery phrase (24 words): ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                line.trim().to_string()
            };
            let new_password = read_secret("New master password: ")?;
            if std::env::var("OURSHELL_PASSWORD").is_err() {
                let again = read_secret("Repeat new master password: ")?;
                if again != new_password {
                    bail!("passwords do not match");
                }
            }
            let result: serde_json::Value = c
                .call(
                    method::VAULT_RECOVER,
                    &VaultRecoverParams {
                        phrase,
                        new_password,
                    },
                )
                .await
                .map_err(friendly)?;
            println!("Vault recovered; master password changed.");

            match result.get("server_updated").and_then(|v| v.as_bool()) {
                Some(true) => println!("Sync server updated; other devices can keep syncing."),
                Some(false) => {
                    let msg = result
                        .get("server_error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("updating the sync server failed for an unknown reason");
                    eprintln!("\nWARNING: {msg}");
                }
                None => {
                    // Not signed in to a server, or an older vault format
                    // that regenerated its account keys locally (see
                    // docs/self-hosting.md's narrowed "known gap").
                    if let Ok(status) = c
                        .call::<_, AccountStatus>(method::ACCOUNT_STATUS, &json!({}))
                        .await
                    {
                        if status.signed_in {
                            eprintln!();
                            eprintln!(
                                "WARNING: this vault is signed in to {} as {}, but its account \
                                 keys could not be pushed there (this vault predates \
                                 recovery-preserves-keys support, so recovering regenerated a new \
                                 signing/encryption keypair). Sync may fail to verify items from \
                                 other devices until you sign out and set up a fresh account. See \
                                 docs/self-hosting.md.",
                                status.server.unwrap_or_default(),
                                status.email.unwrap_or_default()
                            );
                        }
                    }
                }
            }
        }
        VaultCmd::Passwd => {
            let c = connect().await?;
            let old_password = Some(read_secret("Current master password: ")?);
            // Scripts pass the current password in OURSHELL_PASSWORD and the
            // new one in OURSHELL_NEW_PASSWORD.
            let new_password = match std::env::var("OURSHELL_NEW_PASSWORD") {
                Ok(p) if !p.is_empty() => p,
                _ => rpassword::prompt_password("New master password: ")
                    .context("could not read password")?,
            };
            if std::env::var("OURSHELL_NEW_PASSWORD").is_err() {
                let again = read_secret("Repeat new master password: ")?;
                if again != new_password {
                    bail!("passwords do not match");
                }
            }
            let result: serde_json::Value = c
                .call(
                    method::ACCOUNT_CHANGE_PASSWORD,
                    &AccountChangePasswordParams {
                        old_password,
                        new_password,
                    },
                )
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&result)?);
            } else if result.get("server_updated").and_then(|v| v.as_bool()) == Some(true) {
                println!(
                    "Master password changed; sync server updated (other devices signed out)."
                );
            } else {
                println!("Master password changed (not signed in to a sync server).");
            }
        }
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
