mod adapters;
mod app;
mod config;
mod domain;
mod web;

use adapters::anthropic::AnthropicEnricher;
use adapters::postgres::PostgresPool;
use adapters::scraper::HtmlMetadataExtractor;
use adapters::storage::local::LocalStorage;
use adapters::storage::s3::S3Storage;
use app::auth::AuthService;
use app::bookmarks::BookmarkService;
use app::secrets::SecretBox;
use app::settings::SettingsService;
use config::{Config, StorageBackend};
use domain::ports::llm_enricher::LlmEnricher;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use web::state::{AppState, Bookmarks, ImageStorage};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env();

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let db = Arc::new(PostgresPool::new(pool));

    let metadata = Arc::new(HtmlMetadataExtractor::new());

    let bookmarks = match config.storage_backend {
        StorageBackend::Local => {
            let storage = Arc::new(LocalStorage::new(
                "./uploads".into(),
                format!("{}/uploads", config.app_url),
            ));
            Bookmarks::Local(Arc::new(BookmarkService::new(
                db.clone(),
                metadata,
                storage,
            )))
        }
        StorageBackend::S3 => {
            let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await;
            let s3_client = aws_sdk_s3::Client::new(&s3_config);
            let storage = Arc::new(S3Storage::new(
                s3_client,
                config.s3_bucket.clone(),
                config
                    .s3_public_url
                    .clone()
                    .unwrap_or_else(|| format!("https://{}.s3.amazonaws.com", config.s3_bucket)),
            ));
            Bookmarks::S3(Arc::new(BookmarkService::new(
                db.clone(),
                metadata,
                storage,
            )))
        }
    };

    let images_storage = match config.storage_backend {
        StorageBackend::Local => ImageStorage::Local(LocalStorage::new(
            "./uploads/images".into(),
            format!("{}/uploads/images", config.app_url),
        )),
        StorageBackend::S3 => {
            let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await;
            let s3_client = aws_sdk_s3::Client::new(&s3_config);
            ImageStorage::S3(S3Storage::new(
                s3_client,
                config.s3_images_bucket.clone(),
                config
                    .s3_public_url
                    .clone()
                    .map(|u| u.replace(&config.s3_bucket, &config.s3_images_bucket))
                    .unwrap_or_else(|| {
                        format!("https://{}.s3.amazonaws.com", config.s3_images_bucket)
                    }),
            ))
        }
    };

    let auth_service = Arc::new(AuthService::new(db.clone(), db.clone(), db.clone()));
    let secret_box = Arc::new(SecretBox::new(&config.llm_settings_encryption_key));
    let settings_service = Arc::new(SettingsService::new(db.clone(), secret_box));
    let enricher: Arc<dyn LlmEnricher> = Arc::new(AnthropicEnricher::new());

    let state = AppState {
        bookmarks,
        auth: auth_service,
        settings: settings_service,
        config: Arc::new(config.clone()),
        enricher,
        images_storage,
    };

    let app = web::router::create_router(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port))
        .await
        .unwrap();

    tracing::info!("listening on {}", config.port);
    axum::serve(listener, app).await.unwrap();
}
