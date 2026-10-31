//! Client-side sync against an OurShell server (`server/`, `/v1/...`).
//!
//! See the project plan's "Adding a new device" section and
//! `docs/self-hosting.md`'s "Connecting clients" section for the
//! user-facing flow this implements:
//!
//! - [`signup`]: register a brand-new account on a server, uploading this
//!   vault's *existing* key material (nothing is regenerated), then sign in.
//! - [`login`]: sign in on a brand-new device, building a fresh local vault
//!   from the account/vault material the server hands back -- for every
//!   vault (personal and every team vault) that account belongs to.
//! - [`sync`]: push locally-changed items and pull remote changes, for
//!   every vault this account currently has a key for. Pushing uses
//!   optimistic concurrency; on a conflict the server's copy wins and the
//!   local write is kept anyway, as a new item, so nothing is lost. If
//!   this device was removed from a team vault since its last sync, that
//!   vault is forgotten locally (its key no longer opens anything new).
//! - [`change_password`]: re-registers this account's OPAQUE password file
//!   and pushes the freshly re-wrapped account keys, atomically on the
//!   server, then signs every other device/session out.
//! - Team vaults: [`create_team`], [`team_add_member`], [`team_remove_member`]
//!   (which rotates the vault key) and [`share_item`].
//!
//! The server only ever sees ciphertext, wrapped keys and sealed boxes --
//! see `docs/threat-model.md`. This module's job is to shuttle those bytes
//! and nothing more; `crate::vault` and `crate::crypto` do the actual
//! sealing/opening.

mod client;
mod error;
mod session;

pub use client::SyncClient;
pub use error::{Result, SyncError};
pub use session::SyncSession;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use ourshell_proto::opaque;
use ourshell_proto::sync::{
    AccountKeysUpload, AccountPasswordFinish, AccountPasswordStart, AddVaultMemberRequest,
    CreateInviteRequest, CreateVaultRequest, DeviceInfo, DeviceRegister, LoginFinish, LoginStart,
    OrgRole, OrgUserInfo, PublicKeysResponse, PushRequest, PushResult, RegisterFinish,
    RegisterStart, SyncItem, TotpSetupResponse, VaultMemberSeal, VaultUpload,
};

use crate::crypto::keys::{AccountPublicKeys, ItemEnvelope};
use crate::vault::{Header, Vault, VaultError, VaultInfo};

/// Result of one [`sync`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    pub conflicts: usize,
}

fn new_device_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn encode_pub(bytes: &[u8; 32]) -> String {
    STANDARD.encode(bytes)
}

fn decode_pub(s: &str) -> Result<[u8; 32]> {
    let v = STANDARD
        .decode(s)
        .map_err(|_| SyncError::Protocol("malformed public key from server".into()))?;
    v.try_into()
        .map_err(|_| SyncError::Protocol("public key from server has the wrong length".into()))
}

/// Generates a fresh device identity and its `DeviceRegister` upload.
fn fresh_device(name: &str) -> Result<([u8; 32], DeviceRegister)> {
    let mut secret = [0u8; 32];
    crate::crypto::random_bytes(&mut secret)?;
    let signing_key = SigningKey::from_bytes(&secret);
    let device = DeviceRegister {
        device_id: new_device_id(),
        name: name.to_string(),
        public_key: encode_pub(&signing_key.verifying_key().to_bytes()),
    };
    Ok((secret, device))
}

fn account_upload(header: &Header) -> Result<AccountKeysUpload> {
    Ok(AccountKeysUpload {
        enc_public: encode_pub(&header.account_public.enc_public),
        sign_public: encode_pub(&header.account_public.sign_public),
        kdf: serde_json::to_string(&header.kdf)?,
        wrapped_account_keys: header.wrapped_account_keys.clone(),
        recovery_account_wrap: header.recovery_account_wrap.clone().unwrap_or_default(),
    })
}

fn vault_upload(header: &Header) -> VaultUpload {
    VaultUpload {
        vault_id: header.org_id.clone(),
        key_id: header.vault_key_id.clone(),
        sealed_vault_key: header.sealed_vault_key.clone(),
        recovery_wrap: header.recovery_wrap.clone(),
    }
}

