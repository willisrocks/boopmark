mod adapters;
mod app;
mod config;
mod domain;
mod web;

use adapters::anthropic::AnthropicEnricher;
use adapters::login::google::GoogleLoginProvider;
use adapters::login::local_password::LocalPasswordLoginProvider;
use adapters::metadata::fallback::FallbackMetadataExtractor;
use adapters::metadata::html::HtmlMetadataExtractor;
use adapters::metadata::iframely::IframelyExtractor;
use adapters::metadata::opengraph_io::OpengraphIoExtractor;
use adapters::postgres::PostgresPool;
use adapters::screenshot::noop::NoopScreenshot;
use adapters::screenshot::playwright::PlaywrightScreenshot;
use adapters::storage::local::LocalStorage;
use adapters::storage::s3::S3Storage;
use app::auth::AuthService;
use app::bookmarks::BookmarkService;
use app::enrichment::EnrichmentService;
use app::invite::InviteService;
use app::secrets::SecretBox;
use app::settings::SettingsService;
use config::{Config, LoginAdapter, MetadataFallbackBackend, ScreenshotBackend, StorageBackend};
use domain::ports::llm_enricher::LlmEnricher;
use domain::ports::login_provider::LoginProvider;
use domain::ports::screenshot::ScreenshotProvider;
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

    let html_extractor = HtmlMetadataExtractor::new();
    let mut extractors: Vec<Box<dyn domain::ports::metadata::MetadataExtractor>> =
        vec![Box::new(html_extractor)];

    match &config.metadata_fallback_backend {
        MetadataFallbackBackend::Iframely => {
            let api_key = config
                .iframely_api_key
                .clone()
                .expect("IFRAMELY_API_KEY required when METADATA_FALLBACK_BACKEND=iframely");
            tracing::info!("metadata fallback: iframely");
            extractors.push(Box::new(IframelyExtractor::new(api_key)));
        }
        MetadataFallbackBackend::OpengraphIo => {
            let api_key = config.opengraph_io_api_key.clone().expect(
                "OPENGRAPH_IO_API_KEY required when METADATA_FALLBACK_BACKEND=opengraph_io",
            );
            tracing::info!("metadata fallback: opengraph.io");
            extractors.push(Box::new(OpengraphIoExtractor::new(api_key)));
        }
        MetadataFallbackBackend::None => {}
    }

    let metadata = Arc::new(FallbackMetadataExtractor::new(extractors));
    let metadata_for_enrichment = metadata.clone();

    let screenshot: Arc<dyn ScreenshotProvider> = match config.screenshot_backend {
        ScreenshotBackend::Playwright => {
            let url = config
                .screenshot_service_url
                .clone()
                .expect("SCREENSHOT_SERVICE_URL required when SCREENSHOT_BACKEND=playwright");
            Arc::new(PlaywrightScreenshot::new(url))
        }
        ScreenshotBackend::Disabled => Arc::new(NoopScreenshot),
    };

    let (bookmarks, images_storage) = match config.storage_backend {
        StorageBackend::Local => {
            let storage = Arc::new(LocalStorage::new(
                "./uploads".into(),
                format!("{}/uploads", config.app_url),
            ));
            let images = ImageStorage::Local(LocalStorage::new(
                "./uploads/images".into(),
                format!("{}/uploads/images", config.app_url),
            ));
            (
                Bookmarks::Local(Arc::new(BookmarkService::new(
                    db.clone(),
                    metadata,
                    storage,
                    screenshot.clone(),
                ))),
                images,
            )
        }
        StorageBackend::S3 => {
            let mut s3_config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(endpoint) = &config.s3_endpoint {
                s3_config_loader = s3_config_loader.endpoint_url(endpoint);
            }
            if let (Some(access_key), Some(secret_key)) =
                (&config.s3_access_key, &config.s3_secret_key)
            {
                s3_config_loader = s3_config_loader.credentials_provider(
                    aws_sdk_s3::config::Credentials::new(access_key, secret_key, None, None, "env"),
                );
            }
            s3_config_loader =
                s3_config_loader.region(aws_sdk_s3::config::Region::new(config.s3_region.clone()));
            let s3_config = s3_config_loader.load().await;
            let s3_client = aws_sdk_s3::Client::new(&s3_config);
            let images_public_url = config
                .s3_images_public_url
                .clone()
                .unwrap_or_else(|| format!("https://{}.s3.amazonaws.com", config.s3_images_bucket));
            let storage = Arc::new(S3Storage::new(
                s3_client.clone(),
                config.s3_images_bucket.clone(),
                images_public_url.clone(),
            ));
            let images = ImageStorage::S3(S3Storage::new(
                s3_client,
                config.s3_images_bucket.clone(),
                images_public_url,
            ));
            (
                Bookmarks::S3(Arc::new(BookmarkService::new(
                    db.clone(),
                    metadata,
                    storage,
                    screenshot.clone(),
                ))),
                images,
            )
        }
    };

    let auth_service = Arc::new(AuthService::new(db.clone(), db.clone(), db.clone()));
    let secret_box = Arc::new(SecretBox::new(&config.llm_settings_encryption_key));
    let settings_service = Arc::new(SettingsService::new(db.clone(), secret_box));
    let enricher: Arc<dyn LlmEnricher> = Arc::new(AnthropicEnricher::new());
    let enrichment_service = Arc::new(EnrichmentService::new(
        metadata_for_enrichment,
        enricher,
        settings_service.clone(),
    ));
    let tag_consolidator: Arc<dyn domain::ports::tag_consolidator::TagConsolidator> =
        Arc::new(adapters::anthropic_tag_consolidator::AnthropicTagConsolidator::new());
    let tag_consolidation_service = Arc::new(app::tag_consolidation::TagConsolidationService::new(
        db.clone(),
        tag_consolidator,
        settings_service.clone(),
    ));
    let invite_service = Arc::new(InviteService::new(db.clone()));

    let login_provider: Arc<dyn LoginProvider> = match config.login_adapter {
        LoginAdapter::Google => {
            let client_id = config
                .google_client_id
                .clone()
                .expect("GOOGLE_CLIENT_ID required when LOGIN_ADAPTER=google");
            let client_secret = config
                .google_client_secret
                .clone()
                .expect("GOOGLE_CLIENT_SECRET required when LOGIN_ADAPTER=google");
            Arc::new(GoogleLoginProvider {
                client_id,
                client_secret,
            })
        }
        LoginAdapter::LocalPassword => Arc::new(LocalPasswordLoginProvider),
    };

    let state = AppState {
        bookmarks,
        auth: auth_service,
        settings: settings_service,
        config: Arc::new(config.clone()),
        enrichment: enrichment_service,
        tag_consolidation: tag_consolidation_service,
        images_storage,
        active_image_fix_jobs: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        active_tag_consolidation_jobs: Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::new(),
        )),
        login_provider,
        invites: invite_service,
    };

    let app = web::router::create_router(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port))
        .await
        .unwrap();

    tracing::info!("listening on {}", config.port);
    axum::serve(listener, app).await.unwrap();
}
