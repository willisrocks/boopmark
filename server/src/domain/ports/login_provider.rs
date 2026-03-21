use axum::Router;

use crate::web::state::AppState;

pub struct LoginPageContext {
    pub provider_name: String,
}

/// Identity information extracted by a login provider after successful authentication.
#[allow(dead_code)]
pub struct AuthenticatedIdentity {
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
}

pub trait LoginProvider: Send + Sync + 'static {
    fn routes(&self) -> Router<AppState>;
    fn login_page_context(&self) -> LoginPageContext;
}