/// `true` if `e` is the server refusing a vault operation because this
/// account is no longer a member (removed, or a rotation it wasn't part
/// of) -- the one error [`sync`] treats as "forget this vault" rather
/// than a hard failure.
fn is_not_a_member(e: &SyncError) -> bool {
    matches!(e, SyncError::Api { code, .. } if code == "not_a_member")
}

/// Registers a new account on `server_url`, uploading `vault`'s *existing*
/// key material (the account keys and vault key already in its header --
/// nothing is regenerated), then signs in as that account's first device.
///
/// `vault` must already be unlocked. On success it is marked no longer
/// `local_only` and the new session is persisted into it.
pub async fn signup(
    server_url: &str,
    email: &str,
    password: &str,
    vault: &mut Vault,
    device_name: &str,
    invite_code: Option<&str>,
) -> Result<SyncSession> {
    // Signing in on another device unlocks the downloaded vault with the
    // account password, so it has to be this vault's master password.
    vault.check_password(password).map_err(|e| match e {
        VaultError::WrongPassword => SyncError::Protocol(
            "the account password must be this vault's master password (other devices unlock the synced vault with it)"
                .into(),
        ),
        other => other.into(),
    })?;
    let client = SyncClient::new(server_url);
    let header = vault.header().clone();
    let account = account_upload(&header)?;
    let vault_body = vault_upload(&header);
    let (device_secret, device) = fresh_device(device_name)?;

    let (reg_state, request_bytes) = opaque::client_register_start(password.as_bytes())?;
    let start = client
        .register_start(&RegisterStart {
            email: email.to_string(),
            registration_request: request_bytes,
            invite_code: invite_code.map(str::to_string),
        })
        .await?;
    let (upload_bytes, _export_key) = opaque::client_register_finish(
        reg_state,
        password.as_bytes(),
        &start.registration_response,
    )?;

    let device_id = device.device_id.clone();
    let session_resp = client
        .register_finish(&RegisterFinish {
            registration_id: start.registration_id,
            registration_upload: upload_bytes,
            account,
            vault: vault_body,
            device,
        })
        .await?;

    vault.set_local_only(false)?;
    let session = SyncSession {
        server_url: client.server_url().to_string(),
        email: email.to_string(),
        device_id,
        device_secret,
        session_token: session_resp.session_token,
        cursors: Default::default(),
        last_sync: None,
    };
    session.save(vault)?;
    Ok(session)
}

/// Signs in on a brand-new device: builds a fresh local vault at `path`
/// from the account/vault material the server returns -- every vault
/// (personal and every team vault) this account belongs to, not just the
/// personal one -- then unlocks it with `password` to prove that material
/// is correct. Fails (without touching disk) if a vault already exists
/// at `path`.
pub async fn login(
    server_url: &str,
    email: &str,
    password: &str,
    path: &std::path::Path,
    device_name: &str,
    totp: Option<&str>,
) -> Result<(Vault, SyncSession)> {
    let client = SyncClient::new(server_url);
    let (device_secret, device) = fresh_device(device_name)?;
    let device_id = device.device_id.clone();

    let (login_state, request_bytes) = opaque::client_login_start(password.as_bytes())?;
    let start = client
        .login_start(&LoginStart {
            email: email.to_string(),
            credential_request: request_bytes,
        })
        .await?;
    let (finalization_bytes, _session_key, _export_key) =
        opaque::client_login_finish(login_state, password.as_bytes(), &start.credential_response)?;

    let session_resp = client
        .login_finish(&LoginFinish {
            login_id: start.login_id,
            credential_finalization: finalization_bytes,
            totp: totp.map(str::to_string),
            device,
        })
        .await?;

    let personal = session_resp
        .vaults
        .iter()
        .find(|v| v.kind == "personal")
        .or_else(|| session_resp.vaults.first())
        .cloned()
        .ok_or_else(|| SyncError::Protocol("server returned no vaults for this account".into()))?;

    let recovery_account_wrap = if session_resp.account.recovery_account_wrap.is_empty() {
        None
    } else {
        Some(session_resp.account.recovery_account_wrap.clone())
    };
    let header = Header {
        format: 1,
        org_id: personal.vault_id.clone(),
        vault_key_id: personal.key_id.clone(),
        local_only: false,
        kdf: serde_json::from_str(&session_resp.account.kdf)?,
        wrapped_account_keys: session_resp.account.wrapped_account_keys,
        account_public: AccountPublicKeys {
            enc_public: decode_pub(&session_resp.account.enc_public)?,
            sign_public: decode_pub(&session_resp.account.sign_public)?,
        },
        sealed_vault_key: personal.sealed_vault_key,
        recovery_wrap: personal.recovery_wrap,
        recovery_account_wrap,
    };

    let mut vault = Vault::create_from_header(path, header)?;

    // Record every *other* vault (team vaults) this account belongs to
    // too, before unlocking -- `unlock()` opens every row in one pass.
    for v in &session_resp.vaults {
        if v.vault_id == personal.vault_id {
            continue;
        }
        vault.upsert_vault(
            &v.vault_id,
            &v.key_id,
            &v.name,
            &v.kind,
            &v.sealed_vault_key,
            "member",
        )?;
    }

    vault.unlock(password)?;

    for v in &session_resp.vaults {
        if v.kind == "team" {
            let _ = refresh_vault_signers(
                &mut vault,
                &client,
                &session_resp.session_token,
                &v.vault_id,
            )
            .await;
        }
    }

    let session = SyncSession {
        server_url: client.server_url().to_string(),
        email: email.to_string(),
        device_id,
        device_secret,
        session_token: session_resp.session_token,
        cursors: Default::default(),
        last_sync: None,
    };
    session.save(&mut vault)?;
    Ok((vault, session))
}

