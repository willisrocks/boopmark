use axum::Router;

use crate::web::state::AppState;

pub struct LoginPageContext {
    pub provider_name: String,
}

pub trait LoginProvider: Send + Sync + 'static {
    fn routes(&self) -> Router<AppState>;
    fn login_page_context(&self) -> LoginPageContext;
}
