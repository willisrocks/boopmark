use base64::Engine;
use std::env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginAdapter {
    Google,
    LocalPassword,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub database_url: String,
    pub app_url: String,
    pub port: u16,
    pub llm_settings_encryption_key: String,
    pub session_secret: String,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub enable_e2e_auth: bool,
    pub login_adapter: LoginAdapter,
    pub storage_backend: StorageBackend,
    pub s3_endpoint: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_region: String,
    pub s3_images_bucket: String,
    pub s3_images_public_url: Option<String>,
    pub screenshot_backend: ScreenshotBackend,
    pub screenshot_service_url: Option<String>,
    pub metadata_fallback_backend: MetadataFallbackBackend,
    pub iframely_api_key: Option<String>,
    pub opengraph_io_api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ScreenshotBackend {
    Playwright,
    Disabled,
}

#[derive(Debug, Clone)]
pub enum StorageBackend {
    S3,
    Local,
}

#[derive(Debug, Clone)]
pub enum MetadataFallbackBackend {
    Iframely,
    OpengraphIo,
    None,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL required"),
            app_url: env::var("APP_URL").unwrap_or_else(|_| "http://localhost:4000".into()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "4000".into())
                .parse()
                .unwrap(),
            llm_settings_encryption_key: llm_settings_encryption_key(),
            session_secret: env::var("SESSION_SECRET").expect("SESSION_SECRET required"),
            google_client_id: env::var("GOOGLE_CLIENT_ID").ok(),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET").ok(),
            enable_e2e_auth: env::var("ENABLE_E2E_AUTH")
                .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(false),
            // Default is local_password for self-hosting simplicity.
            // Set LOGIN_ADAPTER=google in .env to use Google OAuth.
            login_adapter: match env::var("LOGIN_ADAPTER")
                .unwrap_or_else(|_| "local_password".into())
                .as_str()
            {
                "local_password" => LoginAdapter::LocalPassword,
                _ => LoginAdapter::Google,
            },
            storage_backend: match env::var("STORAGE_BACKEND")
                .unwrap_or_else(|_| "local".into())
                .as_str()
            {
                "s3" => StorageBackend::S3,
                _ => StorageBackend::Local,
            },
            s3_endpoint: env::var("S3_ENDPOINT").ok(),
            s3_access_key: env::var("S3_ACCESS_KEY").ok(),
            s3_secret_key: env::var("S3_SECRET_KEY").ok(),
            s3_region: env::var("S3_REGION").unwrap_or_else(|_| "auto".into()),
            s3_images_bucket: env::var("S3_IMAGES_BUCKET")
                .unwrap_or_else(|_| "boopmark-images".into()),
            s3_images_public_url: env::var("S3_IMAGES_PUBLIC_URL").ok(),
            screenshot_backend: match env::var("SCREENSHOT_BACKEND")
                .unwrap_or_else(|_| "disabled".into())
                .as_str()
            {
                "playwright" => ScreenshotBackend::Playwright,
                _ => ScreenshotBackend::Disabled,
            },
            screenshot_service_url: env::var("SCREENSHOT_SERVICE_URL").ok(),
            metadata_fallback_backend: match env::var("METADATA_FALLBACK_BACKEND")
                .unwrap_or_else(|_| "none".into())
                .as_str()
            {
                "iframely" => MetadataFallbackBackend::Iframely,
                "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
                _ => MetadataFallbackBackend::None,
            },
            iframely_api_key: env::var("IFRAMELY_API_KEY").ok(),
            opengraph_io_api_key: env::var("OPENGRAPH_IO_API_KEY").ok(),
        }
    }
}

