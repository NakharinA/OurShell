//! `account.*`, `sync.*` and `vault.recover` handlers.
//!
//! Signup/login/sync talk to the server while holding the agent's vault
//! lock for the whole call (these are rare, user-initiated actions, unlike
//! the hot-path `item.*` handlers), so no other request can observe a
//! half-updated vault mid-sync.

use std::sync::Arc;
use std::time::Instant;

use ourshell_core::sync::{self, SyncError, SyncSession};
use ourshell_core::vault::{Vault, VaultError};
use ourshell_proto::agent::{
    event, method, AccountChangePasswordParams, AccountLoginParams, AccountSignupParams,
    AccountStatus, DeviceRef, OrgDisableUserParams, OrgInviteParams, OrgInviteResult, SyncParams,
    SyncReport, TeamCreateParams, TeamInfo, TeamMemberParams, TeamRef, TeamShareParams,
    TeamShareResult, TotpCodeParams, VaultRecoverParams,
};
use ourshell_proto::rpc::{self, RpcError};
use ourshell_proto::sync::{DeviceInfo, OrgRole, OrgUserInfo, TotpSetupResponse};
use serde_json::{json, Value};

use super::{err, parse, to_value, vault_err, RpcResult, State};

pub async fn dispatch(state: &Arc<State>, m: &str, params: Value) -> RpcResult {
    match m {
        method::ACCOUNT_SIGNUP => account_signup(state, parse(params)?).await,
        method::ACCOUNT_LOGIN => account_login(state, parse(params)?).await,
        method::ACCOUNT_LOGOUT => account_logout(state).await,
        method::ACCOUNT_STATUS => account_status(state).await,
        method::ACCOUNT_DEVICES => account_devices(state).await,
        method::ACCOUNT_REVOKE_DEVICE => {
            let p: DeviceRef = parse(params)?;
            account_revoke_device(state, p).await
        }
        method::ACCOUNT_TOTP_SETUP => account_totp_setup(state).await,
        method::ACCOUNT_TOTP_CONFIRM => {
            let p: TotpCodeParams = parse(params)?;
            account_totp_confirm(state, p).await
        }
        method::ACCOUNT_TOTP_DISABLE => {
            let p: TotpCodeParams = parse(params)?;
            account_totp_disable(state, p).await
        }
        method::ACCOUNT_CHANGE_PASSWORD => {
            let p: AccountChangePasswordParams = parse(params)?;
            account_change_password(state, p).await
        }
        method::SYNC_RUN => {
            let p: SyncParams = parse(params)?;
            sync_run(state, p).await
        }
        method::VAULT_RECOVER => {
            let p: VaultRecoverParams = parse(params)?;
            vault_recover(state, p).await
        }
        method::TEAM_CREATE => {
            let p: TeamCreateParams = parse(params)?;
            team_create(state, p).await
        }
        method::TEAM_LIST => team_list(state).await,
        method::TEAM_ADD_MEMBER => {
            let p: TeamMemberParams = parse(params)?;
            team_add_member(state, p).await
        }
        method::TEAM_MEMBERS => {
            let p: TeamRef = parse(params)?;
            team_members(state, p).await
        }
        method::TEAM_REMOVE_MEMBER => {
            let p: TeamMemberParams = parse(params)?;
            team_remove_member(state, p).await
        }
        method::TEAM_SHARE => {
            let p: TeamShareParams = parse(params)?;
            team_share(state, p).await
        }
        method::ORG_INVITE => {
            let p: OrgInviteParams = parse(params)?;
            org_invite(state, p).await
        }
        method::ORG_USERS => org_users(state).await,
        method::ORG_DISABLE_USER => {
            let p: OrgDisableUserParams = parse(params)?;
            org_disable_user(state, p).await
        }
        _ => Err(err(rpc::METHOD_NOT_FOUND, format!("unknown method `{m}`"))),
    }
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "ourshell-device".to_string())
}

fn sync_err(e: SyncError) -> RpcError {
    match e {
        SyncError::Api {
            status,
            code,
            message,
        } => RpcError {
            code: rpc::INVALID_PARAMS,
            message: format!("sync server rejected the request ({status} {code}): {message}"),
            // The server's error code (e.g. `totp_required`), so clients can
            // react to it without parsing the message.
            data: Some(json!({ "server_status": status, "server_error": code })),
        },
        SyncError::Http(e) => err(
            rpc::INTERNAL_ERROR,
            format!("could not reach the sync server: {e}"),
        ),
        SyncError::Vault(e) => vault_err(e),
        SyncError::InvalidCredentials => RpcError {
            code: rpc::INVALID_PARAMS,
            message: SyncError::InvalidCredentials.to_string(),
            data: Some(json!({ "server_error": "invalid_credentials" })),
        },
        other => err(rpc::INTERNAL_ERROR, other.to_string()),
    }
}

