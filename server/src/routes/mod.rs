pub mod account;
pub mod admin;
pub mod devices;
pub mod health;
pub mod items;
pub mod login;
pub mod oidc;
pub mod register;
pub mod totp;
pub mod users;
pub mod vaults;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use ourshell_proto::sync::API_PREFIX;
use tower_http::trace::TraceLayer;

use crate::rate_limit::rate_limit_middleware;
use crate::state::AppState;

/// 1 MiB. Individual item envelopes are capped separately and much lower
/// (see `routes::items::MAX_ENVELOPE_BYTES`); this is just a sane ceiling
/// against someone sending an absurd request body.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub fn build_router(state: AppState) -> Router {
    let auth_routes = Router::new()
        .route("/register/start", post(register::start))
        .route("/register/finish", post(register::finish))
        .route("/login/start", post(login::start))
        .route("/login/finish", post(login::finish))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ));

    let api = Router::new()
        .route("/health", get(health::health))
        .merge(auth_routes)
        .route("/logout", post(login::logout))
        .route("/devices", get(devices::list))
        .route("/devices/{device_id}/revoke", post(devices::revoke))
        .route("/totp/setup", post(totp::setup))
        .route("/totp/confirm", post(totp::confirm))
        .route("/totp/disable", post(totp::disable))
        .route("/account/vaults", get(account::vaults))
        .route("/account/password/start", post(account::start))
        .route("/account/password/finish", post(account::finish))
        .route(
            "/vaults/{vault_id}/items",
            get(items::pull).post(items::push),
        )
        .route("/orgs/invites", post(admin::create_invite))
        .route("/orgs/users", get(admin::list_users))
        .route("/orgs/users/{user_id}/disable", post(admin::disable_user))
        .route("/vaults", post(vaults::create))
        .route(
            "/vaults/{vault_id}/members",
            get(vaults::list_members).post(vaults::add_member),
        )
        .route(
            "/vaults/{vault_id}/members/{user_id}",
            delete(vaults::remove_member),
        )
        .route("/users/{email}/public-keys", get(users::public_keys))
        .route("/oidc/start", get(oidc::start))
        .route("/oidc/callback", get(oidc::callback));

    Router::new()
        .nest(API_PREFIX, api)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
