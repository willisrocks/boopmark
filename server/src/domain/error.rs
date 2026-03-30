use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("already exists")]
    AlreadyExists,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Error message used when a Cloudflare challenge page is detected.
/// Shared between the scraper (which detects it) and BookmarkService (which checks for it).
pub const CF_CHALLENGE_MSG: &str = "blocked by Cloudflare challenge";
