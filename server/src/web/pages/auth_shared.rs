use askama::Template;
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar};

use crate::domain::ports::login_provider::AuthenticatedIdentity;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/invite_only.html")]
struct InviteOnlyPage;

/// Derive the origin (scheme + host) from request headers, falling back to config.app_url.
pub fn origin_from_headers(headers: &HeaderMap, config: &crate::config::Config) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok());

    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");

    match host {
        Some(h) => format!("{proto}://{h}"),
        None => config.app_url.clone(),
    }
}

pub fn build_session_cookie(origin: &str, token: String) -> Cookie<'static> {
    Cookie::build(("session", token))
        .path("/")
        .http_only(true)
        .secure(origin.starts_with("https://"))
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build()
}

/// Called by login adapters after successful authentication.
/// Handles invite validation, user creation, and session management.
pub async fn handle_authenticated_identity(
    state: &AppState,
    origin: &str,
    identity: AuthenticatedIdentity,
    jar: CookieJar,
) -> (CookieJar, Response) {
    let invite_token = jar.get("invite_token").map(|c| c.value().to_string());

    let existing_user = state
        .auth
        .find_user_by_email(&identity.email)
        .await
        .ok()
        .flatten();

    match existing_user {
        Some(user) if !user.is_active() => (
            jar,
            Redirect::to("/auth/login?error=deactivated").into_response(),
        ),
        Some(user) => {
            // Existing active user — create session, no invite needed
            match state.auth.create_session(user.id).await {
                Ok(token) => {
                    let cookie = build_session_cookie(origin, token);
                    (jar.add(cookie), Redirect::to("/bookmarks").into_response())
                }
                Err(e) => {
                    tracing::error!("Failed to create session: {e}");
                    (
                        jar,
                        Redirect::to("/auth/login?error=internal").into_response(),
                    )
                }
            }
        }
        None => {
            // New user — need an invite
            match invite_token {
                Some(ref token) => {
                    match state.invites.validate_token(token).await.ok().flatten() {
                        Some(_invite) => {
                            match state
                                .auth
                                .upsert_user(
                                    identity.email,
                                    identity.name,
                                    identity.image,
                                )
                                .await
                            {
                                Ok(user) => {
                                    let _ = state.invites.claim_invite(token, user.id).await;
                                    match state.auth.create_session(user.id).await {
                                        Ok(session_token) => {
                                            let cookie =
                                                build_session_cookie(origin, session_token);
                                            let jar = jar
                                                .add(cookie)
                                                .remove(Cookie::build("invite_token").path("/"));
                                            (
                                                jar,
                                                Redirect::to("/bookmarks").into_response(),
                                            )
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to create session: {e}");
                                            (
                                                jar,
                                                Redirect::to("/auth/login?error=internal")
                                                    .into_response(),
                                            )
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to create user: {e}");
                                    (
                                        jar,
                                        Redirect::to("/auth/login?error=internal")
                                            .into_response(),
                                    )
                                }
                            }
                        }
                        None => {
                            // Invalid/expired invite token
                            let jar = jar.remove(Cookie::build("invite_token").path("/"));
                            (
                                jar,
                                Redirect::to("/auth/login?error=invite_invalid")
                                    .into_response(),
                            )
                        }
                    }
                }
                None => {
                    // No invite — show "invite only" page
                    let body = InviteOnlyPage
                        .render()
                        .unwrap_or_default();
                    (jar, Html(body).into_response())
                }
            }
        }
    }
}