fn report_of(r: sync::SyncReport) -> SyncReport {
    SyncReport {
        pushed: r.pushed,
        pulled: r.pulled,
        conflicts: r.conflicts,
    }
}

fn status_from(session: Option<&SyncSession>) -> AccountStatus {
    match session {
        Some(s) => AccountStatus {
            signed_in: true,
            server: Some(s.server_url.clone()),
            email: Some(s.email.clone()),
            device_id: Some(s.device_id.clone()),
            last_sync: s.last_sync.clone(),
        },
        None => AccountStatus {
            signed_in: false,
            server: None,
            email: None,
            device_id: None,
            last_sync: None,
        },
    }
}

async fn account_signup(state: &Arc<State>, p: AccountSignupParams) -> RpcResult {
    let device_name = p.device_name.unwrap_or_else(default_device_name);
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let session = sync::signup(
        &p.server,
        &p.email,
        &p.password,
        vault,
        &device_name,
        p.invite_code.as_deref(),
    )
    .await
    .map_err(sync_err)?;
    to_value(status_from(Some(&session)))
}

/// Logs in on a device that has no local vault yet (or an empty one),
/// replacing the vault file with one built from the server's material.
/// Refuses if the local vault already has items.
async fn account_login(state: &Arc<State>, p: AccountLoginParams) -> RpcResult {
    let device_name = p.device_name.unwrap_or_else(default_device_name);
    let path = state.opts.vault_path.clone();

    let mut slot = state.vault.lock().await;
    if path.exists() {
        let existing = match slot.vault.take() {
            Some(v) => Some(v),
            None => match Vault::open(&path) {
                Ok(v) => Some(v),
                Err(VaultError::Missing(_)) => None,
                Err(e) => return Err(vault_err(e)),
            },
        };
        if let Some(v) = existing {
            let has_items = !v.metas().map_err(vault_err)?.is_empty();
            if has_items {
                slot.vault = Some(v);
                return Err(err(
                    rpc::INVALID_PARAMS,
                    "the local vault already has items; refusing to replace it with a new \
                     login (back it up or move it aside first)",
                ));
            }
            // Empty: safe to replace. Drop the connection before removing
            // the file (an open handle blocks deletion on Windows).
            drop(v);
            std::fs::remove_file(&path).map_err(|e| {
                err(
                    rpc::INTERNAL_ERROR,
                    format!("could not replace the vault file: {e}"),
                )
            })?;
        }
    }

    let (vault, session) = sync::login(
        &p.server,
        &p.email,
        &p.password,
        &path,
        &device_name,
        p.totp.as_deref(),
    )
    .await
    .map_err(sync_err)?;
    slot.vault = Some(vault);
    slot.last_activity = Instant::now();
    drop(slot);
    state.emit(event::VAULT_UNLOCKED, json!({}));
    to_value(status_from(Some(&session)))
}

async fn account_logout(state: &Arc<State>) -> RpcResult {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let Some(session) = SyncSession::load(vault).map_err(sync_err)? else {
        return Err(err(rpc::INVALID_PARAMS, "not signed in to a sync server"));
    };
    sync::logout(vault, &session).await.map_err(sync_err)?;
    Ok(json!({}))
}

async fn account_status(state: &Arc<State>) -> RpcResult {
    let mut slot = state.vault.lock().await;
    let status = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => status_from(SyncSession::load(v).map_err(sync_err)?.as_ref()),
        _ => status_from(None),
    };
    to_value(status)
}

/// Loads the current session, releasing the vault lock before returning
/// (callers that only need to talk to the server, not the vault, do so
/// without holding it across the network call).
async fn current_session(state: &Arc<State>) -> std::result::Result<SyncSession, RpcError> {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    match SyncSession::load(vault).map_err(sync_err)? {
        Some(s) => Ok(s),
        None => Err(err(rpc::INVALID_PARAMS, "not signed in to a sync server")),
    }
}

async fn account_devices(state: &Arc<State>) -> RpcResult {
    let session = current_session(state).await?;
    let devices: Vec<DeviceInfo> = sync::devices(&session).await.map_err(sync_err)?;
    to_value(devices)
}

async fn account_revoke_device(state: &Arc<State>, p: DeviceRef) -> RpcResult {
    let session = current_session(state).await?;
    sync::revoke_device(&session, &p.device_id)
        .await
        .map_err(sync_err)?;
    Ok(json!({}))
}

async fn account_totp_setup(state: &Arc<State>) -> RpcResult {
    let session = current_session(state).await?;
    let resp: TotpSetupResponse = sync::totp_setup(&session).await.map_err(sync_err)?;
    to_value(resp)
}

async fn account_totp_confirm(state: &Arc<State>, p: TotpCodeParams) -> RpcResult {
    let session = current_session(state).await?;
    sync::totp_confirm(&session, &p.code)
        .await
        .map_err(sync_err)?;
    Ok(json!({}))
}

