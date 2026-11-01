//! HTTP plumbing shared by the vendor adapters behind the `LlmClient` and
//! `Embedder` ports: one JSON POST, and one mapping from failures to the
//! ports' error classes (`Internal` = retryable, `InvalidInput` = final).
//! Messages carry the vendor and HTTP status only, never a key, URL or body.

// Only the port adapters use this, and nothing calls them yet; remove with
// the allow in `llm_client`.
#![allow(dead_code)]

use crate::domain::error::DomainError;
use reqwest::StatusCode;
use serde_json::Value;

/// Whose API key a request carries, which decides whether a rejected key is
/// final: a user fixes their own key in Settings, while an operator key is
/// rotated out of band and the work should retry once it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOwner {
    User,
    Operator,
}

/// Statuses where the same request can succeed later: the SDKs' retry set,
/// plus a rejected key when the operator owns it.
fn is_retryable_status(status: StatusCode, owner: KeyOwner) -> bool {
    match status {
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS => true,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => owner == KeyOwner::Operator,
        _ => !status.is_client_error(),
    }
}

pub fn http_status_error(vendor: &str, status: StatusCode, owner: KeyOwner) -> DomainError {
    let message = format!("{vendor} API returned HTTP {status}");
    if is_retryable_status(status, owner) {
        DomainError::Internal(message)
    } else {
        DomainError::InvalidInput(message)
    }
}

/// reqwest errors can embed the URL, so only the kind of failure is
/// reported. A request that cannot be built (a key that is not a valid
/// header value) fails the same way every time.
pub fn transport_error(vendor: &str, error: &reqwest::Error) -> DomainError {
    if error.is_builder() {
        DomainError::InvalidInput(format!("{vendor} API request could not be built"))
    } else if error.is_timeout() {
        DomainError::Internal(format!("{vendor} API request timed out"))
    } else {
        DomainError::Internal(format!("{vendor} API request failed"))
    }
}

/// Sends `body` as JSON and returns the parsed JSON answer of a 2xx response.
pub async fn post_json(
    request: reqwest::RequestBuilder,
    body: &Value,
    vendor: &str,
    owner: KeyOwner,
) -> Result<Value, DomainError> {
    let response = request
        .json(body)
        .send()
        .await
        .map_err(|error| transport_error(vendor, &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(http_status_error(vendor, status, owner));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| transport_error(vendor, &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| DomainError::Internal(format!("{vendor} returned a malformed response")))
}

/// A token count at `pointer`, 0 when absent, clamped to `u32`.
pub fn token_count(payload: &Value, pointer: &str) -> u32 {
    payload
        .pointer(pointer)
        .and_then(Value::as_u64)
        .map_or(0, |tokens| u32::try_from(tokens).unwrap_or(u32::MAX))
}

/// Fake vendors and assertions shared by the adapter tests.
#[cfg(test)]
pub mod test_support {
    use crate::domain::error::DomainError;
    use axum::Router;
    use tokio::net::TcpListener;

    /// Planted in fake vendor bodies; must never appear in an error.
    pub const SECRET_BODY: &str = "secret detail from the vendor";

    /// Serves `router` on a free local port and returns `http://host:port`.
    pub async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    /// A base URL nothing listens on: the listener is bound, then dropped.
    pub async fn closed_port() -> String {
        let address = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        format!("http://{address}")
    }

    /// An environment lookup over fixed pairs.
    pub fn lookup(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    /// Asserts the error class and that the message names `needle` but
    /// leaks neither the planted body nor the test key.
    pub fn assert_error<T: std::fmt::Debug>(
        result: Result<T, DomainError>,
        retryable: bool,
        needle: &str,
    ) {
        let message = match (result, retryable) {
            (Err(DomainError::Internal(message)), true) => message,
            (Err(DomainError::InvalidInput(message)), false) => message,
            (other, _) => panic!("expected retryable={retryable}, got {other:?}"),
        };
        assert!(message.contains(needle), "{message}");
        assert!(!message.contains(SECRET_BODY));
        assert!(!message.contains("sk-test"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_keys_are_final_only_for_user_keys() {
        let retryable = |code: u16, owner| {
            matches!(
                http_status_error("V", StatusCode::from_u16(code).unwrap(), owner),
                DomainError::Internal(_)
            )
        };
        for code in [401, 403] {
            assert!(!retryable(code, KeyOwner::User), "{code}");
            assert!(retryable(code, KeyOwner::Operator), "{code}");
        }
        for owner in [KeyOwner::User, KeyOwner::Operator] {
            for code in [408, 409, 429, 500, 503, 529] {
                assert!(retryable(code, owner), "{code}");
            }
            for code in [400, 404, 422] {
                assert!(!retryable(code, owner), "{code}");
            }
        }
    }
}
