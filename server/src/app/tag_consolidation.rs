use std::collections::HashMap;

/// Compute the new tag list for a bookmark.
///
/// For each current tag:
/// - Look up its mapping. If absent or empty, treat as identity (`[tag]`).
/// - Collect every value from every mapping output for the bookmark's tags.
/// - Lowercase, dedupe (case-insensitive), and sort lexicographically.
pub fn compute_new_tags(current: &[String], mapping: &HashMap<String, Vec<String>>) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut acc: BTreeSet<String> = BTreeSet::new();
    for tag in current {
        let outputs = match mapping.get(&tag.to_lowercase()) {
            Some(values) if values.iter().any(|v| !v.trim().is_empty()) => values.clone(),
            _ => vec![tag.clone()],
        };
        for out in outputs {
            let normalized = out.trim().to_lowercase();
            if !normalized.is_empty() {
                acc.insert(normalized);
            }
        }
    }
    acc.into_iter().collect()
}

const MAX_CONSOLIDATION_MAPPING_ENTRIES: usize = 500;
const MAX_CONSOLIDATION_VALUES_PER_TAG: usize = 32;
const MAX_CONSOLIDATED_TAG_CHARS: usize = 100;

/// Validate an LLM mapping before it can influence bookmark writes. Mapping
/// keys must refer to tags sent in the request; output values are bounded and
/// cannot contain control characters. Missing submitted keys remain safe and
/// are handled as identity mappings by `compute_new_tags`.
fn validate_consolidation_mapping(
    mapping: HashMap<String, Vec<String>>,
    samples: &[crate::domain::ports::tag_consolidator::TagSample],
) -> Result<HashMap<String, Vec<String>>, DomainError> {
    use std::collections::HashSet;

    if mapping.len() > MAX_CONSOLIDATION_MAPPING_ENTRIES {
        return Err(DomainError::InvalidInput(
            "AI returned too many tag mappings".to_string(),
        ));
    }

    let submitted_tags: HashSet<String> = samples
        .iter()
        .map(|sample| sample.tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    let mut validated = HashMap::with_capacity(mapping.len());

    for (raw_key, values) in mapping {
        let key = validate_tag_text(raw_key, "mapping key")?.to_lowercase();
        if !submitted_tags.contains(&key) {
            return Err(DomainError::InvalidInput(
                "AI returned a mapping for an unknown tag".to_string(),
            ));
        }
        if values.len() > MAX_CONSOLIDATION_VALUES_PER_TAG {
            return Err(DomainError::InvalidInput(
                "AI returned too many replacement tags".to_string(),
            ));
        }

        let mut normalized_values = values
            .into_iter()
            .map(|value| validate_tag_text(value, "replacement tag"))
            .collect::<Result<Vec<_>, _>>()?;
        normalized_values.sort();
        normalized_values.dedup();
        // Case-insensitive aliases such as `JavaScript` and `javascript` can
        // legitimately appear as separate submitted tags. They share one
        // lookup key, so accept duplicate normalized mappings only when their
        // normalized values agree; conflicting outputs are ambiguous and are
        // rejected before any bookmark write.
        if let Some(existing_values) = validated.get(&key) {
            if existing_values != &normalized_values {
                return Err(DomainError::InvalidInput(
                    "AI returned conflicting tag mappings".to_string(),
                ));
            }
        } else {
            // Preserve an empty output vector as the service's intentional
            // identity fallback; non-empty values are normalized above.
            validated.insert(key, normalized_values);
        }
    }

    Ok(validated)
}

fn validate_tag_text(value: String, field: &str) -> Result<String, DomainError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(DomainError::InvalidInput(format!(
            "AI returned an empty {field}"
        )));
    }
    if normalized.chars().count() > MAX_CONSOLIDATED_TAG_CHARS {
        return Err(DomainError::InvalidInput(format!(
            "AI returned an oversized {field}"
        )));
    }
    if normalized.chars().any(char::is_control) {
        return Err(DomainError::InvalidInput(format!(
            "AI returned malformed {field}"
        )));
    }
    Ok(normalized.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::tag_consolidator::TagSample;

    fn map(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, vs)| {
                (
                    (*k).to_string(),
                    vs.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn merges_variants_into_one_canonical() {
        let mapping = map(&[
            ("js", &["javascript"]),
            ("javascript", &["javascript"]),
            ("JavaScript", &["javascript"]),
        ]);
        let result = compute_new_tags(&["js".into(), "JavaScript".into()], &mapping);
        assert_eq!(result, vec!["javascript".to_string()]);
    }

    #[test]
    fn adds_parent_tag_alongside_narrow_tag() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn omitted_tag_is_treated_as_identity() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into(), "rust".into()], &mapping);
        assert_eq!(
            result,
            vec![
                "frontend".to_string(),
                "react".to_string(),
                "rust".to_string()
            ]
        );
    }

    #[test]
    fn empty_mapping_value_is_treated_as_identity() {
        let mapping = map(&[("react", &[])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["react".to_string()]);
    }

    #[test]
    fn outputs_are_lowercased() {
        let mapping = map(&[("react", &["React", "FRONTEND"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn deduplicates_case_insensitively() {
        let mapping = map(&[
            ("react", &["react", "frontend"]),
            ("vue", &["vue", "Frontend"]),
        ]);
        let result = compute_new_tags(&["react".into(), "vue".into()], &mapping);
        assert_eq!(
            result,
            vec![
                "frontend".to_string(),
                "react".to_string(),
                "vue".to_string()
            ]
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        let mapping = map(&[("react", &["react"])]);
        let result = compute_new_tags(&[], &mapping);
        assert!(result.is_empty());
    }

    #[test]
    fn whitespace_or_empty_string_values_are_treated_as_identity() {
        let mapping = map(&[("react", &[""])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["react".to_string()]);

        let mapping = map(&[("react", &["   "])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["react".to_string()]);
    }

    #[test]
    fn lookup_is_case_insensitive_against_lowercase_mapping_keys() {
        // LLM returns lowercase keys (per "use lowercase" rule); bookmark tags
        // may be stored in any case (Postgres preserves case in tag arrays).
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["React".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn mapping_validation_rejects_unknown_keys() {
        let error = validate_consolidation_mapping(
            map(&[("unsubmitted", &["safe"])]),
            &samples_for_validation(&["react"]),
        )
        .expect_err("unknown mapping key should be rejected");
        assert!(matches!(error, DomainError::InvalidInput(_)));
    }

    #[test]
    fn mapping_validation_rejects_oversized_or_control_values() {
        let oversized = "x".repeat(MAX_CONSOLIDATED_TAG_CHARS + 1);
        let error = validate_consolidation_mapping(
            map(&[("react", &[oversized.as_str()])]),
            &samples_for_validation(&["react"]),
        )
        .expect_err("oversized replacement should be rejected");
        assert!(matches!(error, DomainError::InvalidInput(_)));

        let error = validate_consolidation_mapping(
            map(&[("react", &["safe\nvalue"])]),
            &samples_for_validation(&["react"]),
        )
        .expect_err("control characters should be rejected");
        assert!(matches!(error, DomainError::InvalidInput(_)));
    }

    #[test]
    fn mapping_validation_normalizes_case_and_whitespace() {
        let validated = validate_consolidation_mapping(
            map(&[(" React ", &[" Frontend ", "REACT"])]),
            &samples_for_validation(&["react"]),
        )
        .expect("safe mapping");
        assert_eq!(
            validated.get("react"),
            Some(&vec!["frontend".to_string(), "react".to_string()])
        );
    }

    #[test]
    fn mapping_validation_rejects_conflicting_case_aliases() {
        let error = validate_consolidation_mapping(
            map(&[("React", &["react"]), ("react", &["frontend"])]),
            &samples_for_validation(&["react"]),
        )
        .expect_err("conflicting aliases should be rejected");
        assert!(matches!(error, DomainError::InvalidInput(_)));
    }

    fn samples_for_validation(tags: &[&str]) -> Vec<TagSample> {
        tags.iter()
            .map(|tag| TagSample {
                tag: (*tag).to_string(),
                count: 1,
                sample_titles: Vec::new(),
            })
            .collect()
    }
}

use crate::app::settings::SettingsService;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use crate::domain::ports::tag_consolidator::{ConsolidationInput, TagConsolidator};
use std::sync::Arc;
use uuid::Uuid;

pub const MIN_TAGS_FOR_CONSOLIDATION: usize = 5;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsolidationStats {
    pub bookmarks_changed: u64,
    pub tags_before: usize,
    pub tags_after: usize,
}

pub struct TagConsolidationService<B, R> {
    bookmarks: Arc<B>,
    consolidator: Arc<dyn TagConsolidator>,
    settings: Arc<SettingsService<R>>,
}

impl<B, R> TagConsolidationService<B, R>
where
    B: BookmarkRepository + Send + Sync,
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(
        bookmarks: Arc<B>,
        consolidator: Arc<dyn TagConsolidator>,
        settings: Arc<SettingsService<R>>,
    ) -> Self {
        Self {
            bookmarks,
            consolidator,
            settings,
        }
    }

    pub async fn consolidate(&self, user_id: Uuid) -> Result<ConsolidationStats, DomainError> {
        // 1. Get the selected provider's key + model.
        let credentials = self
            .settings
            .get_text_provider_credentials(user_id)
            .await?
            .ok_or_else(|| {
                DomainError::InvalidInput(
                    "No AI provider API key configured. Save one in Settings first.".to_string(),
                )
            })?;

        // 2. Load tag samples.
        let samples = self.bookmarks.tag_samples(user_id).await?;
        if samples.len() < MIN_TAGS_FOR_CONSOLIDATION {
            return Err(DomainError::InvalidInput(format!(
                "Need at least {MIN_TAGS_FOR_CONSOLIDATION} tags to consolidate; you have {}.",
                samples.len()
            )));
        }
        let tags_before = samples.len();

        // 3. Ask the LLM.
        let output = self
            .consolidator
            .consolidate_with_provider(
                credentials.provider,
                &credentials.api_key,
                &credentials.model,
                ConsolidationInput {
                    tags: samples.clone(),
                },
            )
            .await?;
        let mapping = validate_consolidation_mapping(output.mapping, &samples)?;

        // 4. Compute per-bookmark new tags.
        let id_tags = self.bookmarks.list_id_tags(user_id).await?;
        let mut updates: Vec<(Uuid, Vec<String>)> = Vec::new();
        let mut after_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (id, current) in id_tags {
            let new_tags = compute_new_tags(&current, &mapping);
            for t in &new_tags {
                after_set.insert(t.clone());
            }
            // Compare ignoring order; current may not be sorted but compute_new_tags returns sorted.
            let mut current_sorted = current.clone();
            current_sorted.sort();
            current_sorted.dedup();
            let mut new_sorted = new_tags.clone();
            new_sorted.sort();
            new_sorted.dedup();
            if current_sorted != new_sorted {
                updates.push((id, new_tags));
            }
        }

        // 5. Apply.
        let bookmarks_changed = self.bookmarks.update_tags_bulk(user_id, &updates).await?;

        Ok(ConsolidationStats {
            bookmarks_changed,
            tags_before,
            tags_after: after_set.len(),
        })
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::app::secrets::SecretBox;
    use crate::app::settings::SaveLlmSettingsInput;
    use crate::domain::bookmark::{Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark};
    use crate::domain::llm_settings::LlmSettings;
    use crate::domain::ports::bookmark_repo::CreateIdempotency;
    use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
    use crate::domain::ports::tag_consolidator::{
        ConsolidationInput, ConsolidationOutput, TagConsolidator, TagSample,
    };
    use chrono::Utc;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    // ---- Stub LLM settings repo (saves and returns one record) ----
    struct StubLlmSettingsRepo {
        stored: Mutex<Option<LlmSettings>>,
    }
    impl StubLlmSettingsRepo {
        fn new() -> Self {
            Self {
                stored: Mutex::new(None),
            }
        }
    }
    impl LlmSettingsRepository for StubLlmSettingsRepo {
        async fn get(&self, _user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
            Ok(self.stored.lock().unwrap().clone())
        }
        async fn upsert(
            &self,
            user_id: Uuid,
            update: crate::domain::ports::llm_settings_repo::LlmSettingsUpdate,
        ) -> Result<LlmSettings, DomainError> {
            let existing = self.stored.lock().unwrap().clone();
            let encrypted = if update.clear_anthropic_api_key {
                None
            } else {
                update
                    .replace_anthropic_api_key_encrypted
                    .clone()
                    .or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|s| s.anthropic_api_key_encrypted.clone())
                    })
            };
            let saved = LlmSettings {
                user_id,
                enabled: update.enabled,
                metadata_provider: update.metadata_provider,
                anthropic_api_key_encrypted: encrypted,
                anthropic_model: update.anthropic_model,
                openai_api_key_encrypted: update.replace_openai_api_key_encrypted,
                openai_model: update.openai_model,
                image_generation_enabled: update.image_generation_enabled,
                image_generation_model: update.image_generation_model,
                created_at: existing.map(|s| s.created_at).unwrap_or_else(Utc::now),
                updated_at: Utc::now(),
            };
            *self.stored.lock().unwrap() = Some(saved.clone());
            Ok(saved)
        }
    }

    // ---- Stub bookmark repo (only the methods used by the service matter) ----
    struct StubBookmarkRepo {
        bookmarks: Mutex<Vec<Bookmark>>,
        samples: Vec<TagSample>,
    }
    impl StubBookmarkRepo {
        fn new(bookmarks: Vec<Bookmark>, samples: Vec<TagSample>) -> Self {
            Self {
                bookmarks: Mutex::new(bookmarks),
                samples,
            }
        }
    }
    impl BookmarkRepository for StubBookmarkRepo {
        async fn create(&self, _: Uuid, _: CreateBookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn claim_create(
            &self,
            _: Uuid,
            _: CreateIdempotency,
        ) -> Result<crate::domain::ports::bookmark_repo::CreateIdempotencyClaim, DomainError>
        {
            unimplemented!()
        }
        async fn create_claimed(
            &self,
            _: Uuid,
            _: CreateBookmark,
            _: CreateIdempotency,
        ) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn get(&self, _: Uuid, _: Uuid) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn list(&self, _: Uuid, _: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _: Uuid,
            _: Uuid,
            _: UpdateBookmark,
        ) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn delete(&self, _: Uuid, _: Uuid) -> Result<(), DomainError> {
            unimplemented!()
        }
        async fn delete_with_override(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Option<String>, DomainError> {
            unimplemented!()
        }
        async fn all_tags(&self, _: Uuid) -> Result<Vec<String>, DomainError> {
            unimplemented!()
        }
        async fn tags_with_counts(&self, _: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
            unimplemented!()
        }
        async fn export_all(&self, _: Uuid) -> Result<Vec<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn find_by_url(&self, _: Uuid, _: &str) -> Result<Option<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn insert_with_id(&self, _: Bookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn upsert_full(&self, _: Bookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn update_image_url(&self, _: Uuid, _: Uuid, _: &str) -> Result<(), DomainError> {
            unimplemented!()
        }
        async fn replace_override_image_url(
            &self,
            _: Uuid,
            _: Uuid,
            _: Option<&str>,
        ) -> Result<Option<String>, DomainError> {
            unimplemented!()
        }
        async fn tag_samples(&self, _: Uuid) -> Result<Vec<TagSample>, DomainError> {
            Ok(self.samples.clone())
        }
        async fn list_id_tags(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
            Ok(self
                .bookmarks
                .lock()
                .unwrap()
                .iter()
                .filter(|b| b.user_id == user_id)
                .map(|b| (b.id, b.tags.clone()))
                .collect())
        }
        async fn update_tags_bulk(
            &self,
            user_id: Uuid,
            updates: &[(Uuid, Vec<String>)],
        ) -> Result<u64, DomainError> {
            let mut bookmarks = self.bookmarks.lock().unwrap();
            let mut rows = 0u64;
            for (id, new_tags) in updates {
                if let Some(b) = bookmarks
                    .iter_mut()
                    .find(|b| b.id == *id && b.user_id == user_id)
                {
                    b.tags = new_tags.clone();
                    rows += 1;
                }
            }
            Ok(rows)
        }
    }

    // ---- Stub TagConsolidator that returns a canned mapping ----
    struct StubConsolidator {
        mapping: HashMap<String, Vec<String>>,
    }
    impl StubConsolidator {
        fn new(pairs: &[(&str, &[&str])]) -> Self {
            Self {
                mapping: pairs
                    .iter()
                    .map(|(k, vs)| {
                        (
                            (*k).to_string(),
                            vs.iter().map(|v| (*v).to_string()).collect(),
                        )
                    })
                    .collect(),
            }
        }
    }
    impl TagConsolidator for StubConsolidator {
        fn consolidate(
            &self,
            _api_key: &str,
            _model: &str,
            _input: ConsolidationInput,
        ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>
        {
            let mapping = self.mapping.clone();
            Box::pin(async move { Ok(ConsolidationOutput { mapping }) })
        }
    }

    // ---- Helpers ----
    fn bookmark(user_id: Uuid, tags: &[&str]) -> Bookmark {
        Bookmark {
            id: Uuid::new_v4(),
            user_id,
            url: "https://example.com".into(),
            title: Some("t".into()),
            description: None,
            image_url: None,
            override_image_url: None,
            domain: None,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn samples(tags: &[(&str, i64)]) -> Vec<TagSample> {
        tags.iter()
            .map(|(t, c)| TagSample {
                tag: (*t).to_string(),
                count: *c,
                sample_titles: vec![],
            })
            .collect()
    }

    #[tokio::test]
    async fn returns_invalid_input_when_no_api_key() {
        let user_id = Uuid::new_v4();
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(vec![], vec![]));
        let consolidator = Arc::new(StubConsolidator::new(&[]));
        // No api key saved
        let llm_repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let settings = Arc::new(SettingsService::new(llm_repo, secret_box));
        let service = TagConsolidationService::new(bookmark_repo, consolidator, settings);

        let err = service.consolidate(user_id).await.expect_err("should err");
        assert!(matches!(err, DomainError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_input_when_too_few_tags() {
        let user_id = Uuid::new_v4();
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(
            vec![bookmark(user_id, &["a"])],
            samples(&[("a", 1)]), // 1 < MIN_TAGS_FOR_CONSOLIDATION (5)
        ));
        let consolidator = Arc::new(StubConsolidator::new(&[]));
        // Save a key for THIS user_id so we get past the API-key gate
        let llm_repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let settings = Arc::new(SettingsService::new(llm_repo, secret_box));
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-x".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let service = TagConsolidationService::new(bookmark_repo, consolidator, settings);
        let err = service.consolidate(user_id).await.expect_err("should err");
        assert!(matches!(err, DomainError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn applies_mapping_and_returns_stats() {
        let user_id = Uuid::new_v4();
        let bookmarks = vec![
            bookmark(user_id, &["js", "react"]),
            bookmark(user_id, &["JavaScript", "vue"]),
            bookmark(user_id, &["rust"]),
            bookmark(user_id, &["javascript"]),
            bookmark(user_id, &["react"]),
        ];
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(
            bookmarks,
            samples(&[
                ("js", 1),
                ("JavaScript", 1),
                ("javascript", 1),
                ("react", 2),
                ("vue", 1),
                ("rust", 1),
            ]),
        ));
        let consolidator = Arc::new(StubConsolidator::new(&[
            ("js", &["javascript"]),
            ("JavaScript", &["javascript"]),
            ("javascript", &["javascript"]),
            ("react", &["react", "frontend"]),
            ("vue", &["vue", "frontend"]),
            ("rust", &["rust"]),
        ]));

        let llm_repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let settings = Arc::new(SettingsService::new(llm_repo, secret_box));
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-x".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let service = TagConsolidationService::new(bookmark_repo.clone(), consolidator, settings);
        let stats = service.consolidate(user_id).await.expect("ok");

        // 6 distinct input tags. After applying the mapping, the unique output
        // tags across all bookmarks are: javascript, react, frontend, vue, rust.
        assert_eq!(stats.tags_before, 6);
        assert_eq!(stats.tags_after, 5);
        // 5 bookmarks; only the lone "rust" and lone "javascript" bookmarks
        // are unchanged (the mapping is identity for them after sort/dedupe).
        assert_eq!(stats.bookmarks_changed, 3);

        // Verify some bookmark states.
        let bookmarks = bookmark_repo.bookmarks.lock().unwrap();
        // "rust" bookmark unchanged.
        assert!(
            bookmarks.iter().any(|b| b.tags == vec!["rust".to_string()]),
            "expected an unchanged rust bookmark"
        );
        // The lone "javascript" bookmark unchanged.
        assert!(
            bookmarks
                .iter()
                .any(|b| b.tags == vec!["javascript".to_string()]),
            "expected an unchanged javascript bookmark"
        );
        // A "js"+"react" bookmark should now be ["frontend", "javascript", "react"].
        assert!(
            bookmarks.iter().any(|b| b.tags
                == vec![
                    "frontend".to_string(),
                    "javascript".to_string(),
                    "react".to_string()
                ]),
            "expected merged js+react bookmark"
        );
    }
}
