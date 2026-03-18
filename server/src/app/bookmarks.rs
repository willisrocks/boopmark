use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use crate::domain::ports::metadata::MetadataExtractor;
use crate::domain::ports::storage::ObjectStorage;
use std::sync::Arc;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(serde::Serialize, Clone, Debug)]
pub struct ProgressEvent {
    pub checked: usize,
    pub total: usize,
    pub fixed: usize,
    pub failed: usize,
    pub done: bool,
}

pub struct BookmarkService<R, M, S> {
    repo: Arc<R>,
    metadata: Arc<M>,
    storage: Arc<S>,
    http_client: reqwest::Client,
}

impl<R, M, S> BookmarkService<R, M, S>
where
    R: BookmarkRepository + Send + Sync,
    M: MetadataExtractor + Send + Sync,
    S: ObjectStorage + Send + Sync,
{
    pub fn new(repo: Arc<R>, metadata: Arc<M>, storage: Arc<S>) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            repo,
            metadata,
            storage,
            http_client,
        }
    }

    pub async fn create(
        &self,
        user_id: Uuid,
        mut input: CreateBookmark,
    ) -> Result<Bookmark, DomainError> {
        if needs_metadata(&input)
            && let Ok(meta) = self.metadata.extract(&input.url).await
                && let Some(image_url) = merge_metadata(&mut input, meta)
                    && let Ok(stored_url) = self.download_and_store_image(&image_url).await {
                        input.image_url = Some(stored_url);
                    }

        // Extract domain from URL if not set
        if input.domain.is_none()
            && let Ok(parsed) = url::Url::parse(&input.url) {
                input.domain = parsed.host_str().map(|h| h.to_string());
            }

        self.repo.create(user_id, input).await
    }

    pub async fn list(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<Bookmark>, DomainError> {
        self.repo.list(user_id, filter).await
    }

    pub async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
        self.repo.get(id, user_id).await
    }

    pub async fn update(
        &self,
        id: Uuid,
        user_id: Uuid,
        input: UpdateBookmark,
    ) -> Result<Bookmark, DomainError> {
        self.repo.update(id, user_id, input).await
    }

    pub async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        self.repo.delete(id, user_id).await
    }

    pub async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError> {
        self.repo.all_tags(user_id).await
    }

    pub async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        self.repo.tags_with_counts(user_id).await
    }

    pub async fn extract_metadata(&self, url: &str) -> Result<UrlMetadata, DomainError> {
        self.metadata.extract(url).await
    }

    pub async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
        self.repo.export_all(user_id).await
    }

    pub async fn import_batch(
        &self,
        user_id: Uuid,
        records: Vec<crate::domain::transfer::ImportRecord>,
        strategy: crate::domain::transfer::ImportStrategy,
        mode: crate::domain::transfer::ImportMode,
    ) -> Result<crate::domain::transfer::ImportResult, DomainError> {
        use crate::domain::transfer::{ImportError, ImportMode, ImportResult, ImportStrategy};

        let mut result = ImportResult {
            created: 0,
            updated: 0,
            skipped: 0,
            errors: vec![],
        };

        for (idx, record) in records.into_iter().enumerate() {
            let row = idx + 1; // 1-based row numbers in all error messages
            if url::Url::parse(&record.url).is_err() {
                result.errors.push(ImportError {
                    row,
                    message: format!("invalid URL: {}", record.url),
                });
                continue;
            }

            match mode {
                ImportMode::Import => {
                    match self.repo.find_by_url(user_id, &record.url).await? {
                        Some(existing) => match strategy {
                            ImportStrategy::Skip => result.skipped += 1,
                            ImportStrategy::Upsert => {
                                self.repo
                                    .update(
                                        existing.id,
                                        user_id,
                                        UpdateBookmark {
                                            title: record.title,
                                            description: record.description,
                                            tags: Some(record.tags),
                                        },
                                    )
                                    .await?;
                                result.updated += 1;
                            }
                        },
                        None => {
                            self.repo
                                .create(
                                    user_id,
                                    CreateBookmark {
                                        url: record.url,
                                        title: record.title,
                                        description: record.description,
                                        image_url: None,
                                        domain: None,
                                        tags: Some(record.tags),
                                    },
                                )
                                .await?;
                            result.created += 1;
                        }
                    }
                }
                ImportMode::Restore => {
                    let Some(id) = record.id else {
                        result.errors.push(ImportError {
                            row,
                            message: "restore mode requires id field".to_string(),
                        });
                        continue;
                    };

                    let now = chrono::Utc::now();
                    // Derive both timestamps from whichever is present to
                    // avoid impossible ordering (created_at > updated_at):
                    //   - both present      → use as-is
                    //   - only created_at   → updated_at = created_at
                    //   - only updated_at   → created_at = updated_at
                    //   - neither present   → both = now
                    let (created_at, updated_at) = match (record.created_at, record.updated_at) {
                        (Some(c), Some(u)) => (c, u),
                        (Some(c), None) => (c, c),
                        (None, Some(u)) => (u, u),
                        (None, None) => (now, now),
                    };
                    let bookmark = Bookmark {
                        id,
                        user_id,
                        url: record.url,
                        title: record.title,
                        description: record.description,
                        image_url: record.image_url,
                        domain: record.domain,
                        tags: record.tags,
                        created_at,
                        updated_at,
                    };

                    match self.repo.get(id, user_id).await {
                        Ok(_) => match strategy {
                            ImportStrategy::Skip => result.skipped += 1,
                            ImportStrategy::Upsert => {
                                match self.repo.upsert_full(bookmark).await {
                                    Ok(_) => result.updated += 1,
                                    // ID belongs to another user — row-level error
                                    Err(DomainError::AlreadyExists) => {
                                        result.errors.push(ImportError {
                                            row,
                                            message: format!(
                                                "id {id} already exists (owned by another user)"
                                            ),
                                        });
                                    }
                                    Err(e) => return Err(e),
                                }
                            }
                        },
                        Err(DomainError::NotFound) => {
                            match self.repo.insert_with_id(bookmark).await {
                                Ok(_) => result.created += 1,
                                // PK belongs to another user — treat as row-level error
                                Err(DomainError::AlreadyExists) => {
                                    result.errors.push(ImportError {
                                        row,
                                        message: format!(
                                            "id {id} already exists (owned by another user)"
                                        ),
                                    });
                                }
                                Err(e) => return Err(e),
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }

        Ok(result)
    }

    async fn download_and_store_image(&self, image_url: &str) -> Result<String, DomainError> {
        let resp = self
            .http_client
            .get(image_url)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("image fetch error: {e}")))?;

        if !resp.status().is_success() {
            return Err(DomainError::Internal(format!(
                "image fetch returned HTTP {}",
                resp.status()
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| DomainError::Internal(format!("image read error: {e}")))?;

        let key = format!(
            "images/{}.{}",
            Uuid::new_v4(),
            extension_from_content_type(&content_type)
        );
        self.storage.put(&key, bytes.to_vec(), &content_type).await
    }
}

#[allow(dead_code)]
impl<R, M, S> BookmarkService<R, M, S>
where
    R: BookmarkRepository + Send + Sync,
    M: MetadataExtractor + Send + Sync,
    S: ObjectStorage + Send + Sync,
{
    pub async fn fix_missing_images(
        &self,
        user_id: Uuid,
        screenshot_service_url: Option<&str>,
        tx: tokio::sync::mpsc::Sender<ProgressEvent>,
    ) {
        let bookmarks = match self.repo.export_all(user_id).await {
            Ok(b) => b,
            Err(_) => return,
        };

        let total = bookmarks.len();
        let mut checked = 0;
        let mut fixed = 0;
        let mut failed = 0;

        for bookmark in bookmarks {
            let needs_fix = match &bookmark.image_url {
                None => true,
                Some(url) => self
                    .http_client
                    .head(url)
                    .send()
                    .await
                    .map(|r| !r.status().is_success())
                    .unwrap_or(true),
            };

            if needs_fix {
                match self
                    .fetch_and_store_image(&bookmark.url, screenshot_service_url)
                    .await
                {
                    Ok(new_url) => {
                        if self
                            .repo
                            .update_image_url(bookmark.id, user_id, &new_url)
                            .await
                            .is_ok()
                        {
                            fixed += 1;
                        } else {
                            failed += 1;
                        }
                    }
                    Err(_) => failed += 1,
                }
            }

            checked += 1;
            let _ = tx
                .send(ProgressEvent { checked, total, fixed, failed, done: false })
                .await;
        }

        let _ = tx
            .send(ProgressEvent { checked, total, fixed, failed, done: true })
            .await;
    }

    /// Try og:image scrape first; fall back to screenshot sidecar.
    async fn fetch_and_store_image(
        &self,
        page_url: &str,
        screenshot_service_url: Option<&str>,
    ) -> Result<String, DomainError> {
        // 1. Try og:image
        if let Ok(meta) = self.metadata.extract(page_url).await
            && let Some(image_url) = meta.image_url
                && let Ok(stored) = self.download_and_store_image(&image_url).await {
                    return Ok(stored);
                }

        // 2. Fall back to screenshot sidecar via ScreenshotClient adapter
        let svc_url = screenshot_service_url.ok_or_else(|| {
            DomainError::Internal("no screenshot svc".into())
        })?;

        let screenshot_client =
            crate::adapters::screenshot::ScreenshotClient::new(svc_url.to_string());
        let bytes = screenshot_client.capture(page_url).await?;

        let key = format!("images/{}.jpg", Uuid::new_v4());
        self.storage.put(&key, bytes, "image/jpeg").await
    }
}

fn extension_from_content_type(ct: &str) -> &str {
    let mime = ct.split(';').next().unwrap_or(ct).trim();
    match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "jpg",
    }
}

fn needs_metadata(input: &CreateBookmark) -> bool {
    input.title.is_none()
        || input.description.is_none()
        || input.domain.is_none()
        || input.image_url.is_none()
}

fn merge_metadata(input: &mut CreateBookmark, meta: UrlMetadata) -> Option<String> {
    if input.title.is_none() {
        input.title = meta.title;
    }
    if input.description.is_none() {
        input.description = meta.description;
    }
    if input.domain.is_none() {
        input.domain = meta.domain;
    }
    if input.image_url.is_none() {
        return meta.image_url;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_metadata_when_image_or_domain_is_missing() {
        let input = CreateBookmark {
            url: "https://github.com/danshapiro/trycycle".to_string(),
            title: Some("trycycle".to_string()),
            description: Some("already filled".to_string()),
            image_url: None,
            domain: None,
            tags: None,
        };

        assert!(needs_metadata(&input));
    }

    #[test]
    fn merge_metadata_preserves_user_text_but_returns_missing_image() {
        let mut input = CreateBookmark {
            url: "https://github.com/danshapiro/trycycle".to_string(),
            title: Some("Custom Title".to_string()),
            description: None,
            image_url: None,
            domain: None,
            tags: None,
        };

        let image = merge_metadata(
            &mut input,
            UrlMetadata {
                title: Some("Suggested Title".to_string()),
                description: Some("Suggested description".to_string()),
                image_url: Some("https://example.com/preview.png".to_string()),
                domain: Some("github.com".to_string()),
            },
        );

        assert_eq!(input.title.as_deref(), Some("Custom Title"));
        assert_eq!(input.description.as_deref(), Some("Suggested description"));
        assert_eq!(input.domain.as_deref(), Some("github.com"));
        assert_eq!(image.as_deref(), Some("https://example.com/preview.png"));
    }

    mod import_tests {
        use crate::app::bookmarks::BookmarkService;
        use crate::domain::bookmark::*;
        use crate::domain::error::DomainError;
        use crate::domain::ports::bookmark_repo::BookmarkRepository;
        use crate::domain::ports::metadata::MetadataExtractor;
        use crate::domain::ports::storage::ObjectStorage;
        use crate::domain::transfer::*;
        use chrono::Utc;
        use std::sync::{Arc, Mutex};
        use uuid::Uuid;

        struct MockRepo {
            bookmarks: Mutex<Vec<Bookmark>>,
            /// When true, `upsert_full` always returns `AlreadyExists` to
            /// simulate the race condition where a row changes owner between
            /// `get()` and `upsert_full()`.
            fail_upsert: bool,
        }

        impl MockRepo {
            fn new(bookmarks: Vec<Bookmark>) -> Self {
                Self { bookmarks: Mutex::new(bookmarks), fail_upsert: false }
            }

            fn new_with_failing_upsert(bookmarks: Vec<Bookmark>) -> Self {
                Self { bookmarks: Mutex::new(bookmarks), fail_upsert: true }
            }
        }

        impl BookmarkRepository for MockRepo {
            async fn create(
                &self,
                user_id: Uuid,
                input: CreateBookmark,
            ) -> Result<Bookmark, DomainError> {
                let b = Bookmark {
                    id: Uuid::new_v4(),
                    user_id,
                    url: input.url,
                    title: input.title,
                    description: input.description,
                    image_url: input.image_url,
                    domain: input.domain,
                    tags: input.tags.unwrap_or_default(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                };
                self.bookmarks.lock().unwrap().push(b.clone());
                Ok(b)
            }
            async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
                self.bookmarks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|b| b.id == id && b.user_id == user_id)
                    .cloned()
                    .ok_or(DomainError::NotFound)
            }
            async fn list(
                &self,
                user_id: Uuid,
                _filter: BookmarkFilter,
            ) -> Result<Vec<Bookmark>, DomainError> {
                Ok(self
                    .bookmarks
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|b| b.user_id == user_id)
                    .cloned()
                    .collect())
            }
            async fn update(
                &self,
                id: Uuid,
                user_id: Uuid,
                input: UpdateBookmark,
            ) -> Result<Bookmark, DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                let b = bookmarks
                    .iter_mut()
                    .find(|b| b.id == id && b.user_id == user_id)
                    .ok_or(DomainError::NotFound)?;
                if let Some(t) = input.title {
                    b.title = Some(t);
                }
                if let Some(d) = input.description {
                    b.description = Some(d);
                }
                if let Some(tags) = input.tags {
                    b.tags = tags;
                }
                Ok(b.clone())
            }
            async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                let len_before = bookmarks.len();
                bookmarks.retain(|b| !(b.id == id && b.user_id == user_id));
                if bookmarks.len() == len_before {
                    Err(DomainError::NotFound)
                } else {
                    Ok(())
                }
            }
            async fn all_tags(&self, _user_id: Uuid) -> Result<Vec<String>, DomainError> {
                Ok(vec![])
            }
            async fn tags_with_counts(
                &self,
                _user_id: Uuid,
            ) -> Result<Vec<(String, i64)>, DomainError> {
                Ok(vec![])
            }
            async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
                Ok(self
                    .bookmarks
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|b| b.user_id == user_id)
                    .cloned()
                    .collect())
            }
            async fn find_by_url(
                &self,
                user_id: Uuid,
                url: &str,
            ) -> Result<Option<Bookmark>, DomainError> {
                Ok(self
                    .bookmarks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|b| b.user_id == user_id && b.url == url)
                    .cloned())
            }
            async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                // Simulate Postgres unique-constraint violation when the PK
                // already exists (cross-tenant or same-tenant collision).
                if bookmarks.iter().any(|b| b.id == bookmark.id) {
                    return Err(DomainError::AlreadyExists);
                }
                bookmarks.push(bookmark.clone());
                Ok(bookmark)
            }
            async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
                // Simulate the race-condition path: row changed owner between
                // get() and upsert_full().
                if self.fail_upsert {
                    return Err(DomainError::AlreadyExists);
                }
                let mut bookmarks = self.bookmarks.lock().unwrap();
                // Simulate Postgres cross-tenant guard: only update if the
                // existing row belongs to the same user.
                if let Some(existing) = bookmarks.iter().find(|b| b.id == bookmark.id) {
                    if existing.user_id != bookmark.user_id {
                        return Err(DomainError::AlreadyExists);
                    }
                }
                if let Some(b) = bookmarks.iter_mut().find(|b| b.id == bookmark.id) {
                    *b = bookmark.clone();
                    Ok(bookmark)
                } else {
                    bookmarks.push(bookmark.clone());
                    Ok(bookmark)
                }
            }
            async fn update_image_url(
                &self,
                id: Uuid,
                user_id: Uuid,
                image_url: &str,
            ) -> Result<(), DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                if let Some(b) = bookmarks.iter_mut().find(|b| b.id == id && b.user_id == user_id) {
                    b.image_url = Some(image_url.to_string());
                    Ok(())
                } else {
                    Err(DomainError::NotFound)
                }
            }
        }

        struct NoopMetadata;
        impl MetadataExtractor for NoopMetadata {
            async fn extract(&self, _url: &str) -> Result<UrlMetadata, DomainError> {
                Ok(UrlMetadata {
                    title: None,
                    description: None,
                    image_url: None,
                    domain: None,
                })
            }
        }

        struct NoopStorage;
        impl ObjectStorage for NoopStorage {
            async fn put(
                &self,
                _key: &str,
                _data: Vec<u8>,
                _content_type: &str,
            ) -> Result<String, DomainError> {
                Ok(String::new())
            }
            async fn get(&self, _key: &str) -> Result<Vec<u8>, DomainError> {
                Ok(vec![])
            }
            async fn delete(&self, _key: &str) -> Result<(), DomainError> {
                Ok(())
            }
            fn public_url(&self, key: &str) -> String {
                key.to_string()
            }
        }

        fn make_service(
            bookmarks: Vec<Bookmark>,
        ) -> BookmarkService<MockRepo, NoopMetadata, NoopStorage> {
            BookmarkService::new(
                Arc::new(MockRepo::new(bookmarks)),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            )
        }

        fn make_service_with_failing_upsert(
            bookmarks: Vec<Bookmark>,
        ) -> BookmarkService<MockRepo, NoopMetadata, NoopStorage> {
            BookmarkService::new(
                Arc::new(MockRepo::new_with_failing_upsert(bookmarks)),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            )
        }

        fn make_bookmark(user_id: Uuid, url: &str) -> Bookmark {
            Bookmark {
                id: Uuid::new_v4(),
                user_id,
                url: url.to_string(),
                title: Some("Test".to_string()),
                description: None,
                image_url: None,
                domain: None,
                tags: vec![],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }
        }

        fn make_record(url: &str) -> ImportRecord {
            ImportRecord {
                url: url.to_string(),
                title: Some("Imported".to_string()),
                description: None,
                tags: vec![],
                id: None,
                image_url: None,
                domain: None,
                created_at: None,
                updated_at: None,
            }
        }

        fn make_restore_record(url: &str, id: Uuid) -> ImportRecord {
            ImportRecord {
                url: url.to_string(),
                title: Some("Imported".to_string()),
                description: None,
                tags: vec![],
                id: Some(id),
                image_url: None,
                domain: None,
                created_at: Some(Utc::now()),
                updated_at: Some(Utc::now()),
            }
        }

        #[tokio::test]
        async fn import_creates_new_bookmark() {
            let user_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("https://example.com")],
                    ImportStrategy::Upsert,
                    ImportMode::Import,
                )
                .await
                .unwrap();
            assert_eq!(result.created, 1);
            assert_eq!(result.updated, 0);
            assert_eq!(result.skipped, 0);
        }

        #[tokio::test]
        async fn import_skips_existing_url_when_strategy_is_skip() {
            let user_id = Uuid::new_v4();
            let existing = make_bookmark(user_id, "https://example.com");
            let svc = make_service(vec![existing]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("https://example.com")],
                    ImportStrategy::Skip,
                    ImportMode::Import,
                )
                .await
                .unwrap();
            assert_eq!(result.skipped, 1);
            assert_eq!(result.created, 0);
            assert_eq!(result.updated, 0);
        }

        #[tokio::test]
        async fn import_upserts_existing_url_when_strategy_is_upsert() {
            let user_id = Uuid::new_v4();
            let existing = make_bookmark(user_id, "https://example.com");
            let svc = make_service(vec![existing]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("https://example.com")],
                    ImportStrategy::Upsert,
                    ImportMode::Import,
                )
                .await
                .unwrap();
            assert_eq!(result.updated, 1);
            assert_eq!(result.created, 0);
            assert_eq!(result.skipped, 0);
        }

        #[tokio::test]
        async fn import_records_error_for_invalid_url() {
            let user_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("not-a-url")],
                    ImportStrategy::Upsert,
                    ImportMode::Import,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 1);
            assert_eq!(result.created, 0);
        }

        #[tokio::test]
        async fn restore_creates_new_bookmark_with_original_id() {
            let user_id = Uuid::new_v4();
            let original_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let record = make_restore_record("https://example.com", original_id);
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.created, 1);
        }

        #[tokio::test]
        async fn restore_records_error_when_id_is_missing() {
            let user_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("https://example.com")],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 1);
            assert_eq!(result.created, 0);
        }

        #[tokio::test]
        async fn restore_skips_existing_id_when_strategy_is_skip() {
            let user_id = Uuid::new_v4();
            let existing = make_bookmark(user_id, "https://example.com");
            let existing_id = existing.id;
            let svc = make_service(vec![existing]);
            let record = make_restore_record("https://example.com", existing_id);
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Skip,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.skipped, 1);
            assert_eq!(result.created, 0);
            assert_eq!(result.updated, 0);
        }

        #[tokio::test]
        async fn restore_upserts_existing_id() {
            let user_id = Uuid::new_v4();
            let existing = make_bookmark(user_id, "https://example.com");
            let existing_id = existing.id;
            let svc = make_service(vec![existing]);
            let record = make_restore_record("https://updated.com", existing_id);
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.updated, 1);
            assert_eq!(result.created, 0);
        }

        #[tokio::test]
        async fn export_all_returns_user_bookmarks() {
            let user_id = Uuid::new_v4();
            let other_user = Uuid::new_v4();
            let svc = make_service(vec![
                make_bookmark(user_id, "https://mine.com"),
                make_bookmark(other_user, "https://theirs.com"),
            ]);
            let bookmarks = svc.export_all(user_id).await.unwrap();
            assert_eq!(bookmarks.len(), 1);
            assert_eq!(bookmarks[0].url, "https://mine.com");
        }

        #[tokio::test]
        async fn import_multiple_records_mixed_results() {
            let user_id = Uuid::new_v4();
            let existing = make_bookmark(user_id, "https://existing.com");
            let svc = make_service(vec![existing]);
            let records = vec![
                make_record("https://new.com"),
                make_record("https://existing.com"),
                make_record("bad-url"),
            ];
            let result = svc
                .import_batch(user_id, records, ImportStrategy::Skip, ImportMode::Import)
                .await
                .unwrap();
            assert_eq!(result.created, 1);
            assert_eq!(result.skipped, 1);
            assert_eq!(result.errors.len(), 1);
        }

        #[tokio::test]
        async fn restore_succeeds_when_timestamps_are_missing() {
            // Per plan: missing timestamps in restore mode use unwrap_or(now),
            // they are NOT rejected as errors.
            let user_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let mut record = make_restore_record("https://example.com", Uuid::new_v4());
            record.created_at = None;
            record.updated_at = None;
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 0);
            assert_eq!(result.created, 1);
        }

        #[tokio::test]
        async fn restore_cross_account_pk_collision_via_insert_with_id_is_a_row_error() {
            // User B tries to restore a bookmark using an ID owned by user A.
            // repo.get(id, user_b) returns NotFound (different user), so the
            // service calls insert_with_id which returns AlreadyExists (PK taken).
            // That must surface as a row-level error, not a propagated Err.
            let user_a = Uuid::new_v4();
            let user_b = Uuid::new_v4();
            let shared_id = Uuid::new_v4();

            let existing = Bookmark {
                id: shared_id,
                user_id: user_a,
                url: "https://user-a.example.com".to_string(),
                title: None,
                description: None,
                image_url: None,
                domain: None,
                tags: vec![],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            let svc = make_service(vec![existing]);

            let record = make_restore_record("https://user-b.example.com", shared_id);
            let result = svc
                .import_batch(
                    user_b,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 1, "cross-account collision must be a row error");
            assert!(result.errors[0].message.contains("already exists"));
            assert_eq!(result.created, 0);
        }

        #[tokio::test]
        async fn mock_upsert_full_rejects_cross_tenant_id_directly() {
            // Direct unit test of MockRepo::upsert_full's cross-tenant guard.
            // The service-level path that reaches upsert_full with a
            // cross-tenant collision is a race condition (ID changes owner
            // between get() and upsert_full()) not easily reproduced in an
            // in-memory mock; this test verifies the guard at the repo level.
            let user_a = Uuid::new_v4();
            let user_b = Uuid::new_v4();
            let shared_id = Uuid::new_v4();

            let existing = Bookmark {
                id: shared_id,
                user_id: user_a,
                url: "https://user-a.example.com".to_string(),
                title: None,
                description: None,
                image_url: None,
                domain: None,
                tags: vec![],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            let repo = Arc::new(MockRepo::new(vec![existing]));

            // user_b tries to upsert a row whose ID belongs to user_a.
            let intruder = Bookmark {
                id: shared_id,
                user_id: user_b,
                url: "https://user-b.example.com".to_string(),
                title: None,
                description: None,
                image_url: None,
                domain: None,
                tags: vec![],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            let err = repo.upsert_full(intruder).await.unwrap_err();
            assert!(
                matches!(err, DomainError::AlreadyExists),
                "upsert_full must return AlreadyExists for cross-tenant ID"
            );
        }

        #[tokio::test]
        async fn restore_cross_account_pk_collision_via_upsert_full_is_a_row_error() {
            // Service-level test: the ID already belongs to this user (get()
            // succeeds), but upsert_full then returns AlreadyExists — simulating
            // the race condition where the row was transferred between get() and
            // upsert_full(). import_batch must record this as a row-level error.
            let user_id = Uuid::new_v4();
            let original_id = Uuid::new_v4();

            // Pre-load the row under the same user so get() returns Ok.
            let existing = make_bookmark(user_id, "https://original.example.com");
            let existing_id = existing.id;
            // Use fail_upsert=true so upsert_full fires AlreadyExists regardless.
            let _ = original_id; // suppress unused warning; using existing_id below
            let svc = make_service_with_failing_upsert(vec![existing]);

            let record = make_restore_record("https://original.example.com", existing_id);
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 1, "cross-tenant upsert_full collision must be a row error");
            assert!(result.errors[0].message.contains("already exists"));
            assert_eq!(result.updated, 0);
        }

        #[tokio::test]
        async fn restore_missing_updated_at_uses_created_at_to_preserve_ordering() {
            // When updated_at is missing but created_at is present, the stored
            // bookmark must have updated_at == created_at (not "now"), so that
            // created_at <= updated_at is maintained.
            let user_id = Uuid::new_v4();
            let repo = Arc::new(MockRepo::new(vec![]));
            let svc = BookmarkService::new(
                Arc::clone(&repo),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let past = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            let id = Uuid::new_v4();
            let mut record = make_restore_record("https://example.com", id);
            record.created_at = Some(past);
            record.updated_at = None;
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 0);
            assert_eq!(result.created, 1);

            // Verify created_at <= updated_at in the persisted bookmark.
            let stored = repo
                .export_all(user_id)
                .await
                .unwrap()
                .into_iter()
                .find(|b| b.id == id)
                .expect("bookmark should have been persisted");
            assert_eq!(stored.created_at, past, "created_at must be preserved");
            assert_eq!(
                stored.updated_at, past,
                "updated_at must fall back to created_at when absent"
            );
            assert!(
                stored.created_at <= stored.updated_at,
                "created_at must not exceed updated_at"
            );
        }

        #[tokio::test]
        async fn restore_missing_created_at_uses_updated_at_to_preserve_ordering() {
            // When created_at is missing but updated_at is present, the stored
            // bookmark must have created_at == updated_at, not "now" (which could
            // produce future-created_at if updated_at is in the past).
            let user_id = Uuid::new_v4();
            let repo = Arc::new(MockRepo::new(vec![]));
            let svc = BookmarkService::new(
                Arc::clone(&repo),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let past = chrono::DateTime::parse_from_rfc3339("2021-06-15T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            let id = Uuid::new_v4();
            let mut record = make_restore_record("https://example.com/created-missing", id);
            record.created_at = None;
            record.updated_at = Some(past);
            let result = svc
                .import_batch(
                    user_id,
                    vec![record],
                    ImportStrategy::Upsert,
                    ImportMode::Restore,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 0);
            assert_eq!(result.created, 1);

            let stored = repo
                .export_all(user_id)
                .await
                .unwrap()
                .into_iter()
                .find(|b| b.id == id)
                .expect("bookmark should have been persisted");
            assert_eq!(stored.updated_at, past, "updated_at must be preserved");
            assert_eq!(
                stored.created_at, past,
                "created_at must fall back to updated_at when absent"
            );
            assert!(stored.created_at <= stored.updated_at);
        }

        #[tokio::test]
        async fn import_error_rows_are_one_based() {
            // The first record (index 0) with a bad URL must report row: 1
            let user_id = Uuid::new_v4();
            let svc = make_service(vec![]);
            let result = svc
                .import_batch(
                    user_id,
                    vec![make_record("not-a-url")],
                    ImportStrategy::Upsert,
                    ImportMode::Import,
                )
                .await
                .unwrap();
            assert_eq!(result.errors.len(), 1);
            assert_eq!(result.errors[0].row, 1);
        }
    }

    mod fix_images_tests {
        use super::super::*;
        use axum::{
            Router,
            routing::{get, head as head_route, post},
        };
        use chrono::Utc;
        use std::sync::{Arc, Mutex};
        use tokio::sync::mpsc;
        use uuid::Uuid;

        // ── helpers ───────────────────────────────────────────────────────────

        struct MockRepo {
            bookmarks: Mutex<Vec<Bookmark>>,
        }

        impl MockRepo {
            fn new(bookmarks: Vec<Bookmark>) -> Self {
                Self { bookmarks: Mutex::new(bookmarks) }
            }
        }

        impl BookmarkRepository for MockRepo {
            async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError> {
                let b = Bookmark {
                    id: Uuid::new_v4(), user_id, url: input.url, title: input.title,
                    description: input.description, image_url: input.image_url,
                    domain: input.domain, tags: input.tags.unwrap_or_default(),
                    created_at: Utc::now(), updated_at: Utc::now(),
                };
                self.bookmarks.lock().unwrap().push(b.clone());
                Ok(b)
            }
            async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
                self.bookmarks.lock().unwrap().iter()
                    .find(|b| b.id == id && b.user_id == user_id).cloned()
                    .ok_or(DomainError::NotFound)
            }
            async fn list(&self, user_id: Uuid, _filter: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError> {
                Ok(self.bookmarks.lock().unwrap().iter().filter(|b| b.user_id == user_id).cloned().collect())
            }
            async fn update(&self, id: Uuid, user_id: Uuid, input: UpdateBookmark) -> Result<Bookmark, DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                let b = bookmarks.iter_mut().find(|b| b.id == id && b.user_id == user_id).ok_or(DomainError::NotFound)?;
                if let Some(t) = input.title { b.title = Some(t); }
                if let Some(d) = input.description { b.description = Some(d); }
                if let Some(tags) = input.tags { b.tags = tags; }
                Ok(b.clone())
            }
            async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
                let mut b = self.bookmarks.lock().unwrap();
                let before = b.len();
                b.retain(|bm| !(bm.id == id && bm.user_id == user_id));
                if b.len() == before { Err(DomainError::NotFound) } else { Ok(()) }
            }
            async fn all_tags(&self, _user_id: Uuid) -> Result<Vec<String>, DomainError> { Ok(vec![]) }
            async fn tags_with_counts(&self, _user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> { Ok(vec![]) }
            async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
                Ok(self.bookmarks.lock().unwrap().iter().filter(|b| b.user_id == user_id).cloned().collect())
            }
            async fn find_by_url(&self, user_id: Uuid, url: &str) -> Result<Option<Bookmark>, DomainError> {
                Ok(self.bookmarks.lock().unwrap().iter().find(|b| b.user_id == user_id && b.url == url).cloned())
            }
            async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
                let mut b = self.bookmarks.lock().unwrap();
                if b.iter().any(|bm| bm.id == bookmark.id) { return Err(DomainError::AlreadyExists); }
                b.push(bookmark.clone()); Ok(bookmark)
            }
            async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
                let mut b = self.bookmarks.lock().unwrap();
                if let Some(existing) = b.iter_mut().find(|bm| bm.id == bookmark.id) {
                    *existing = bookmark.clone(); Ok(bookmark)
                } else {
                    b.push(bookmark.clone()); Ok(bookmark)
                }
            }
            async fn update_image_url(&self, id: Uuid, user_id: Uuid, image_url: &str) -> Result<(), DomainError> {
                let mut b = self.bookmarks.lock().unwrap();
                if let Some(bm) = b.iter_mut().find(|bm| bm.id == id && bm.user_id == user_id) {
                    bm.image_url = Some(image_url.to_string()); Ok(())
                } else { Err(DomainError::NotFound) }
            }
        }

        struct NoopMetadata;
        impl MetadataExtractor for NoopMetadata {
            async fn extract(&self, _url: &str) -> Result<UrlMetadata, DomainError> {
                Ok(UrlMetadata { title: None, description: None, image_url: None, domain: None })
            }
        }

        struct HtmlMetadata {
            image_url: Option<String>,
        }
        impl MetadataExtractor for HtmlMetadata {
            async fn extract(&self, _url: &str) -> Result<UrlMetadata, DomainError> {
                Ok(UrlMetadata { title: None, description: None, image_url: self.image_url.clone(), domain: None })
            }
        }

        struct NoopStorage;
        impl ObjectStorage for NoopStorage {
            async fn put(&self, key: &str, _data: Vec<u8>, _ct: &str) -> Result<String, DomainError> {
                Ok(format!("https://stored/{}", key))
            }
            async fn get(&self, _key: &str) -> Result<Vec<u8>, DomainError> { Ok(vec![]) }
            async fn delete(&self, _key: &str) -> Result<(), DomainError> { Ok(()) }
            fn public_url(&self, key: &str) -> String { format!("https://stored/{}", key) }
        }

        fn make_bookmark(user_id: Uuid, url: &str, image_url: Option<&str>) -> Bookmark {
            Bookmark {
                id: Uuid::new_v4(),
                user_id,
                url: url.to_string(),
                title: Some("Test".to_string()),
                description: None,
                image_url: image_url.map(|s| s.to_string()),
                domain: None,
                tags: vec![],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }
        }

        async fn collect_events(mut rx: mpsc::Receiver<ProgressEvent>) -> Vec<ProgressEvent> {
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                let done = event.done;
                events.push(event);
                if done { break; }
            }
            events
        }

        // Spin up a minimal HTTP server that:
        // - GET / → returns `html`
        // - HEAD /image.jpg → returns `image_status`
        async fn start_fake_site(html: &'static str, image_status: u16) -> std::net::SocketAddr {
            let app = Router::new()
                .route("/", get(move || {
                    let html = html.to_string();
                    async move {
                        (axum::http::StatusCode::OK, [("Content-Type", "text/html")], html)
                    }
                }))
                .route("/image.jpg", head_route(move || async move {
                    axum::http::StatusCode::from_u16(image_status).unwrap()
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            addr
        }

        // Spin up a fake screenshot sidecar that returns a minimal JPEG
        async fn start_fake_screenshot_svc() -> std::net::SocketAddr {
            let app = Router::new().route("/screenshot", post(|| async {
                let jpeg: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xD9];
                (axum::http::StatusCode::OK, [("Content-Type", "image/jpeg")], jpeg)
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            addr
        }

        // ── tests ─────────────────────────────────────────────────────────────

        #[tokio::test]
        async fn empty_bookmark_list_emits_single_done_event() {
            let user_id = Uuid::new_v4();
            let svc = BookmarkService::new(
                Arc::new(MockRepo::new(vec![])),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let (tx, rx) = mpsc::channel(32);
            svc.fix_missing_images(user_id, None, tx).await;
            let events = collect_events(rx).await;
            assert_eq!(events.len(), 1);
            let last = &events[0];
            assert_eq!(last.checked, 0);
            assert_eq!(last.total, 0);
            assert_eq!(last.fixed, 0);
            assert_eq!(last.failed, 0);
            assert!(last.done);
        }

        #[tokio::test]
        async fn skips_bookmarks_with_valid_images() {
            // Arrange: one bookmark whose image HEAD returns 200 (valid)
            let addr = start_fake_site("", 200).await;
            let image_url = format!("http://{}/image.jpg", addr);
            let user_id = Uuid::new_v4();
            let bookmark = make_bookmark(user_id, &format!("http://{}/", addr), Some(&image_url));

            let svc = BookmarkService::new(
                Arc::new(MockRepo::new(vec![bookmark])),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let (tx, rx) = mpsc::channel(32);
            svc.fix_missing_images(user_id, None, tx).await;
            let events = collect_events(rx).await;
            let last = events.last().unwrap();
            assert_eq!(last.fixed, 0, "should not fix an already-working image");
            assert_eq!(last.failed, 0);
            assert_eq!(last.checked, 1);
            assert!(last.done);
        }

        #[tokio::test]
        async fn records_failure_when_no_image_and_no_screenshot_svc() {
            // Arrange: one bookmark with image_url = None, no og:image, no screenshot svc
            let user_id = Uuid::new_v4();
            let bookmark = make_bookmark(user_id, "http://127.0.0.1:1/", None);
            let svc = BookmarkService::new(
                Arc::new(MockRepo::new(vec![bookmark])),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let (tx, rx) = mpsc::channel(32);
            svc.fix_missing_images(user_id, None, tx).await;
            let events = collect_events(rx).await;
            let last = events.last().unwrap();
            assert_eq!(last.fixed, 0);
            assert_eq!(last.failed, 1);
            assert!(last.done);
        }

        #[tokio::test]
        async fn fixes_bookmark_with_broken_image_via_og_image() {
            // Arrange: bookmark with image_url returning 404 (broken);
            // og:image is available on the page via HtmlMetadata
            let addr = start_fake_site("", 404).await;
            let image_url = format!("http://{}/image.jpg", addr);
            let user_id = Uuid::new_v4();
            let bookmark = make_bookmark(user_id, &format!("http://{}/", addr), Some(&image_url));
            let og_image = format!("http://{}/image.jpg", addr);

            // Use a metadata extractor that returns the og:image pointing back at /image.jpg
            // but now the storage will "store" it and return a new URL
            // We need a metadata that returns a downloadable image URL.
            // Since the fake site HEAD returns 404 but we need the og:image GET to succeed,
            // start a second server that serves the image as GET.
            let img_server = Router::new()
                .route("/image.jpg", get(|| async {
                    let jpeg: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xD9];
                    (axum::http::StatusCode::OK, [("Content-Type", "image/jpeg")], jpeg)
                }));
            let img_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let img_addr = img_listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(img_listener, img_server).await.unwrap() });
            let downloadable_og_image = format!("http://{}/image.jpg", img_addr);

            let _ = og_image; // suppress warning

            let svc = BookmarkService::new(
                Arc::new(MockRepo::new(vec![bookmark])),
                Arc::new(HtmlMetadata { image_url: Some(downloadable_og_image) }),
                Arc::new(NoopStorage),
            );
            let (tx, rx) = mpsc::channel(32);
            svc.fix_missing_images(user_id, None, tx).await;
            let events = collect_events(rx).await;
            let last = events.last().unwrap();
            assert_eq!(last.fixed, 1, "broken image should be fixed via og:image");
            assert_eq!(last.failed, 0);
            assert!(last.done);
        }

        #[tokio::test]
        async fn fixes_bookmark_via_screenshot_fallback() {
            // Arrange: no image, no og:image, but screenshot svc available
            let screenshot_addr = start_fake_screenshot_svc().await;
            let screenshot_url = format!("http://{}", screenshot_addr);
            let user_id = Uuid::new_v4();
            let bookmark = make_bookmark(user_id, "http://127.0.0.1:1/", None);

            let svc = BookmarkService::new(
                Arc::new(MockRepo::new(vec![bookmark])),
                Arc::new(NoopMetadata),
                Arc::new(NoopStorage),
            );
            let (tx, rx) = mpsc::channel(32);
            svc.fix_missing_images(user_id, Some(&screenshot_url), tx).await;
            let events = collect_events(rx).await;
            let last = events.last().unwrap();
            assert_eq!(last.fixed, 1, "should fix via screenshot sidecar");
            assert_eq!(last.failed, 0);
            assert!(last.done);
        }
    }
}
