//! `team` subcommands: create/list team vaults, manage members, share items.

use anyhow::{anyhow, Result};
use clap::Subcommand;
use ourshell_proto::agent::{
    method, ItemRef, TeamCreateParams, TeamInfo, TeamMemberParams, TeamShareParams, TeamShareResult,
};
use serde_json::{json, Value};

use super::{connect_unlocked, friendly};

#[derive(Subcommand)]
pub enum TeamCmd {
    /// Create a team vault.
    Create {
        name: String,
        /// Member email to share it with immediately (repeatable).
        #[arg(long = "member")]
        members: Vec<String>,
    },
    /// List every vault (personal and team) this account can open.
    #[command(alias = "list")]
    Ls,
    /// Add a member to a team vault.
    AddMember { team: String, email: String },
    /// Remove a member from a team vault (rotates its key so the removed
    /// member -- and anyone who hasn't synced -- can't decrypt anything
    /// pushed afterward).
    RemoveMember { team: String, email: String },
    /// Share a saved host into a team vault (moves it under a new id).
    Share { host: String, team: String },
}

pub async fn run(cmd: TeamCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        TeamCmd::Create { name, members } => {
            let info: TeamInfo = c
                .call(method::TEAM_CREATE, &TeamCreateParams { name, members })
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&info)?);
            } else {
                println!(
                    "Created team `{}` (vault id: {}).",
                    info.name, info.vault_id
                );
            }
        }
        TeamCmd::Ls => {
            let teams: Vec<TeamInfo> = c
                .call(method::TEAM_LIST, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&teams)?);
                return Ok(());
            }
            println!("{:<36} {:<20} {:<9} ROLE", "VAULT ID", "NAME", "KIND");
            for t in teams {
                println!("{:<36} {:<20} {:<9} {}", t.vault_id, t.name, t.kind, t.role);
            }
        }
        TeamCmd::AddMember { team, email } => {
            c.call::<_, Value>(
                method::TEAM_ADD_MEMBER,
                &TeamMemberParams {
                    team: team.clone(),
                    email: email.clone(),
                },
            )
            .await
            .map_err(friendly)?;
            if !json_out {
                println!("Added {email} to `{team}`.");
            }
        }
        TeamCmd::RemoveMember { team, email } => {
            c.call::<_, Value>(
                method::TEAM_REMOVE_MEMBER,
                &TeamMemberParams {
                    team: team.clone(),
                    email: email.clone(),
                },
            )
            .await
            .map_err(friendly)?;
            if !json_out {
                println!("Removed {email} from `{team}` and rotated its key.");
            }
        }
        TeamCmd::Share { host, team } => {
            // Sharing only needs the item's id, not its kind -- resolve
            // the host name/id to an id first (works for any host this
            // account can already see, personal or another team's).
            let record: Value = c
                .call(
                    method::ITEM_GET,
                    &ItemRef {
                        kind: "host".into(),
                        id: host.clone(),
                    },
                )
                .await
                .map_err(friendly)?;
            let item_id = record
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("could not resolve host `{host}`"))?
                .to_string();
            let result: TeamShareResult = c
                .call(
                    method::TEAM_SHARE,
                    &TeamShareParams {
                        item_id,
                        team: team.clone(),
                    },
                )
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&result)?);
            } else {
                println!("Shared `{host}` into `{team}` (new id: {}).", result.new_id);
            }
        }
    }
    Ok(())
}
