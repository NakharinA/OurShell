//! Thin HTTP client for the server's `/v1/...` API (`ourshell_proto::sync`).
//!
//! This module only moves bytes; it never looks inside an envelope or a
//! wrapped/sealed key. All the cryptography lives in `super` (OPAQUE calls,
//! building the account/vault upload) and in `crate::vault`.

use ourshell_proto::sync::{
    AccountPasswordFinish, AccountPasswordStart, AccountPasswordStartResponse,
    AccountVaultsResponse, AddVaultMemberRequest, ApiError, CreateInviteRequest,
    CreateInviteResponse, CreateVaultRequest, DeviceListResponse, IdResponse, LoginFinish,
    LoginStart, LoginStartResponse, OrgUsersResponse, PublicKeysResponse, PullResponse,
    PushRequest, PushResponse, RegisterFinish, RegisterStart, RegisterStartResponse,
    SessionResponse, TotpConfirm, TotpSetupResponse, VaultMembersResponse, API_PREFIX,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::error::{Result, SyncError};

#[derive(Clone)]
pub struct SyncClient {
    http: reqwest::Client,
    base_url: String,
}

impl SyncClient {
    pub fn new(server_url: impl Into<String>) -> Self {
        let base_url = server_url.into();
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// The server URL this client was built with (no trailing slash).
    pub fn server_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}{API_PREFIX}{path}", self.base_url)
    }

    async fn send<Req, Res>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&Req>,
    ) -> Result<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let mut req = self.http.request(method, self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if status.is_success() {
            let text = if text.trim().is_empty() {
                "null"
            } else {
                text.as_str()
            };
            serde_json::from_str(text).map_err(SyncError::Json)
        } else {
            let (code, message) = serde_json::from_str::<ApiError>(&text)
                .map(|e| (e.error, e.message))
                .unwrap_or_else(|_| ("http_error".to_string(), text));
            Err(SyncError::Api {
                status: status.as_u16(),
                code,
                message,
            })
        }
    }

    pub async fn register_start(&self, body: &RegisterStart) -> Result<RegisterStartResponse> {
        self.send(Method::POST, "/register/start", None, Some(body))
            .await
    }

    pub async fn register_finish(&self, body: &RegisterFinish) -> Result<SessionResponse> {
        self.send(Method::POST, "/register/finish", None, Some(body))
            .await
    }

    pub async fn login_start(&self, body: &LoginStart) -> Result<LoginStartResponse> {
        self.send(Method::POST, "/login/start", None, Some(body))
            .await
    }

    pub async fn login_finish(&self, body: &LoginFinish) -> Result<SessionResponse> {
        self.send(Method::POST, "/login/finish", None, Some(body))
            .await
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        self.send::<(), ()>(Method::POST, "/logout", Some(token), None)
            .await
    }

    pub async fn devices(&self, token: &str) -> Result<DeviceListResponse> {
        self.send::<(), _>(Method::GET, "/devices", Some(token), None)
            .await
    }

    pub async fn revoke_device(&self, token: &str, device_id: &str) -> Result<()> {
        let path = format!("/devices/{device_id}/revoke");
        self.send::<(), ()>(Method::POST, &path, Some(token), None)
            .await
    }

    pub async fn totp_setup(&self, token: &str) -> Result<TotpSetupResponse> {
        self.send::<(), _>(Method::POST, "/totp/setup", Some(token), None)
            .await
    }

    pub async fn totp_confirm(&self, token: &str, code: &str) -> Result<()> {
        self.send::<_, ()>(
            Method::POST,
            "/totp/confirm",
            Some(token),
            Some(&TotpConfirm {
                code: code.to_string(),
            }),
        )
        .await
    }

    pub async fn totp_disable(&self, token: &str, code: &str) -> Result<()> {
        self.send::<_, ()>(
            Method::POST,
            "/totp/disable",
            Some(token),
            Some(&TotpConfirm {
                code: code.to_string(),
            }),
        )
        .await
    }

    pub async fn push(
        &self,
        token: &str,
        vault_id: &str,
        body: &PushRequest,
    ) -> Result<PushResponse> {
        let path = format!("/vaults/{vault_id}/items");
        self.send(Method::POST, &path, Some(token), Some(body))
            .await
    }

    pub async fn pull(&self, token: &str, vault_id: &str, since: i64) -> Result<PullResponse> {
        let path = format!("/vaults/{vault_id}/items?since={since}");
        self.send::<(), _>(Method::GET, &path, Some(token), None)
            .await
    }

    pub async fn account_vaults(&self, token: &str) -> Result<AccountVaultsResponse> {
        self.send::<(), _>(Method::GET, "/account/vaults", Some(token), None)
            .await
    }

    // ----- account password change / recovery-while-signed-in -----

    pub async fn account_password_start(
        &self,
        token: &str,
        body: &AccountPasswordStart,
    ) -> Result<AccountPasswordStartResponse> {
        self.send(
            Method::POST,
            "/account/password/start",
            Some(token),
            Some(body),
        )
        .await
    }

    pub async fn account_password_finish(
        &self,
        token: &str,
        body: &AccountPasswordFinish,
    ) -> Result<()> {
        self.send(
            Method::POST,
            "/account/password/finish",
            Some(token),
            Some(body),
        )
        .await
    }

    // ----- team vaults / org admin -----

    pub async fn public_keys(&self, token: &str, email: &str) -> Result<PublicKeysResponse> {
        let path = format!("/users/{email}/public-keys");
        self.send::<(), _>(Method::GET, &path, Some(token), None)
            .await
    }

    pub async fn create_vault(&self, token: &str, body: &CreateVaultRequest) -> Result<IdResponse> {
        self.send(Method::POST, "/vaults", Some(token), Some(body))
            .await
    }

    pub async fn vault_members(&self, token: &str, vault_id: &str) -> Result<VaultMembersResponse> {
        let path = format!("/vaults/{vault_id}/members");
        self.send::<(), _>(Method::GET, &path, Some(token), None)
            .await
    }

    pub async fn add_vault_member(
        &self,
        token: &str,
        vault_id: &str,
        body: &AddVaultMemberRequest,
    ) -> Result<()> {
        let path = format!("/vaults/{vault_id}/members");
        self.send(Method::POST, &path, Some(token), Some(body))
            .await
    }

    pub async fn remove_vault_member(
        &self,
        token: &str,
        vault_id: &str,
        user_id: &str,
    ) -> Result<()> {
        let path = format!("/vaults/{vault_id}/members/{user_id}");
        self.send::<(), ()>(Method::DELETE, &path, Some(token), None)
            .await
    }

    pub async fn create_invite(
        &self,
        token: &str,
        body: &CreateInviteRequest,
    ) -> Result<CreateInviteResponse> {
        self.send(Method::POST, "/orgs/invites", Some(token), Some(body))
            .await
    }

    pub async fn org_users(&self, token: &str) -> Result<OrgUsersResponse> {
        self.send::<(), _>(Method::GET, "/orgs/users", Some(token), None)
            .await
    }

    pub async fn disable_user(&self, token: &str, user_id: &str) -> Result<()> {
        let path = format!("/orgs/users/{user_id}/disable");
        self.send::<(), ()>(Method::POST, &path, Some(token), None)
            .await
    }
}
