//! Optional OIDC sign-in (`OIDC_ISSUER` / `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET`
//! / `OIDC_REDIRECT_URL`). A successful OIDC round trip only *identifies*
//! the user by email; the master password (over OPAQUE) is still required
//! to unlock the vault. See `docs/self-hosting.md`.
//!
//! Gated behind the default-on `oidc` cargo feature. Its dependency tree
//! (`openidconnect` -> `oauth2` -> `reqwest` -> `rustls`) resolves to the
//! `ring` crypto provider, never `aws-lc-sys`, which is why it's safe to
//! leave on by default; disable the feature to drop it entirely, in which
//! case the `/v1/oidc/*` routes always answer 501.

use anyhow::{anyhow, Context, Result};

use crate::config::OidcConfig;

#[cfg(feature = "oidc")]
mod real {
    use super::*;
    use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
    use openidconnect::reqwest;
    use openidconnect::{
        AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
        EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
        TokenResponse,
    };

    /// What `CoreProviderMetadata::discover_async` + `from_provider_metadata`
    /// actually produces: the authorization endpoint is always set by
    /// discovery, device-auth/introspection/revocation endpoints are never
    /// set (OpenID discovery doesn't define them), and the token/userinfo
    /// endpoints are "maybe set" (present if the provider advertised them).
    type DiscoveredClient = CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >;

    pub struct OidcClient {
        client: DiscoveredClient,
        http: reqwest::Client,
    }

    /// One pending `/v1/oidc/start` -> `/v1/oidc/callback` round trip's
    /// client-opaque state, persisted in the `oidc_requests` table.
    pub struct OidcRequestState {
        pub csrf_token: String,
        pub nonce: String,
        pub pkce_verifier: String,
    }

    impl OidcClient {
        pub async fn discover(cfg: &OidcConfig) -> Result<Self> {
            let http = reqwest::ClientBuilder::new()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .context("failed to build the OIDC HTTP client")?;
            let issuer = IssuerUrl::new(cfg.issuer.clone()).context("invalid OIDC_ISSUER")?;
            let metadata = CoreProviderMetadata::discover_async(issuer, &http)
                .await
                .context("OIDC discovery failed")?;
            let client = CoreClient::from_provider_metadata(
                metadata,
                ClientId::new(cfg.client_id.clone()),
                Some(ClientSecret::new(cfg.client_secret.clone())),
            )
            .set_redirect_uri(
                RedirectUrl::new(cfg.redirect_url.clone()).context("invalid OIDC_REDIRECT_URL")?,
            );
            Ok(Self { client, http })
        }

        /// Builds the provider authorization URL and the state to persist
        /// until the callback arrives.
        pub fn authorize_url(&self) -> (String, OidcRequestState) {
            let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
            let (url, csrf_token, nonce) = self
                .client
                .authorize_url(
                    CoreAuthenticationFlow::AuthorizationCode,
                    CsrfToken::new_random,
                    Nonce::new_random,
                )
                .add_scope(Scope::new("openid".to_string()))
                .add_scope(Scope::new("email".to_string()))
                .set_pkce_challenge(pkce_challenge)
                .url();
            (
                url.to_string(),
                OidcRequestState {
                    csrf_token: csrf_token.secret().clone(),
                    nonce: nonce.secret().clone(),
                    pkce_verifier: pkce_verifier.secret().clone(),
                },
            )
        }

        /// Exchanges the authorization `code` and verifies the ID token,
        /// returning the verified email address.
        pub async fn exchange(
            &self,
            code: String,
            pkce_verifier: String,
            nonce: String,
        ) -> Result<String> {
            let token_response = self
                .client
                .exchange_code(AuthorizationCode::new(code))
                .map_err(|e| anyhow!("failed to build token exchange request: {e}"))?
                .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
                .request_async(&self.http)
                .await
                .map_err(|e| anyhow!("token exchange with the OIDC provider failed: {e}"))?;

            let id_token = token_response
                .id_token()
                .ok_or_else(|| anyhow!("OIDC provider did not return an id_token"))?;
            let verifier = self.client.id_token_verifier();
            let claims = id_token
                .claims(&verifier, &Nonce::new(nonce))
                .context("id_token verification failed")?;
            let email = claims
                .email()
                .ok_or_else(|| anyhow!("OIDC provider did not return an email claim"))?;
            Ok(email.as_str().to_string())
        }
    }
}

#[cfg(not(feature = "oidc"))]
mod stub {
    use super::*;

    pub struct OidcClient;

    pub struct OidcRequestState {
        pub csrf_token: String,
        pub nonce: String,
        pub pkce_verifier: String,
    }

    impl OidcClient {
        pub async fn discover(_cfg: &OidcConfig) -> Result<Self> {
            Err(anyhow!(
                "this build was compiled without the `oidc` cargo feature"
            ))
        }

        pub fn authorize_url(&self) -> (String, OidcRequestState) {
            unreachable!("OidcClient can never be constructed without the `oidc` feature")
        }

        pub async fn exchange(
            &self,
            _code: String,
            _pkce_verifier: String,
            _nonce: String,
        ) -> Result<String> {
            unreachable!("OidcClient can never be constructed without the `oidc` feature")
        }
    }
}

#[cfg(feature = "oidc")]
pub use real::{OidcClient, OidcRequestState};
#[cfg(not(feature = "oidc"))]
pub use stub::{OidcClient, OidcRequestState};
