use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use crate::domain::ports::metadata::MetadataExtractor;
use crate::domain::ports::storage::ObjectStorage;
use std::sync::Arc;
use uuid::Uuid;

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

        for (row, record) in records.into_iter().enumerate() {
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
                    let bookmark = Bookmark {
                        id,
                        user_id,
                        url: record.url,
                        title: record.title,
                        description: record.description,
                        image_url: record.image_url,
                        domain: record.domain,
                        tags: record.tags,
                        created_at: record.created_at.unwrap_or(now),
                        updated_at: record.updated_at.unwrap_or(now),
                    };

                    match self.repo.get(id, user_id).await {
                        Ok(_) => match strategy {
                            ImportStrategy::Skip => result.skipped += 1,
                            ImportStrategy::Upsert => {
                                self.repo.upsert_full(bookmark).await?;
                                result.updated += 1;
                            }
                        },
                        Err(DomainError::NotFound) => {
                            self.repo.insert_with_id(bookmark).await?;
                            result.created += 1;
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
        }

        impl MockRepo {
            fn new(bookmarks: Vec<Bookmark>) -> Self {
                Self { bookmarks: Mutex::new(bookmarks) }
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
                self.bookmarks.lock().unwrap().push(bookmark.clone());
                Ok(bookmark)
            }
            async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
                let mut bookmarks = self.bookmarks.lock().unwrap();
                if let Some(b) = bookmarks.iter_mut().find(|b| b.id == bookmark.id) {
                    *b = bookmark.clone();
                    Ok(bookmark)
                } else {
                    bookmarks.push(bookmark.clone());
                    Ok(bookmark)
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
            let mut record = make_record("https://example.com");
            record.id = Some(original_id);
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
            let mut record = make_record("https://example.com");
            record.id = Some(existing_id);
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
            let mut record = make_record("https://updated.com");
            record.id = Some(existing_id);
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
    }
}
