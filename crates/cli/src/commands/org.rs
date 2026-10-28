//! `org` subcommands: invites and admin user management.

use anyhow::Result;
use clap::Subcommand;
use ourshell_proto::agent::{method, OrgDisableUserParams, OrgInviteParams, OrgInviteResult};
use ourshell_proto::sync::OrgUserInfo;
use serde_json::{json, Value};

use super::{connect_unlocked, friendly};

#[derive(Subcommand)]
pub enum OrgCmd {
    /// Create a single-use invite code (admin/owner only).
    Invite {
        /// "admin" or "member" (default).
        #[arg(long)]
        role: Option<String>,
    },
    /// List every user in the org (admin/owner only).
    Users,
    /// Disable a user's account and kill its sessions (admin/owner only).
    Disable { user_id: String },
}

pub async fn run(cmd: OrgCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        OrgCmd::Invite { role } => {
            let res: OrgInviteResult = c
                .call(method::ORG_INVITE, &OrgInviteParams { role })
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&res)?);
            } else {
                println!("Invite code: {}", res.code);
            }
        }
        OrgCmd::Users => {
            let users: Vec<OrgUserInfo> = c
                .call(method::ORG_USERS, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&users)?);
                return Ok(());
            }
            println!("{:<36} {:<30} {:<8} DISABLED", "USER ID", "EMAIL", "ROLE");
            for u in users {
                println!(
                    "{:<36} {:<30} {:<8} {}",
                    u.user_id,
                    u.email,
                    format!("{:?}", u.role),
                    u.disabled
                );
            }
        }
        OrgCmd::Disable { user_id } => {
            c.call::<_, Value>(
                method::ORG_DISABLE_USER,
                &OrgDisableUserParams {
                    user_id: user_id.clone(),
                },
            )
            .await
            .map_err(friendly)?;
            if !json_out {
                println!("Disabled user `{user_id}`.");
            }
        }
    }
    Ok(())
}