async fn account_totp_disable(state: &Arc<State>, p: TotpCodeParams) -> RpcResult {
    let session = current_session(state).await?;
    sync::totp_disable(&session, &p.code)
        .await
        .map_err(sync_err)?;
    Ok(json!({}))
}

async fn sync_run(state: &Arc<State>, _p: SyncParams) -> RpcResult {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let mut session = SyncSession::load(vault)
        .map_err(sync_err)?
        .ok_or_else(|| err(rpc::INVALID_PARAMS, "not signed in to a sync server"))?;
    let report = sync::sync(vault, &mut session).await.map_err(sync_err)?;
    to_value(report_of(report))
}

/// Runs one sync if the vault is unlocked and signed in; called by the
/// agent's background timer (`OURSHELL_AUTOSYNC_SECS`). Failures are
/// logged, never propagated -- a flaky network must not crash the agent or
/// touch the vault's lock state.
pub async fn autosync_tick(state: &Arc<State>) {
    let mut slot = state.vault.lock().await;
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return,
    };
    let Ok(Some(mut session)) = SyncSession::load(vault) else {
        return;
    };
    match sync::sync(vault, &mut session).await {
        Ok(report) => {
            if report.pushed > 0 || report.pulled > 0 || report.conflicts > 0 {
                tracing::info!(?report, "autosync");
            }
        }
        Err(e) => tracing::warn!("autosync failed: {e}"),
    }
}

/// Recovers from the 24-word phrase and sets a new master password. If
/// this vault is signed in to a sync server, also pushes the recovered
/// account key material there (best-effort: local recovery has already
/// succeeded either way -- see `docs/self-hosting.md`).
async fn vault_recover(state: &Arc<State>, p: VaultRecoverParams) -> RpcResult {
    let path = state.opts.vault_path.clone();
    let mut slot = state.vault.lock().await;
    let mut vault = match slot.vault.take() {
        Some(v) => v,
        None => Vault::open(&path).map_err(|e| match e {
            VaultError::Missing(_) => {
                err(rpc::NOT_FOUND, "no vault yet; run `ourshell vault init`")
            }
            other => vault_err(other),
        })?,
    };
    let res = tokio::task::block_in_place(|| vault.recover(&p.phrase, &p.new_password));
    slot.vault = Some(vault);
    res.map_err(vault_err)?;
    slot.last_activity = Instant::now();

    let mut result = json!({});
    let vault = slot.vault.as_mut().expect("just set");
    let session_and_material = SyncSession::load(vault)
        .ok()
        .flatten()
        .map(|session| (session, sync::AccountKeyMaterial::from(&*vault)));
    if let Some((session, material)) = session_and_material {
        match sync::push_account_keys(&material, &session, &p.new_password).await {
            Ok(()) => result = json!({"server_updated": true}),
            Err(e) => {
                result = json!({
                    "server_updated": false,
                    "server_error": format!(
                        "local recovery succeeded, but updating the sync server failed ({e}). \
                         Your session may have expired -- log in with your OLD password first \
                         (if you still remember it) and run `vault recover` again, or ask an \
                         org admin to re-invite/disable this account."
                    ),
                });
            }
        }
    }
    drop(slot);
    state.emit(event::VAULT_UNLOCKED, json!({}));
    Ok(result)
}

/// Changes the master password, both locally and (if signed in) on the
/// sync server. If the vault is locked, `old_password` unlocks it first.
async fn account_change_password(state: &Arc<State>, p: AccountChangePasswordParams) -> RpcResult {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) => v,
        None => {
            return Err(err(
                rpc::NOT_FOUND,
                "no vault yet; run `ourshell vault init`",
            ))
        }
    };
    // Always prove the current master password, even when the vault is
    // already unlocked: otherwise anyone at an unlocked computer could
    // change it (and the sync account's password with it).
    let old = p
        .old_password
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err(
                rpc::INVALID_PARAMS,
                "the current master password is required to change it",
            )
        })?;
    if vault.is_unlocked() {
        tokio::task::block_in_place(|| vault.check_password(old)).map_err(vault_err)?;
    } else {
        tokio::task::block_in_place(|| vault.unlock(old)).map_err(vault_err)?;
    }
    let session = SyncSession::load(vault).map_err(sync_err)?;
    let server_updated = session.is_some();
    sync::change_password(vault, session.as_ref(), &p.new_password)
        .await
        .map_err(sync_err)?;
    slot.last_activity = Instant::now();
    Ok(json!({"server_updated": server_updated}))
}

// ----- team vaults / org administration -----

fn team_err(e: ourshell_core::vault::VaultError) -> RpcError {
    vault_err(e)
}

