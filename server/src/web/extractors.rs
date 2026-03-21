use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;

use crate::domain::user::User;
use crate::web::state::AppState;

/// Extractor that requires an authenticated user.
///
/// Checks the `Authorization: Bearer <token>` header first (API key),
/// then falls back to the `session` cookie.
pub struct AuthUser(pub User);

/// Extractor that optionally resolves an authenticated user.
///
/// Returns `None` for unauthenticated requests instead of rejecting them.
pub struct MaybeUser(pub Option<User>);

async fn resolve_user(parts: &mut Parts, state: &AppState) -> Option<User> {
    // 1. Check Authorization header for API key
    if let Some(auth_header) = parts.headers.get("authorization")
        && let Ok(value) = auth_header.to_str()
        && let Some(token) = value.strip_prefix("Bearer ")
        && let Ok(user) = state.auth.validate_api_key(token).await
    {
        return Some(user);
    }

    // 2. Fall back to session cookie
    let jar = CookieJar::from_headers(&parts.headers);
    if let Some(cookie) = jar.get("session")
        && let Ok(user) = state.auth.validate_session(cookie.value()).await
    {
        return Some(user);
    }

    None
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = axum::http::StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        resolve_user(parts, state)
            .await
            .map(AuthUser)
            .ok_or(axum::http::StatusCode::UNAUTHORIZED)
    }
}

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(resolve_user(parts, state).await))
    }
}

/// Requires authenticated user with owner or admin role.
/// Returns 403 Forbidden if the user doesn't have the required role.
#[allow(dead_code)]
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = axum::http::StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if user.is_admin_or_owner() {
            Ok(AdminUser(user))
        } else {
            Err(axum::http::StatusCode::FORBIDDEN)
        }
    }
}
