use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Redirect;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::domain::ports::login_provider::{LoginPageContext, LoginProvider};
use crate::web::pages::auth_shared::{build_session_cookie, origin_from_headers};
use crate::web::state::AppState;

pub struct LocalPasswordLoginProvider;

impl LoginProvider for LocalPasswordLoginProvider {
    fn routes(&self) -> Router<AppState> {
        Router::new().route("/auth/local-login", axum::routing::post(local_login))
    }

    fn login_page_context(&self) -> LoginPageContext {
        LoginPageContext {
            provider_name: "local_password".to_string(),
        }
    }
}

#[derive(Deserialize)]
struct LocalLoginForm {
    email: String,
    password: String,
}

async fn local_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<LocalLoginForm>,
) -> Result<(CookieJar, Redirect), Redirect> {
    let (_, token) = state
        .auth
        .local_login(&form.email, &form.password)
        .await
        .map_err(|_| Redirect::to("/auth/login?error=Invalid+email+or+password"))?;

    let origin = origin_from_headers(&headers, &state.config);
    let cookie = build_session_cookie(&origin, token);

    Ok((jar.add(cookie), Redirect::to("/")))
}