fn team_info_of(v: &ourshell_core::vault::VaultInfo) -> TeamInfo {
    TeamInfo {
        vault_id: v.vault_id.clone(),
        key_id: v.key_id.clone(),
        name: v.name.clone(),
        kind: v.kind.clone(),
        role: v.role.clone(),
    }
}

/// Resolves a team vault by id or (case-insensitive) name.
fn resolve_team(vault: &Vault, team: &str) -> std::result::Result<String, RpcError> {
    let vaults = vault.vaults().map_err(team_err)?;
    if let Some(v) = vaults.iter().find(|v| v.vault_id == team) {
        return Ok(v.vault_id.clone());
    }
    vaults
        .iter()
        .find(|v| v.kind == "team" && v.name.eq_ignore_ascii_case(team))
        .map(|v| v.vault_id.clone())
        .ok_or_else(|| err(rpc::NOT_FOUND, format!("no team vault named `{team}`")))
}

async fn team_create(state: &Arc<State>, p: TeamCreateParams) -> RpcResult {
    let session = current_session(state).await?;
    let mut slot = state.vault.lock().await;
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let info = sync::create_team(vault, &session, &p.name, &p.members)
        .await
        .map_err(sync_err)?;
    to_value(team_info_of(&info))
}

async fn team_list(state: &Arc<State>) -> RpcResult {
    let teams = state
        .with_vault(|v| v.vaults())
        .await?
        .into_iter()
        .map(|v| team_info_of(&v))
        .collect::<Vec<_>>();
    to_value(teams)
}

async fn team_members(state: &Arc<State>, p: TeamRef) -> RpcResult {
    let session = current_session(state).await?;
    let vault_id = {
        let slot = state.vault.lock().await;
        match slot.vault.as_ref() {
            Some(v) if v.is_unlocked() => resolve_team(v, &p.team)?,
            _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
        }
    };
    let members = sync::team_members(&session, &vault_id)
        .await
        .map_err(sync_err)?;
    to_value(members)
}

async fn team_add_member(state: &Arc<State>, p: TeamMemberParams) -> RpcResult {
    let session = current_session(state).await?;
    let mut slot = state.vault.lock().await;
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let vault_id = resolve_team(vault, &p.team)?;
    sync::team_add_member(vault, &session, &vault_id, &p.email)
        .await
        .map_err(sync_err)?;
    Ok(json!({}))
}

async fn team_remove_member(state: &Arc<State>, p: TeamMemberParams) -> RpcResult {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let mut session = SyncSession::load(vault)
        .map_err(sync_err)?
        .ok_or_else(|| err(rpc::INVALID_PARAMS, "not signed in to a sync server"))?;
    let vault_id = resolve_team(vault, &p.team)?;
    sync::team_remove_member(vault, &mut session, &vault_id, &p.email)
        .await
        .map_err(sync_err)?;
    session.save(vault).map_err(sync_err)?;
    Ok(json!({}))
}

async fn team_share(state: &Arc<State>, p: TeamShareParams) -> RpcResult {
    let mut slot = state.vault.lock().await;
    slot.last_activity = Instant::now();
    let vault = match slot.vault.as_mut() {
        Some(v) if v.is_unlocked() => v,
        _ => return Err(err(rpc::VAULT_LOCKED, "vault is locked")),
    };
    let mut session = SyncSession::load(vault)
        .map_err(sync_err)?
        .ok_or_else(|| err(rpc::INVALID_PARAMS, "not signed in to a sync server"))?;
    let vault_id = resolve_team(vault, &p.team)?;
    let (new_id, _report) = sync::share_item(vault, &mut session, &p.item_id, &vault_id)
        .await
        .map_err(sync_err)?;
    session.save(vault).map_err(sync_err)?;
    to_value(TeamShareResult { new_id })
}

async fn org_invite(state: &Arc<State>, p: OrgInviteParams) -> RpcResult {
    let session = current_session(state).await?;
    let role = match p.role.as_deref() {
        Some("admin") => Some(OrgRole::Admin),
        Some("member") | None => None,
        Some(other) => {
            return Err(err(
                rpc::INVALID_PARAMS,
                format!("unknown role `{other}` (use admin or member)"),
            ))
        }
    };
    let code = sync::create_invite(&session, role)
        .await
        .map_err(sync_err)?;
    to_value(OrgInviteResult { code })
}

async fn org_users(state: &Arc<State>) -> RpcResult {
    let session = current_session(state).await?;
    let users: Vec<OrgUserInfo> = sync::org_users(&session).await.map_err(sync_err)?;
    to_value(users)
}

async fn org_disable_user(state: &Arc<State>, p: OrgDisableUserParams) -> RpcResult {
    let session = current_session(state).await?;
    sync::disable_user(&session, &p.user_id)
        .await
        .map_err(sync_err)?;
    Ok(json!({}))
}