/// Pushes locally-changed items and pulls remote changes, for every vault
/// this account currently has a key for (personal plus every team vault).
/// Loops within each vault until nothing is left dirty (a conflict copy
/// created mid-sync is itself pushed before returning).
pub async fn sync(vault: &mut Vault, session: &mut SyncSession) -> Result<SyncReport> {
    let client = SyncClient::new(session.server_url.clone());

    // Pick up any vault this account was added to (or notice one it was
    // removed from) since the last sync/login, without a fresh login.
    let remote_vaults = client.account_vaults(&session.session_token).await?.vaults;
    let known: std::collections::HashSet<String> = vault
        .vaults()
        .map_err(SyncError::Vault)?
        .into_iter()
        .map(|v| v.vault_id)
        .collect();
    for v in &remote_vaults {
        if !known.contains(&v.vault_id) {
            vault
                .upsert_vault(
                    &v.vault_id,
                    &v.key_id,
                    &v.name,
                    &v.kind,
                    &v.sealed_vault_key,
                    "member",
                )
                .map_err(SyncError::Vault)?;
        }
    }
    let still_member: std::collections::HashSet<String> =
        remote_vaults.iter().map(|v| v.vault_id.clone()).collect();
    for vault_id in known.difference(&still_member) {
        vault.forget_vault(vault_id).map_err(SyncError::Vault)?;
    }

    let vaults = vault.vaults().map_err(SyncError::Vault)?;
    let mut total = SyncReport::default();
    let mut forget = Vec::new();

    for vinfo in &vaults {
        if vinfo.kind == "team" {
            match refresh_vault_signers(vault, &client, &session.session_token, &vinfo.vault_id)
                .await
            {
                Ok(()) => {}
                Err(e) if is_not_a_member(&e) => {
                    forget.push(vinfo.vault_id.clone());
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        match sync_one_vault(vault, &client, session, &vinfo.vault_id).await {
            Ok(r) => {
                total.pushed += r.pushed;
                total.pulled += r.pulled;
                total.conflicts += r.conflicts;
            }
            Err(e) if is_not_a_member(&e) => forget.push(vinfo.vault_id.clone()),
            Err(e) => return Err(e),
        }
    }

    for vault_id in forget {
        vault.forget_vault(&vault_id).map_err(SyncError::Vault)?;
    }

    session.last_sync = Some(now_rfc3339());
    session.save(vault)?;
    Ok(total)
}

async fn sync_one_vault(
    vault: &mut Vault,
    client: &SyncClient,
    session: &mut SyncSession,
    vault_id: &str,
) -> Result<SyncReport> {
    let mut pushed = 0usize;
    let mut conflicts = 0usize;
    // A generous but finite cap: every normal pass either clears every
    // currently-dirty id (ok -> clean, conflict -> overwritten clean) or
    // adds at most one freshly-dirty conflict copy per conflicting id, so
    // this converges in a couple of passes. The cap only guards against a
    // logic bug turning this into an infinite loop.
    for _ in 0..32 {
        let dirty: Vec<_> = vault
            .dirty_envelopes()
            .map_err(SyncError::Vault)?
            .into_iter()
            .filter(|(env, _)| env.org_id == vault_id)
            .collect();
        if dirty.is_empty() {
            break;
        }
        let items = dirty
            .iter()
            .map(|(env, deleted)| {
                Ok(SyncItem {
                    id: env.id.clone(),
                    kind: env.kind.clone(),
                    version: env.version,
                    vault_id: vault_id.to_string(),
                    deleted: *deleted,
                    envelope: serde_json::to_string(env)?,
                    updated_at: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let resp = client
            .push(&session.session_token, vault_id, &PushRequest { items })
            .await?;
        for r in &resp.results {
            match r.status.as_str() {
                "ok" => {
                    if let Some((env, _)) = dirty.iter().find(|(e, _)| e.id == r.id) {
                        vault
                            .mark_clean(&env.id, env.version)
                            .map_err(SyncError::Vault)?;
                    }
                    pushed += 1;
                }
                "conflict" => {
                    conflicts += 1;
                    resolve_conflict(vault, &dirty, r, vault_id)?;
                }
                other => {
                    return Err(SyncError::Protocol(format!(
                        "unexpected push result status `{other}`"
                    )));
                }
            }
        }
    }

    let mut pulled = 0usize;
    loop {
        let cursor = session.cursor_for(vault_id);
        let resp = client
            .pull(&session.session_token, vault_id, cursor)
            .await?;
        let got = resp.items.len();
        for item in &resp.items {
            let env: ItemEnvelope = serde_json::from_str(&item.envelope)?;
            if vault
                .apply_remote(&env, item.deleted)
                .map_err(SyncError::Vault)?
            {
                pulled += 1;
            }
        }
        session.set_cursor(vault_id, resp.cursor);
        if got == 0 {
            break;
        }
    }

    Ok(SyncReport {
        pushed,
        pulled,
        conflicts,
    })
}

/// Server wins a push conflict; the local write is preserved as a new item
/// first (so nothing is lost), renaming it when the decrypted JSON has a
/// `name` field.
fn resolve_conflict(
    vault: &mut Vault,
    dirty: &[(ItemEnvelope, bool)],
    result: &PushResult,
    vault_id: &str,
) -> Result<()> {
    let current = result
        .current
        .as_ref()
        .ok_or_else(|| SyncError::Protocol("conflict result had no `current` copy".into()))?;
    let server_env: ItemEnvelope = serde_json::from_str(&current.envelope)?;

    if let Some((local_env, local_deleted)) = dirty.iter().find(|(e, _)| e.id == result.id) {
        if !*local_deleted {
            let plaintext = vault.open_envelope(local_env).map_err(SyncError::Vault)?;
            let mut value: serde_json::Value = serde_json::from_slice(&plaintext)?;
            if let Some(obj) = value.as_object_mut() {
                if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                    obj.insert(
                        "name".into(),
                        serde_json::Value::String(format!("{name} (conflict copy)")),
                    );
                }
            }
            let new_plaintext = serde_json::to_vec(&value)?;
            vault
                .put_plain(
                    &crate::vault::new_id(),
                    &local_env.kind,
                    vault_id,
                    &new_plaintext,
                )
                .map_err(SyncError::Vault)?;
        }
    }

    vault
        .force_apply_remote(&server_env, current.deleted)
        .map_err(SyncError::Vault)?;
    Ok(())
}

/// Ends this device's session on the server and forgets it locally. The
/// vault's items (and its `local_only` flag) are left untouched: signing
/// back in later resumes sync.
pub async fn logout(vault: &mut Vault, session: &SyncSession) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    client.logout(&session.session_token).await?;
    SyncSession::clear(vault)?;
    Ok(())
}

pub async fn devices(session: &SyncSession) -> Result<Vec<DeviceInfo>> {
    let client = SyncClient::new(session.server_url.clone());
    Ok(client.devices(&session.session_token).await?.devices)
}

pub async fn revoke_device(session: &SyncSession, device_id: &str) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    client
        .revoke_device(&session.session_token, device_id)
        .await
}

pub async fn totp_setup(session: &SyncSession) -> Result<TotpSetupResponse> {
    let client = SyncClient::new(session.server_url.clone());
    client.totp_setup(&session.session_token).await
}

pub async fn totp_confirm(session: &SyncSession, code: &str) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    client.totp_confirm(&session.session_token, code).await
}

pub async fn totp_disable(session: &SyncSession, code: &str) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    client.totp_disable(&session.session_token, code).await
}

// ----- password change / recovery-while-signed-in -----

/// Changes this account's master password, both locally and (if
/// `session` is `Some`) on the sync server: a fresh OPAQUE registration
/// replaces the server's password file and its copy of the KDF
/// params/wrapped account keys, atomically, and every other session for
/// this user is revoked.
///
/// `vault` must already be unlocked. If the server step fails (e.g. no
/// network), the vault has still changed its password locally -- the
/// caller should tell the user to retry once connectivity is back
/// (retrying is safe: it just re-registers under the same new password).
pub async fn change_password(
    vault: &mut Vault,
    session: Option<&SyncSession>,
    new_password: &str,
) -> Result<()> {
    vault
        .change_password(new_password)
        .map_err(SyncError::Vault)?;
    let Some(session) = session else {
        return Ok(());
    };
    let material = AccountKeyMaterial::from(&*vault);
    push_account_keys(&material, session, new_password).await
}

/// This vault's current account key material, extracted as plain owned
/// values. `Vault` holds a `rusqlite::Connection`, which is `Send` but
/// not `Sync` -- a shared `&Vault` therefore can never be `Send` on its
/// own (unlike `&mut Vault`, which only needs `Vault: Send`), so an
/// `async fn` taking `&Vault` and awaiting anything can't be used from
/// `tokio::spawn`. Built with [`Self::from`] before any `.await`.
pub struct AccountKeyMaterial {
    pub kdf: crate::crypto::kdf::KdfParams,
    pub wrapped_account_keys: Vec<u8>,
    pub recovery_account_wrap: Option<Vec<u8>>,
    pub enc_public: String,
    pub sign_public: String,
}

impl From<&Vault> for AccountKeyMaterial {
    fn from(vault: &Vault) -> Self {
        let (kdf, wrapped_account_keys, recovery_account_wrap) = vault.account_key_material();
        Self {
            kdf,
            wrapped_account_keys,
            recovery_account_wrap,
            enc_public: encode_pub(&vault.header().account_public.enc_public),
            sign_public: encode_pub(&vault.header().account_public.sign_public),
        }
    }
}

/// Pushes `material` to the server under a fresh OPAQUE registration for
/// `new_password`. Used by [`change_password`] and by `vault recover`
/// while still signed in (see `docs/self-hosting.md`).
pub async fn push_account_keys(
    material: &AccountKeyMaterial,
    session: &SyncSession,
    new_password: &str,
) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    let (reg_state, request_bytes) = opaque::client_register_start(new_password.as_bytes())?;
    let start = client
        .account_password_start(
            &session.session_token,
            &AccountPasswordStart {
                registration_request: request_bytes,
            },
        )
        .await?;
    let (upload_bytes, _export_key) = opaque::client_register_finish(
        reg_state,
        new_password.as_bytes(),
        &start.registration_response,
    )?;
    client
        .account_password_finish(
            &session.session_token,
            &AccountPasswordFinish {
                registration_id: start.registration_id,
                registration_upload: upload_bytes,
                enc_public: material.enc_public.clone(),
                sign_public: material.sign_public.clone(),
                kdf: serde_json::to_string(&material.kdf)?,
                wrapped_account_keys: material.wrapped_account_keys.clone(),
                recovery_account_wrap: material.recovery_account_wrap.clone().unwrap_or_default(),
            },
        )
        .await?;
    Ok(())
}

// ----- team vaults -----

async fn refresh_vault_signers(
    vault: &mut Vault,
    client: &SyncClient,
    token: &str,
    vault_id: &str,
) -> Result<()> {
    let resp = client.vault_members(token, vault_id).await?;
    let signers: Vec<String> = resp.members.into_iter().map(|m| m.sign_public).collect();
    vault
        .set_vault_signers(vault_id, &signers)
        .map_err(SyncError::Vault)?;
    Ok(())
}

/// Creates a team vault: generates a fresh vault key, seals a copy to
/// this account and to every member in `member_emails` (looked up by
/// email), uploads it, and records the vault locally.
pub async fn create_team(
    vault: &mut Vault,
    session: &SyncSession,
    name: &str,
    member_emails: &[String],
) -> Result<VaultInfo> {
    let client = SyncClient::new(session.server_url.clone());
    let vault_id = crate::vault::new_id();
    let key_id = crate::vault::new_id();
    let team_key = crate::crypto::SymmetricKey::generate()?;

    let own_public = vault.account_keys().map_err(SyncError::Vault)?.public();
    let sealed_self =
        crate::crypto::keys::seal_vault_key(&own_public, &team_key).map_err(SyncError::Crypto)?;
    let mut members = vec![VaultMemberSeal {
        user_id: session_user_id(&client, session).await?,
        sealed_vault_key: sealed_self,
    }];

    for email in member_emails {
        let pk = client.public_keys(&session.session_token, email).await?;
        let sealed = seal_for(&pk, &team_key)?;
        members.push(VaultMemberSeal {
            user_id: pk.user_id,
            sealed_vault_key: sealed,
        });
    }

    client
        .create_vault(
            &session.session_token,
            &CreateVaultRequest {
                vault_id: vault_id.clone(),
                key_id: key_id.clone(),
                name: name.to_string(),
                members,
            },
        )
        .await?;

    let sealed_self_again =
        crate::crypto::keys::seal_vault_key(&own_public, &team_key).map_err(SyncError::Crypto)?;
    vault
        .upsert_vault(
            &vault_id,
            &key_id,
            name,
            "team",
            &sealed_self_again,
            "owner",
        )
        .map_err(SyncError::Vault)?;
    refresh_vault_signers(vault, &client, &session.session_token, &vault_id).await?;

    Ok(VaultInfo {
        vault_id,
        key_id,
        name: name.to_string(),
        kind: "team".to_string(),
        role: "owner".to_string(),
    })
}

fn seal_for(pk: &PublicKeysResponse, key: &crate::crypto::SymmetricKey) -> Result<Vec<u8>> {
    let bytes = decode_pub(&pk.enc_public)?;
    let enc_public = crate::crypto::keys::AccountPublicKeys {
        enc_public: bytes,
        sign_public: [0u8; 32],
    };
    crate::crypto::keys::seal_vault_key(&enc_public, key).map_err(SyncError::Crypto)
}

/// This account's own `user_id`, as the server already knows it (looked
/// up by this device's own email via the public-keys endpoint -- there is
/// no separate "whoami" endpoint today).
async fn session_user_id(client: &SyncClient, session: &SyncSession) -> Result<String> {
    let pk = client
        .public_keys(&session.session_token, &session.email)
        .await?;
    Ok(pk.user_id)
}

/// Adds `email` to an existing team vault: seals that vault's current key
/// to them and uploads the share.
///
/// Takes `&mut Vault` (not `&Vault`, even though nothing here mutates it)
/// because `Vault` is `Send` but not `Sync` (it owns a
/// `rusqlite::Connection`): a shared `&Vault` can never be `Send`
/// regardless of how it's used, which would make this `async fn`'s
/// future non-`Send` and unusable from `tokio::spawn` (see the agent's
/// per-request task); `&mut Vault` has no such problem.
/// Lists the members of a team vault, as the server knows them.
pub async fn team_members(
    session: &SyncSession,
    vault_id: &str,
) -> Result<Vec<ourshell_proto::sync::VaultMemberInfo>> {
    let client = SyncClient::new(session.server_url.clone());
    Ok(client
        .vault_members(&session.session_token, vault_id)
        .await?
        .members)
}

pub async fn team_add_member(
    vault: &mut Vault,
    session: &SyncSession,
    vault_id: &str,
    email: &str,
) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    let key_id = vault
        .vaults()
        .map_err(SyncError::Vault)?
        .into_iter()
        .find(|v| v.vault_id == vault_id)
        .map(|v| v.key_id)
        .ok_or_else(|| SyncError::Protocol(format!("not a member of vault `{vault_id}`")))?;
    // Cloned (not kept as a `&Vault` borrow) -- `Vault` isn't `Sync`, so a
    // reference into it can't be held alive across the `.await` below.
    let key = vault
        .vault_key_for(vault_id)
        .map_err(SyncError::Vault)?
        .clone();
    let pk = client.public_keys(&session.session_token, email).await?;
    let sealed = seal_for(&pk, &key)?;
    client
        .add_vault_member(
            &session.session_token,
            vault_id,
            &AddVaultMemberRequest {
                user_id: pk.user_id,
                key_id,
                sealed_vault_key: sealed,
            },
        )
        .await?;
    Ok(())
}

