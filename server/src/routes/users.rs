use axum::extract::{Path, State};
use axum::Json;
use ourshell_proto::sync::PublicKeysResponse;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::routes::register::normalize_email;
use crate::sqlx;
use crate::state::AppState;

/// Looks up a teammate's public keys by email, so a client can seal a team
/// vault key to them. Scoped to the caller's own org.
pub async fn public_keys(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(email): Path<String>,
) -> AppResult<Json<PublicKeysResponse>> {
    let email = normalize_email(&email);
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT ak.user_id, ak.enc_public, ak.sign_public \
         FROM account_keys ak JOIN users u ON u.id = ak.user_id \
         WHERE u.email = $1 AND u.org_id = $2",
    )
    .bind(&email)
    .bind(auth.org_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((user_id, enc_public, sign_public)) = row else {
        return Err(AppError::not_found(
            "user_not_found",
            "no such user in this org",
        ));
    };
    Ok(Json(PublicKeysResponse {
        user_id: user_id.to_string(),
        enc_public,
        sign_public,
    }))
}
