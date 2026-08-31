use axum::Router;
use tower_http::services::ServeDir;

use crate::web::state::AppState;

#[derive(serde::Serialize)]
struct ReleaseInfo {
    version: &'static str,
    git_sha: &'static str,
}

async fn release_info() -> axum::Json<ReleaseInfo> {
    axum::Json(ReleaseInfo {
        version: env!("CARGO_PKG_VERSION"),
        git_sha: option_env!("BOOPMARK_GIT_SHA").unwrap_or("unknown"),
    })
}

pub fn create_router(state: AppState) -> Router {
    let login_routes = state.login_provider.routes();

    Router::new()
        // API routes
        .nest("/api/v1", super::api::routes())
        // Page routes
        .merge(super::pages::routes())
        // Login provider routes (Google OAuth or local password)
        .merge(login_routes)
        // Static files (checked-in assets: CSS, JS, etc.)
        .nest_service("/static", ServeDir::new("static"))
        // User-generated uploads (images, etc.)
        .nest_service("/uploads", ServeDir::new("uploads"))
        // Health check
        .route("/health", axum::routing::get(|| async { "ok" }))
        // Exact release deployed to this process
        .route("/version", axum::routing::get(release_info))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn release_info_uses_the_package_version() {
        let axum::Json(info) = release_info().await;
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.git_sha.is_empty());
    }
}
