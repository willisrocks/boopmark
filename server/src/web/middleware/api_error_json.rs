use axum::body::Body;
use axum::http::{Request, Response, header};
use axum::middleware::Next;
use axum::response::IntoResponse;
use serde_json::json;

/// Middleware that transforms bare error status code responses (4xx/5xx with
/// empty body) into JSON `{"error": "..."}` responses. This ensures API
/// clients always receive structured errors, even from extractors like
/// `AuthUser` that return bare status codes.
pub async fn json_error_layer(
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let response = next.run(request).await;
    let status = response.status();

    // Only transform error responses (4xx and 5xx)
    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }

    // If the response already has a content-type (i.e., it already has a
    // JSON body from error_response or similar), leave it alone.
    if response.headers().contains_key(header::CONTENT_TYPE) {
        return response;
    }

    // Build a JSON error body from the status code's canonical reason phrase
    let reason = status.canonical_reason().unwrap_or("error");
    let body = json!({"error": reason.to_lowercase()});
    (status, axum::Json(body)).into_response()
}
