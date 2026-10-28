//! `ourshell ai ...`: approvals, dangerous mode, kill switch and audit log.

use std::io::Write;

use anyhow::Result;
use clap::Subcommand;
use ourshell::client::{hello, AgentClient};
use ourshell_core::vault::AuditEntry;
use ourshell_proto::agent::{
    event, method, AiStatus, ApprovalRequest, AuditQuery, ClientKind, DangerousParams, DecideParams,
};
use serde_json::{json, Value};

use super::{connect_unlocked, friendly};

#[derive(Subcommand)]
pub enum AiCmd {
    /// Watch for AI requests and approve or reject them interactively.
    Watch,
    /// List pending approvals.
    Pending,
    /// Approve a pending request.
    Approve { approval_id: String },
    /// Reject a pending request.
    Reject { approval_id: String },
    /// Show connected AI sessions and their mode.
    Status,
    /// Let one AI session run without approval on chosen hosts, for a limited time.
    Dangerous {
        /// AI session id (see `ourshell ai status`).
        #[arg(long)]
        session: String,
        /// Comma-separated AI-allowed host names.
        #[arg(long)]
        hosts: String,
        /// Minutes until approvals are required again (default 60).
        #[arg(long, default_value_t = 60)]
        minutes: u64,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Turn dangerous mode off for a session.
    Safe {
        #[arg(long)]
        session: String,
    },
    /// Kill switch: end dangerous mode, reject pending requests, block all AI sessions.
    Stop,
    /// Show the AI audit log.
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

pub async fn run(cmd: AiCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        AiCmd::Watch => watch().await?,
        AiCmd::Pending => {
            let p: Vec<ApprovalRequest> = c
                .call(method::AI_APPROVALS, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&p)?);
            } else if p.is_empty() {
                println!("No pending AI requests.");
            } else {
                for r in p {
                    print_request(&r);
                }
            }
        }
        AiCmd::Approve { approval_id } => decide(&c, &approval_id, true).await?,
        AiCmd::Reject { approval_id } => decide(&c, &approval_id, false).await?,
        AiCmd::Status => {
            let st: AiStatus = c
                .call(method::AI_STATUS, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&st)?);
                return Ok(());
            }
            if st.sessions.is_empty() {
                println!("No AI sessions connected.");
            }
            for s in &st.sessions {
                let mode = match &s.dangerous_until {
                    Some(u) => format!(
                        "\x1b[41;97m DANGEROUS \x1b[0m until {u} on {}",
                        s.dangerous_hosts.join(", ")
                    ),
                    None => "approval".into(),
                };
                println!(
                    "{}  {}  {}",
                    s.ai_session,
                    s.ai_client_name.as_deref().unwrap_or("AI client"),
                    mode
                );
            }
            if !st.pending.is_empty() {
                println!(
                    "\n{} pending request(s); run `ourshell ai watch`.",
                    st.pending.len()
                );
            }
        }
        AiCmd::Dangerous {
            session,
            hosts,
            minutes,
            yes,
        } => {
            let hosts: Vec<String> = hosts
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !yes {
                println!("\x1b[41;97m WARNING: DANGEROUS MODE \x1b[0m");
                println!(
                    "AI session {session} will run commands and write files WITHOUT asking you on:"
                );
                for h in &hosts {
                    println!("  - {h}");
                }
                println!("for the next {minutes} minutes or until the AI session ends.");
                println!("Output from servers can contain text written to trick the AI (prompt injection).");
                print!("Type `yes` to continue: ");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim() != "yes" {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            c.call::<_, Value>(
                method::AI_DANGEROUS,
                &DangerousParams {
                    ai_session: session,
                    hosts,
                    minutes: Some(minutes),
                },
            )
            .await
            .map_err(friendly)?;
            println!("Dangerous mode on for {minutes} minutes. Stop it any time with `ourshell ai stop`.");
        }
        AiCmd::Safe { session } => {
            c.call::<_, Value>(
                method::AI_DANGEROUS,
                &DangerousParams {
                    ai_session: session,
                    hosts: vec![],
                    minutes: Some(0),
                },
            )
            .await
            .map_err(friendly)?;
            println!("Approval mode restored.");
        }
        AiCmd::Stop => {
            c.call::<_, Value>(method::AI_STOP, &json!({}))
                .await
                .map_err(friendly)?;
            println!("All AI sessions stopped. AI clients must reconnect to be used again.");
        }
        AiCmd::Audit { limit } => {
            let entries: Vec<AuditEntry> = c
                .call(method::AI_AUDIT, &AuditQuery { limit: Some(limit) })
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&entries)?);
                return Ok(());
            }
            println!(
                "{:<25} {:<12} {:<11} {:<10} {:<9} {:<5} DETAIL",
                "TIME", "HOST", "TOOL", "MODE", "OUTCOME", "EXIT"
            );
            for e in entries {
                println!(
                    "{:<25} {:<12} {:<11} {:<10} {:<9} {:<5} {}",
                    &e.time[..e.time.len().min(25)],
                    e.host,
                    e.tool,
                    e.mode,
                    e.outcome,
                    e.exit_code.map(|c| c.to_string()).unwrap_or_default(),
                    e.detail.replace('\n', " ")
                );
            }
        }
    }
    Ok(())
}

