//! Bearer-token session authentication. The token is a random 32-byte
//! value; only its SHA-256 is ever stored or compared.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::Utc;
use uuid::Uuid;

use crate::crypto::hash_token;
use crate::error::AppError;
use crate::sqlx;
use crate::state::AppState;

/// The authenticated user + device + org for the current request, resolved
/// from the `Authorization: Bearer <token>` header.
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub device_id: Uuid,
    pub role: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::unauthorized("missing Authorization header"))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::unauthorized("expected a Bearer token"))?;
        let token_hash = hash_token(token);

        type SessionRow = (Uuid, Uuid, Uuid, Uuid, String, bool, chrono::DateTime<Utc>);
        let row: Option<SessionRow> = sqlx::query_as(
            r#"
            SELECT s.id, s.user_id, s.org_id, s.device_id, u.role, u.disabled, s.expires_at
            FROM sessions s
            JOIN users u ON u.id = s.user_id
            WHERE s.token_hash = $1
            "#,
        )
        .bind(&token_hash)
        .fetch_optional(state.pool())
        .await?;

        let Some((session_id, user_id, org_id, device_id, role, disabled, expires_at)) = row else {
            return Err(AppError::unauthorized("invalid or expired session"));
        };
        if disabled {
            return Err(AppError::forbidden("user_disabled", "account is disabled"));
        }
        if expires_at < Utc::now() {
            return Err(AppError::unauthorized("invalid or expired session"));
        }

        Ok(AuthUser {
            session_id,
            user_id,
            org_id,
            device_id,
            role,
        })
    }
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "owner" || self.role == "admin"
    }

    pub fn require_admin(&self) -> Result<(), AppError> {
        if self.is_admin() {
            Ok(())
        } else {
            Err(AppError::forbidden("forbidden", "admin role required"))
        }
    }
}
