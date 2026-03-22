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
            login_adapter: match env::var("LOGIN_ADAPTER")
                .unwrap_or_else(|_| "google".into())
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
    use super::llm_settings_encryption_key;

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
