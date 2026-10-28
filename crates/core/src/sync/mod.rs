//! Client-side sync against an OurShell server (`server/`, `/v1/...`).
//!
//! See the project plan's "Adding a new device" section and
//! `docs/self-hosting.md`'s "Connecting clients" section for the
//! user-facing flow this implements:
//!
//! - [`signup`]: register a brand-new account on a server, uploading this
//!   vault's *existing* key material (nothing is regenerated), then sign in.
//! - [`login`]: sign in on a brand-new device, building a fresh local vault
//!   from the account/vault material the server hands back.
//! - [`sync`]: push locally-changed items and pull remote changes. Pushing
//!   uses optimistic concurrency; on a conflict the server's copy wins and
//!   the local write is kept anyway, as a new item, so nothing is lost.
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
    AccountKeysUpload, DeviceInfo, DeviceRegister, LoginFinish, LoginStart, PushRequest,
    PushResult, RegisterFinish, RegisterStart, SyncItem, TotpSetupResponse, VaultUpload,
};

use crate::crypto::keys::{AccountPublicKeys, ItemEnvelope};
use crate::vault::{Header, Vault};

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
        cursor: 0,
        last_sync: None,
    };
    session.save(vault)?;
    Ok(session)
}

/// Signs in on a brand-new device: builds a fresh local vault at `path`
/// from the account/vault material the server returns, then unlocks it
/// with `password` to prove that material is correct. Fails (without
/// touching disk) if a vault already exists at `path`.
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

    let header = Header {
        format: 1,
        org_id: personal.vault_id,
        vault_key_id: personal.key_id,
        local_only: false,
        kdf: serde_json::from_str(&session_resp.account.kdf)?,
        wrapped_account_keys: session_resp.account.wrapped_account_keys,
        account_public: AccountPublicKeys {
            enc_public: decode_pub(&session_resp.account.enc_public)?,
            sign_public: decode_pub(&session_resp.account.sign_public)?,
        },
        sealed_vault_key: personal.sealed_vault_key,
        recovery_wrap: personal.recovery_wrap,
    };

    let mut vault = Vault::create_from_header(path, header)?;
    vault.unlock(password)?;

    let session = SyncSession {
        server_url: client.server_url().to_string(),
        email: email.to_string(),
        device_id,
        device_secret,
        session_token: session_resp.session_token,
        cursor: 0,
        last_sync: None,
    };
    session.save(&mut vault)?;
    Ok((vault, session))
}

/// Pushes locally-changed items and pulls remote changes, looping until
/// nothing is left dirty (a conflict copy created mid-sync is itself
/// pushed before `sync` returns).
pub async fn sync(vault: &mut Vault, session: &mut SyncSession) -> Result<SyncReport> {
    let client = SyncClient::new(session.server_url.clone());
    let vault_id = vault.header().org_id.clone();

    let mut pushed = 0usize;
    let mut conflicts = 0usize;
    // A generous but finite cap: every normal pass either clears every
    // currently-dirty id (ok -> clean, conflict -> overwritten clean) or
    // adds at most one freshly-dirty conflict copy per conflicting id, so
    // this converges in a couple of passes. The cap only guards against a
    // logic bug turning this into an infinite loop.
    for _ in 0..32 {
        let dirty = vault.dirty_envelopes()?;
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
                    vault_id: vault_id.clone(),
                    deleted: *deleted,
                    envelope: serde_json::to_string(env)?,
                    updated_at: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let resp = client
            .push(&session.session_token, &vault_id, &PushRequest { items })
            .await?;
        for r in &resp.results {
            match r.status.as_str() {
                "ok" => {
                    if let Some((env, _)) = dirty.iter().find(|(e, _)| e.id == r.id) {
                        vault.mark_clean(&env.id, env.version)?;
                    }
                    pushed += 1;
                }
                "conflict" => {
                    conflicts += 1;
                    resolve_conflict(vault, &dirty, r)?;
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
        let resp = client
            .pull(&session.session_token, &vault_id, session.cursor)
            .await?;
        let got = resp.items.len();
        for item in &resp.items {
            let env: ItemEnvelope = serde_json::from_str(&item.envelope)?;
            if vault.apply_remote(&env, item.deleted)? {
                pulled += 1;
            }
        }
        session.cursor = resp.cursor;
        if got == 0 {
            break;
        }
    }

    session.last_sync = Some(now_rfc3339());
    session.save(vault)?;
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
) -> Result<()> {
    let current = result
        .current
        .as_ref()
        .ok_or_else(|| SyncError::Protocol("conflict result had no `current` copy".into()))?;
    let server_env: ItemEnvelope = serde_json::from_str(&current.envelope)?;

    if let Some((local_env, local_deleted)) = dirty.iter().find(|(e, _)| e.id == result.id) {
        if !*local_deleted {
            let plaintext = vault.open_envelope(local_env)?;
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
            vault.put_plain(&crate::vault::new_id(), &local_env.kind, &new_plaintext)?;
        }
    }

    vault.force_apply_remote(&server_env, current.deleted)?;
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