/// Removes `email` from a team vault and rotates its key: every remaining
/// member is re-sealed a fresh key, and every item in the vault is
/// re-saved as a new version under it (see [`Vault::rotate_vault_key`]
/// and `docs/self-hosting.md`). The removed member -- and any device that
/// doesn't sync before this runs -- can no longer decrypt anything pushed
/// from this point on.
pub async fn team_remove_member(
    vault: &mut Vault,
    session: &mut SyncSession,
    vault_id: &str,
    email: &str,
) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    let target = client.public_keys(&session.session_token, email).await?;
    client
        .remove_vault_member(&session.session_token, vault_id, &target.user_id)
        .await?;

    let new_key_id = crate::vault::new_id();
    let new_key = crate::crypto::SymmetricKey::generate()?;
    let own_public = vault.account_keys().map_err(SyncError::Vault)?.public();
    let sealed_self =
        crate::crypto::keys::seal_vault_key(&own_public, &new_key).map_err(SyncError::Crypto)?;

    // Re-seal the new key to every remaining member (the server already
    // dropped the one we just removed).
    let remaining = client
        .vault_members(&session.session_token, vault_id)
        .await?;
    for m in &remaining.members {
        let sealed = seal_for(
            &PublicKeysResponse {
                user_id: m.user_id.clone(),
                enc_public: m.enc_public.clone(),
                sign_public: m.sign_public.clone(),
            },
            &new_key,
        )?;
        client
            .add_vault_member(
                &session.session_token,
                vault_id,
                &AddVaultMemberRequest {
                    user_id: m.user_id.clone(),
                    key_id: new_key_id.clone(),
                    sealed_vault_key: sealed,
                },
            )
            .await?;
    }

    vault
        .rotate_vault_key(vault_id, &new_key_id, &sealed_self)
        .map_err(SyncError::Vault)?;
    refresh_vault_signers(vault, &client, &session.session_token, vault_id).await?;

    // Push every re-sealed item right away so the rotation takes effect
    // on the server immediately, not just at the next `sync`.
    sync_one_vault(vault, &client, session, vault_id).await?;
    Ok(())
}

