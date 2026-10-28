//! Sync server HTTP API (`/v1/...`) request and response bodies.
//!
//! The server only ever sees these types. Every `*_ciphertext` / `sealed_*` /
//! `wrapped_*` field is opaque to it.

use serde::{Deserialize, Serialize};

pub const API_PREFIX: &str = "/v1";

/// One encrypted item as stored on the server. Mirrors
/// `ourshell_core::crypto::keys::ItemEnvelope` plus sync metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub vault_id: String,
    pub deleted: bool,
    /// JSON-serialized `ItemEnvelope`.
    pub envelope: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ----- OPAQUE registration / login -----
// Each OPAQUE message is the opaque-ke serialized bytes, base64-encoded.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterStart {
    pub email: String,
    #[serde(with = "crate::b64")]
    pub registration_request: Vec<u8>,
    #[serde(default)]
    pub invite_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterStartResponse {
    pub registration_id: String,
    #[serde(with = "crate::b64")]
    pub registration_response: Vec<u8>,
}

/// Account material uploaded at signup. All secrets are wrapped client-side.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountKeysUpload {
    pub enc_public: String,
    pub sign_public: String,
    /// KDF params JSON (salt etc.) needed to re-derive the master key on a new device.
    pub kdf: String,
    #[serde(with = "crate::b64")]
    pub wrapped_account_keys: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultUpload {
    pub vault_id: String,
    pub key_id: String,
    #[serde(with = "crate::b64")]
    pub sealed_vault_key: Vec<u8>,
    #[serde(with = "crate::b64")]
    pub recovery_wrap: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterFinish {
    pub registration_id: String,
    #[serde(with = "crate::b64")]
    pub registration_upload: Vec<u8>,
    pub account: AccountKeysUpload,
    pub vault: VaultUpload,
    pub device: DeviceRegister,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStart {
    pub email: String,
    #[serde(with = "crate::b64")]
    pub credential_request: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStartResponse {
    pub login_id: String,
    #[serde(with = "crate::b64")]
    pub credential_response: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginFinish {
    pub login_id: String,
    #[serde(with = "crate::b64")]
    pub credential_finalization: Vec<u8>,
    #[serde(default)]
    pub totp: Option<String>,
    pub device: DeviceRegister,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceRegister {
    pub device_id: String,
    pub name: String,
    /// Ed25519 device public key, base64.
    pub public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub session_token: String,
    pub user_id: String,
    pub org_id: String,
    pub account: AccountKeysUpload,
    pub vaults: Vec<VaultKeyInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultKeyInfo {
    pub vault_id: String,
    pub key_id: String,
    pub name: String,
    /// "personal" or "team".
    pub kind: String,
    #[serde(with = "crate::b64")]
    pub sealed_vault_key: Vec<u8>,
    #[serde(default, with = "crate::b64")]
    pub recovery_wrap: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub public_key: String,
    pub created_at: String,
    pub revoked: bool,
}

// ----- items -----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushRequest {
    pub items: Vec<SyncItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResult {
    pub id: String,
    /// "ok" or "conflict".
    pub status: String,
    /// On conflict, the server's current copy.
    #[serde(default)]
    pub current: Option<SyncItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub results: Vec<PushResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub items: Vec<SyncItem>,
    /// Opaque cursor for the next pull.
    pub cursor: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpSetupResponse {
    pub secret_base32: String,
    pub otpauth_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpConfirm {
    pub code: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}

// ----- devices -----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceInfo>,
}

// ----- org / team administration -----

/// Role of a user within an org.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    #[serde(default)]
    pub role: Option<OrgRole>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateInviteResponse {
    pub code: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgUserInfo {
    pub user_id: String,
    pub email: String,
    pub role: OrgRole,
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgUsersResponse {
    pub users: Vec<OrgUserInfo>,
}

// ----- team vaults -----

/// One member's sealed copy of a (team) vault key, uploaded by the client
/// that created or is extending the share. The server never sees the vault
/// key itself, only this per-recipient sealed box.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultMemberSeal {
    pub user_id: String,
    #[serde(with = "crate::b64")]
    pub sealed_vault_key: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateVaultRequest {
    pub vault_id: String,
    pub key_id: String,
    pub name: String,
    pub members: Vec<VaultMemberSeal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddVaultMemberRequest {
    pub user_id: String,
    pub key_id: String,
    #[serde(with = "crate::b64")]
    pub sealed_vault_key: Vec<u8>,
}

/// Public halves of an account's key pair, looked up by email so a client
/// can seal a team vault key to a new teammate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicKeysResponse {
    pub user_id: String,
    pub enc_public: String,
    pub sign_public: String,
}

/// Generic acknowledgement carrying the id of whatever was just created.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdResponse {
    pub id: String,
}