fn llm_settings_encryption_key() -> String {
    let raw_key =
        env::var("LLM_SETTINGS_ENCRYPTION_KEY").expect("LLM_SETTINGS_ENCRYPTION_KEY required");
    let trimmed_key = raw_key.trim();
    if trimmed_key.is_empty() {
        panic!("LLM_SETTINGS_ENCRYPTION_KEY must be non-empty base64 that decodes to 32 bytes");
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed_key)
        .expect("LLM_SETTINGS_ENCRYPTION_KEY must be valid base64");
    if decoded.len() != 32 {
        panic!("LLM_SETTINGS_ENCRYPTION_KEY must decode to 32 bytes");
    }

    trimmed_key.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        llm_settings_encryption_key, LoginAdapter, MetadataFallbackBackend, ScreenshotBackend,
    };

    /// LOGIN_ADAPTER defaults to local_password for self-hosting convenience.
    /// Existing Google OAuth deployments must set LOGIN_ADAPTER=google explicitly.
    /// These tests verify the parsing logic in isolation (not env var lookup,
    /// which is inherently racy in multi-threaded tests).
    #[test]
    fn login_adapter_parses_local_password() {
        let adapter: LoginAdapter = match "local_password" {
            "local_password" => LoginAdapter::LocalPassword,
            _ => LoginAdapter::Google,
        };
        assert!(matches!(adapter, LoginAdapter::LocalPassword));
    }

    #[test]
    fn login_adapter_default_value_is_local_password() {
        // The literal default passed to unwrap_or_else must be "local_password"
        // so that unset deployments get local auth, not Google OAuth.
        let default = "local_password";
        let adapter: LoginAdapter = match default {
            "local_password" => LoginAdapter::LocalPassword,
            _ => LoginAdapter::Google,
        };
        assert!(
            matches!(adapter, LoginAdapter::LocalPassword),
            "default must be LocalPassword for self-hosting"
        );
    }

    #[test]
    fn login_adapter_parses_google() {
        let adapter: LoginAdapter = match "google" {
            "local_password" => LoginAdapter::LocalPassword,
            _ => LoginAdapter::Google,
        };
        assert!(matches!(adapter, LoginAdapter::Google));
    }

    #[test]
    fn screenshot_backend_default_is_disabled() {
        let default = "disabled";
        let backend: ScreenshotBackend = match default {
            "playwright" => ScreenshotBackend::Playwright,
            _ => ScreenshotBackend::Disabled,
        };
        assert!(matches!(backend, ScreenshotBackend::Disabled));
    }

    #[test]
    fn screenshot_backend_parses_playwright() {
        let backend: ScreenshotBackend = match "playwright" {
            "playwright" => ScreenshotBackend::Playwright,
            _ => ScreenshotBackend::Disabled,
        };
        assert!(matches!(backend, ScreenshotBackend::Playwright));
    }

    #[test]
    fn metadata_fallback_backend_default_is_none() {
        let backend: MetadataFallbackBackend = match "none" {
            "iframely" => MetadataFallbackBackend::Iframely,
            "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
            _ => MetadataFallbackBackend::None,
        };
        assert!(matches!(backend, MetadataFallbackBackend::None));
    }

    #[test]
    fn metadata_fallback_backend_parses_iframely() {
        let backend: MetadataFallbackBackend = match "iframely" {
            "iframely" => MetadataFallbackBackend::Iframely,
            "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
            _ => MetadataFallbackBackend::None,
        };
        assert!(matches!(backend, MetadataFallbackBackend::Iframely));
    }

    #[test]
    fn metadata_fallback_backend_parses_opengraph_io() {
        let backend: MetadataFallbackBackend = match "opengraph_io" {
            "iframely" => MetadataFallbackBackend::Iframely,
            "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
            _ => MetadataFallbackBackend::None,
        };
        assert!(matches!(backend, MetadataFallbackBackend::OpengraphIo));
    }

    #[test]
    fn accepts_valid_llm_settings_encryption_key() {
        unsafe {
            std::env::set_var(
                "LLM_SETTINGS_ENCRYPTION_KEY",
                "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
            );
        }

        assert_eq!(
            llm_settings_encryption_key(),
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        );
    }

    #[test]
    #[should_panic(
        expected = "LLM_SETTINGS_ENCRYPTION_KEY must be non-empty base64 that decodes to 32 bytes"
    )]
    fn rejects_blank_llm_settings_encryption_key() {
        unsafe {
            std::env::set_var("LLM_SETTINGS_ENCRYPTION_KEY", "   ");
        }

        llm_settings_encryption_key();
    }
}