/// Moves a personal item into a team vault (re-wrapping its key, not
/// re-encrypting its body) and syncs both the move and the tombstone it
/// leaves behind.
/// Moves item `item_id` into `team_vault_id` (as a *new* id -- see
/// [`Vault::share_into_vault`]) and syncs both the new copy and the
/// tombstone it leaves behind. Returns the item's new id and the sync
/// report.
pub async fn share_item(
    vault: &mut Vault,
    session: &mut SyncSession,
    item_id: &str,
    team_vault_id: &str,
) -> Result<(String, SyncReport)> {
    let new_id = vault
        .share_into_vault(item_id, team_vault_id)
        .map_err(SyncError::Vault)?;
    let report = sync(vault, session).await?;
    Ok((new_id, report))
}

// ----- org administration -----

pub async fn create_invite(session: &SyncSession, role: Option<OrgRole>) -> Result<String> {
    let client = SyncClient::new(session.server_url.clone());
    let resp = client
        .create_invite(&session.session_token, &CreateInviteRequest { role })
        .await?;
    Ok(resp.code)
}

pub async fn org_users(session: &SyncSession) -> Result<Vec<OrgUserInfo>> {
    let client = SyncClient::new(session.server_url.clone());
    Ok(client.org_users(&session.session_token).await?.users)
}

pub async fn disable_user(session: &SyncSession, user_id: &str) -> Result<()> {
    let client = SyncClient::new(session.server_url.clone());
    client.disable_user(&session.session_token, user_id).await
}
