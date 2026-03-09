use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub app_url: String,
    pub port: u16,
    pub llm_settings_encryption_key: String,
    pub session_secret: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub enable_e2e_auth: bool,
    pub storage_backend: StorageBackend,
    pub s3_endpoint: Option<String>,
    pub s3_bucket: String,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_region: String,
    pub s3_public_url: Option<String>,
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
            llm_settings_encryption_key: env::var("LLM_SETTINGS_ENCRYPTION_KEY")
                .expect("LLM_SETTINGS_ENCRYPTION_KEY required"),
            session_secret: env::var("SESSION_SECRET").expect("SESSION_SECRET required"),
            google_client_id: env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID required"),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET")
                .expect("GOOGLE_CLIENT_SECRET required"),
            enable_e2e_auth: env::var("ENABLE_E2E_AUTH")
                .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(false),
            storage_backend: match env::var("STORAGE_BACKEND")
                .unwrap_or_else(|_| "local".into())
                .as_str()
            {
                "s3" => StorageBackend::S3,
                _ => StorageBackend::Local,
            },
            s3_endpoint: env::var("S3_ENDPOINT").ok(),
            s3_bucket: env::var("S3_BUCKET").unwrap_or_else(|_| "boopmark".into()),
            s3_access_key: env::var("S3_ACCESS_KEY").ok(),
            s3_secret_key: env::var("S3_SECRET_KEY").ok(),
            s3_region: env::var("S3_REGION").unwrap_or_else(|_| "auto".into()),
            s3_public_url: env::var("S3_PUBLIC_URL").ok(),
        }
    }
}