async fn decide(c: &AgentClient, id: &str, approve: bool) -> Result<()> {
    c.call::<_, Value>(
        method::AI_DECIDE,
        &DecideParams {
            approval_id: id.into(),
            approve,
        },
    )
    .await
    .map_err(friendly)?;
    println!("{}", if approve { "Approved." } else { "Rejected." });
    Ok(())
}

fn print_request(r: &ApprovalRequest) {
    println!(
        "[{}] {} wants to {} on `{}`:\n    {}",
        r.approval_id,
        r.ai_client_name.as_deref().unwrap_or("AI"),
        r.tool,
        r.host,
        r.detail.replace('\n', "\n    ")
    );
}

/// Streams approval requests and asks y/n for each.
async fn watch() -> Result<()> {
    let (c, mut events) = AgentClient::connect_or_spawn(hello(ClientKind::Cli)).await?;
    c.call::<_, Value>(method::EVENTS_SUBSCRIBE, &json!({}))
        .await?;
    println!("Watching for AI requests (Ctrl+C to stop)...");
    let pending: Vec<ApprovalRequest> = c
        .call(method::AI_APPROVALS, &json!({}))
        .await
        .map_err(friendly)?;
    for r in pending {
        ask(&c, &r).await?;
    }
    while let Some(n) = events.recv().await {
        match n.method.as_str() {
            event::APPROVAL_REQUEST => {
                if let Ok(r) = serde_json::from_value::<ApprovalRequest>(n.params) {
                    ask(&c, &r).await?;
                }
            }
            event::AI_MODE => println!("[ai] {}", n.params),
            event::AI_CALL => {
                let e = &n.params;
                println!(
                    "[ai] {} {} on {}: {} ({})",
                    e["tool"].as_str().unwrap_or(""),
                    e["detail"].as_str().unwrap_or(""),
                    e["host"].as_str().unwrap_or(""),
                    e["outcome"].as_str().unwrap_or(""),
                    e["mode"].as_str().unwrap_or("")
                );
            }
            event::VAULT_LOCKED => println!("[vault] locked; AI calls will fail until you unlock."),
            _ => {}
        }
    }
    Ok(())
}

async fn ask(c: &AgentClient, r: &ApprovalRequest) -> Result<()> {
    print_request(r);
    let r2 = r.clone();
    let answer = tokio::task::spawn_blocking(move || {
        print!("Approve {}? [y/N] ", r2.tool);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes")
    })
    .await?;
    match c
        .call::<_, Value>(
            method::AI_DECIDE,
            &DecideParams {
                approval_id: r.approval_id.clone(),
                approve: answer,
            },
        )
        .await
    {
        Ok(_) => println!(
            "{}",
            if answer {
                "  -> approved"
            } else {
                "  -> rejected"
            }
        ),
        Err(e) => println!("  -> could not answer: {e} (maybe it timed out)"),
    }
    Ok(())
}
