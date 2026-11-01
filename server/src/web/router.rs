use axum::Router;
use axum::http::{HeaderValue, header};
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

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

/// User-generated files, served from the app's own origin. They are images
/// fetched from other sites, so a file must never run as a document of ours:
/// a sandboxing CSP stops any script or embedded content even if a file is
/// opened directly, and `nosniff` holds the browser to the extension's type.
fn uploads_router<S: Clone + Send + Sync + 'static>(dir: &str) -> Router<S> {
    let service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .service(ServeDir::new(dir));
    Router::new().fallback_service(service)
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
        .nest("/uploads", uploads_router("uploads"))
        // Health check
        .route("/health", axum::routing::get(|| async { "ok" }))
        // Exact release deployed to this process
        .route("/version", axum::routing::get(release_info))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn release_info_uses_the_package_version() {
        let axum::Json(info) = release_info().await;
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.git_sha.is_empty());
    }

    #[tokio::test]
    async fn uploads_are_served_sandboxed_and_unsniffed() {
        let dir = std::env::temp_dir().join(format!("boopmark-uploads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("images")).unwrap();
        // An SVG stored before fetched images refused them.
        std::fs::write(
            dir.join("images/old.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        )
        .unwrap();
        std::fs::write(dir.join("images/cat.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let app: Router = Router::new().nest("/uploads", uploads_router(dir.to_str().unwrap()));

        for path in ["/uploads/images/old.svg", "/uploads/images/cat.png"] {
            let resp = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(
                resp.headers()[header::CONTENT_SECURITY_POLICY],
                "default-src 'none'; sandbox",
                "{path}"
            );
            assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        }
        // Errors carry the headers too.
        let missing = app
            .oneshot(
                Request::get("/uploads/images/missing.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
