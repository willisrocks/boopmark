use axum::http::HeaderMap;

/// Returns `true` if the request was made by HTMX (has `HX-Request` header).
pub fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}
