//! `account` subcommands: signup, login, logout, status, devices, totp.

use anyhow::Result;
use clap::Subcommand;
use ourshell_proto::agent::{
    method, AccountLoginParams, AccountSignupParams, AccountStatus, DeviceRef, TotpCodeParams,
};
use ourshell_proto::sync::{DeviceInfo, TotpSetupResponse};
use serde_json::{json, Value};

use super::{connect, connect_unlocked, friendly, read_secret};

#[derive(Subcommand)]
pub enum AccountCmd {
    /// Register a new account on a sync server, uploading this vault's
    /// existing key material (run `ourshell vault init` first).
    Signup {
        /// Sync server base URL, e.g. https://ourshell.example.com.
        #[arg(long, env = "OURSHELL_SERVER")]
        server: String,
        #[arg(long)]
        email: String,
        /// Required if the server has INVITE_ONLY set.
        #[arg(long)]
        invite_code: Option<String>,
        /// Defaults to this machine's hostname.
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Sign in on a new device. Requires no local vault, or an empty one
    /// (it is replaced with one built from the server's material).
    Login {
        #[arg(long, env = "OURSHELL_SERVER")]
        server: String,
        #[arg(long)]
        email: String,
        /// Required if the account has two-factor authentication enabled.
        #[arg(long)]
        totp: Option<String>,
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Sign out of the sync server on this device.
    Logout,
    /// Show whether this device is signed in, and to where.
    Status,
    /// List every device registered to this account.
    Devices,
    /// Revoke a device by id (also ends its sessions).
    Revoke { device_id: String },
    /// Manage two-factor authentication (TOTP) for this account.
    #[command(subcommand)]
    Totp(TotpCmd),
}

#[derive(Subcommand)]
pub enum TotpCmd {
    /// Start enrolling an authenticator app.
    Setup,
    /// Finish enrolling with a code from the app.
    Confirm { code: String },
    /// Turn two-factor authentication off.
    Disable { code: String },
}

fn print_status(s: &AccountStatus, json_out: bool) -> Result<()> {
    if json_out {
        println!("{}", serde_json::to_string(s)?);
        return Ok(());
    }
    if s.signed_in {
        println!(
            "signed in as {} on {}",
            s.email.as_deref().unwrap_or("?"),
            s.server.as_deref().unwrap_or("?")
        );
        println!("device:    {}", s.device_id.as_deref().unwrap_or("?"));
        println!("last sync: {}", s.last_sync.as_deref().unwrap_or("never"));
    } else {
        println!("not signed in to a sync server");
    }
    Ok(())
}

pub async fn run(cmd: AccountCmd, json_out: bool) -> Result<()> {
    match cmd {
        AccountCmd::Signup {
            server,
            email,
            invite_code,
            device_name,
        } => {
            let c = connect_unlocked().await?;
            let password = read_secret("Master password (to prove you own this vault): ")?;
            let res: AccountStatus = c
                .call(
                    method::ACCOUNT_SIGNUP,
                    &AccountSignupParams {
                        server,
                        email,
                        password,
                        invite_code,
                        device_name,
                    },
                )
                .await
                .map_err(friendly)?;
            print_status(&res, json_out)?;
        }
        AccountCmd::Login {
            server,
            email,
            totp,
            device_name,
        } => {
            let c = connect().await?;
            let password = read_secret("Master password: ")?;
            let res: AccountStatus = c
                .call(
                    method::ACCOUNT_LOGIN,
                    &AccountLoginParams {
                        server,
                        email,
                        password,
                        totp,
                        device_name,
                    },
                )
                .await
                .map_err(friendly)?;
            print_status(&res, json_out)?;
        }
        AccountCmd::Logout => {
            let c = connect_unlocked().await?;
            c.call::<_, Value>(method::ACCOUNT_LOGOUT, &json!({}))
                .await
                .map_err(friendly)?;
            if !json_out {
                println!("Signed out.");
            }
        }
        AccountCmd::Status => {
            let c = connect_unlocked().await?;
            let res: AccountStatus = c
                .call(method::ACCOUNT_STATUS, &json!({}))
                .await
                .map_err(friendly)?;
            print_status(&res, json_out)?;
        }
        AccountCmd::Devices => {
            let c = connect_unlocked().await?;
            let devices: Vec<DeviceInfo> = c
                .call(method::ACCOUNT_DEVICES, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&devices)?);
                return Ok(());
            }
            println!("{:<36} {:<20} {:<8} CREATED", "ID", "NAME", "REVOKED");
            for d in devices {
                println!(
                    "{:<36} {:<20} {:<8} {}",
                    d.device_id, d.name, d.revoked, d.created_at
                );
            }
        }
        AccountCmd::Revoke { device_id } => {
            let c = connect_unlocked().await?;
            c.call::<_, Value>(
                method::ACCOUNT_REVOKE_DEVICE,
                &DeviceRef {
                    device_id: device_id.clone(),
                },
            )
            .await
            .map_err(friendly)?;
            if !json_out {
                println!("Revoked device `{device_id}`.");
            }
        }
        AccountCmd::Totp(cmd) => totp(cmd, json_out).await?,
    }
    Ok(())
}

async fn totp(cmd: TotpCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        TotpCmd::Setup => {
            let res: TotpSetupResponse = c
                .call(method::ACCOUNT_TOTP_SETUP, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&res)?);
                return Ok(());
            }
            println!("Scan this into your authenticator app, then run:");
            println!("    ourshell account totp confirm <code>\n");
            println!("    {}\n", res.otpauth_url);
            println!("Manual entry secret: {}", res.secret_base32);
        }
        TotpCmd::Confirm { code } => {
            c.call::<_, Value>(method::ACCOUNT_TOTP_CONFIRM, &TotpCodeParams { code })
                .await
                .map_err(friendly)?;
            if !json_out {
                println!("Two-factor authentication enabled.");
            }
        }
        TotpCmd::Disable { code } => {
            c.call::<_, Value>(method::ACCOUNT_TOTP_DISABLE, &TotpCodeParams { code })
                .await
                .map_err(friendly)?;
            if !json_out {
                println!("Two-factor authentication disabled.");
            }
        }
    }
    Ok(())
}
