use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::embedding_repo::EmbeddingRepository;
use crate::domain::ports::search_repo::SearchIndexRepository;
use crate::domain::search::{EmbedJob, EmbeddedChunk, SearchIndexStatus};
use chrono::{DateTime, Utc};
use std::fmt::Write;
use std::time::Duration;
use uuid::Uuid;

fn db_error(e: sqlx::Error) -> DomainError {
    DomainError::Internal(e.to_string())
}

/// pgvector's text form, `[x,y,...]`. Rust prints finite floats without an
/// exponent, which `halfvec_in` reads; the adapters never return NaN.
fn vector_literal(values: &[f32]) -> String {
    let mut out = String::with_capacity(values.len() * 10 + 2);
    out.push('[');
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "{value}").expect("writing to a String cannot fail");
    }
    out.push(']');
    out
}

impl EmbeddingRepository for PostgresPool {
    async fn claim_embed_batch(
        &self,
        model_id: &str,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<EmbedJob>, DomainError> {
        // The lease moves `embed_next_attempt_at` and leaves `updated_at`
        // alone: `updated_at` is the version a result is checked against.
        sqlx::query_as::<_, EmbedJob>(
            "WITH picked AS (
                 SELECT bookmark_id FROM bookmark_contents
                 WHERE status <> 'pending'
                   AND (embedded_model IS NULL OR embedded_model <> $1)
                   AND embed_next_attempt_at <= now()
                 ORDER BY embed_next_attempt_at, bookmark_id
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED
             )
             UPDATE bookmark_contents c
             SET embed_attempts = c.embed_attempts + 1,
                 embed_next_attempt_at = now() + make_interval(secs => $3)
             FROM picked p JOIN bookmarks b ON b.id = p.bookmark_id
             WHERE c.bookmark_id = p.bookmark_id
             RETURNING c.bookmark_id, c.user_id, b.url, b.title, b.description, b.tags, c.text,
                       c.embed_attempts, c.updated_at AS version",
        )
        .bind(model_id)
        .bind(limit)
        .bind(lease.as_secs_f64())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)
    }

    async fn store_chunks(
        &self,
        job: &EmbedJob,
        model_id: &str,
        chunks: &[EmbeddedChunk],
    ) -> Result<bool, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        // Lock in the order a delete does (bookmark, then its contents and
        // chunks), so a concurrent delete waits instead of deadlocking: the
        // chunk inserts below would otherwise take this key lock last. A
        // deleted bookmark is simply gone.
        let exists = sqlx::query("SELECT 1 FROM bookmarks WHERE id = $1 FOR KEY SHARE")
            .bind(job.bookmark_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
        if exists.is_none() {
            return Ok(false);
        }
        // Locks the row: an edit's trigger waits for this commit and then
        // queues the bookmark again.
        let marked = sqlx::query(
            "UPDATE bookmark_contents
             SET embedded_model = $3, embed_attempts = 0, embed_error = NULL,
                 embed_next_attempt_at = now()
             WHERE bookmark_id = $1 AND updated_at = $2 AND status <> 'pending'",
        )
        .bind(job.bookmark_id)
        .bind(job.version)
        .bind(model_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if marked.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("DELETE FROM bookmark_chunks WHERE bookmark_id = $1")
            .bind(job.bookmark_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors: Vec<String> = chunks
            .iter()
            .map(|c| vector_literal(&c.embedding))
            .collect();
        sqlx::query(
            "INSERT INTO bookmark_chunks (bookmark_id, user_id, ord, text, embedding, model)
             SELECT $1, $2, u.ord - 1, u.text, u.embedding::halfvec, $3
             FROM unnest($4::text[], $5::text[]) WITH ORDINALITY AS u(text, embedding, ord)",
        )
        .bind(job.bookmark_id)
        .bind(job.user_id)
        .bind(model_id)
        .bind(&texts)
        .bind(&vectors)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(true)
    }

    async fn record_embed_failure(
        &self,
        job: &EmbedJob,
        model_id: &str,
        error: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<bool, DomainError> {
        // A final failure marks the row done for this model and drops chunks
        // of an earlier model, which the vector leg would skip anyway.
        let rows: i64 = sqlx::query_scalar(
            "WITH failed AS (
                 UPDATE bookmark_contents
                 SET embed_error = $3,
                     embed_next_attempt_at = coalesce($4, now()),
                     embedded_model = CASE WHEN $4 IS NULL THEN $5 ELSE embedded_model END,
                     embed_attempts = CASE WHEN $4 IS NULL THEN 0 ELSE embed_attempts END
                 WHERE bookmark_id = $1 AND updated_at = $2 AND status <> 'pending'
                 RETURNING bookmark_id
             ),
             dropped AS (
                 DELETE FROM bookmark_chunks ch USING failed f
                 WHERE ch.bookmark_id = f.bookmark_id AND $4::timestamptz IS NULL
             )
             SELECT count(*) FROM failed",
        )
        .bind(job.bookmark_id)
        .bind(job.version)
        .bind(error)
        .bind(retry_at)
        .bind(model_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows > 0)
    }
}

/// Hosts listed in the Search index panel.
const FAILING_DOMAINS_SHOWN: i64 = 5;

impl SearchIndexRepository for PostgresPool {
    async fn index_status(
        &self,
        user_id: Uuid,
        model_id: Option<&str>,
    ) -> Result<SearchIndexStatus, DomainError> {
        let (total, fetched, pending, unfetched, embedded): (i64, i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT count(*),
                        count(*) FILTER (WHERE status = 'fetched'),
                        count(*) FILTER (WHERE status = 'pending'),
                        count(*) FILTER (WHERE status IN ('failed', 'skipped')),
                        -- A final embedding failure is done for the model but has
                        -- no vectors (its error stays set until a success).
                        count(*) FILTER (WHERE embedded_model = $2 AND embed_error IS NULL)
                 FROM bookmark_contents WHERE user_id = $1",
            )
            .bind(user_id)
            .bind(model_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        let failing_domains = sqlx::query_as(
            "SELECT coalesce(b.domain,
                             lower(substring(b.url from '^[^:]+://(?:[^@/]*@)?([^/:?#]+)')),
                             b.url) AS host,
                    count(*)
             FROM bookmark_contents c JOIN bookmarks b ON b.id = c.bookmark_id
             WHERE c.user_id = $1 AND c.status IN ('failed', 'skipped')
             GROUP BY host
             ORDER BY count(*) DESC, host
             LIMIT $2",
        )
        .bind(user_id)
        .bind(FAILING_DOMAINS_SHOWN)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(SearchIndexStatus {
            total,
            fetched,
            pending,
            unfetched,
            embedded: model_id.map(|_| embedded),
            failing_domains,
        })
    }

    async fn retry_failed_fetches(&self, user_id: Uuid) -> Result<u64, DomainError> {
        requeue(&self.pool, user_id, None).await
    }

    async fn refetch(&self, user_id: Uuid, bookmark_id: Uuid) -> Result<bool, DomainError> {
        Ok(requeue(&self.pool, user_id, Some(bookmark_id)).await? > 0)
    }

    async fn extension_versions(&self) -> Result<Vec<(String, String)>, DomainError> {
        sqlx::query_as(
            "SELECT extname::text, extversion FROM pg_extension
             WHERE extname IN ('pg_trgm', 'vector') ORDER BY extname",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)
    }
}

/// Put fetches back in the ingest queue: one bookmark in any state, or
/// (`bookmark_id = None`) every failed and skipped one of the user. A fresh
/// start (attempts 0, no claim token, so a job in flight is dropped), and a
/// `NOTIFY` wakes the worker. The stored text and chunks stay searchable
/// until the new result replaces them; unchanged text keeps its chunks.
async fn requeue(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    bookmark_id: Option<Uuid>,
) -> Result<u64, DomainError> {
    let queued: i64 = sqlx::query_scalar(
        "WITH queued AS (
             UPDATE bookmark_contents
             SET status = 'pending', attempts = 0, next_attempt_at = now(),
                 claim_token = NULL, error = NULL, updated_at = now()
             WHERE user_id = $1
               AND CASE WHEN $2::uuid IS NULL THEN status IN ('failed', 'skipped')
                        ELSE bookmark_id = $2 END
             RETURNING bookmark_id
         )
         SELECT count(*) FROM queued, LATERAL (SELECT pg_notify('bookmark_ingest', '')) n",
    )
    .bind(user_id)
    .bind(bookmark_id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(queued as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_literals_are_pgvector_text() {
        assert_eq!(vector_literal(&[]), "[]");
        assert_eq!(vector_literal(&[0.5, -0.25, 1.0]), "[0.5,-0.25,1]");
        assert_eq!(vector_literal(&[0.00001]), "[0.00001]");
    }
}

#[cfg(test)]
mod embedding_tests {
    use super::*;
    use crate::adapters::postgres::search_repo::search_tests::{
        LEASE, SERIAL, bookmark, claim_mine, ids, pool, store, user,
    };
    use crate::app::embedding::tests::StubEmbedder;
    use crate::app::search::SearchService;
    use crate::domain::bookmark::{BookmarkFilter, BookmarkSort};
    use crate::domain::chunking::ChunkSource;
    use crate::domain::ports::bookmark_repo::BookmarkRepository;
    use crate::domain::ports::search_repo::{SearchIndexRepository, SearchRepository};
    use crate::domain::search::{
        IngestFailure, IngestStatus, MAX_SEARCH_RESULTS, QueryVector, SearchHit, SearchQuery,
    };
    use sqlx::PgPool;
    use std::sync::Arc;
    use uuid::Uuid;

    const DIMS: usize = 384;

    /// Claim every embed job of `users`, embed it with `embedder` the way the
    /// worker does, and store it. Rows of other tests are put back.
    async fn embed_all(
        pool: &PgPool,
        repo: &PostgresPool,
        embedder: &StubEmbedder,
        users: &[Uuid],
    ) {
        let jobs = repo
            .claim_embed_batch(&embedder.model_id, 1000, LEASE)
            .await
            .unwrap();
        let (mine, others): (Vec<EmbedJob>, Vec<EmbedJob>) =
            jobs.into_iter().partition(|j| users.contains(&j.user_id));
        release(pool, &others).await;
        for job in mine {
            let chunks: Vec<EmbeddedChunk> = chunk_source(&job)
                .chunks()
                .into_iter()
                .map(|c| EmbeddedChunk {
                    embedding: embedder.vector(&c.input),
                    text: c.text,
                })
                .collect();
            assert!(
                repo.store_chunks(&job, &embedder.model_id, &chunks)
                    .await
                    .unwrap()
            );
        }
    }

    fn chunk_source(job: &EmbedJob) -> ChunkSource<'_> {
        ChunkSource {
            title: job.title.as_deref(),
            description: job.description.as_deref(),
            url: &job.url,
            tags: &job.tags,
            text: job.text.as_deref(),
        }
    }

    async fn release(pool: &PgPool, jobs: &[EmbedJob]) {
        let ids: Vec<Uuid> = jobs.iter().map(|j| j.bookmark_id).collect();
        sqlx::query(
            "UPDATE bookmark_contents
             SET embed_next_attempt_at = now(), embed_attempts = embed_attempts - 1
             WHERE bookmark_id = ANY($1)",
        )
        .bind(&ids)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn claim_one(
        pool: &PgPool,
        repo: &PostgresPool,
        model: &str,
        id: Uuid,
    ) -> Option<EmbedJob> {
        let jobs = repo.claim_embed_batch(model, 1000, LEASE).await.unwrap();
        let (mine, others): (Vec<EmbedJob>, Vec<EmbedJob>) =
            jobs.into_iter().partition(|j| j.bookmark_id == id);
        release(pool, &others).await;
        mine.into_iter().next()
    }

    async fn chunks_of(pool: &PgPool, id: Uuid) -> Vec<(i32, String, String, i32)> {
        sqlx::query_as(
            "SELECT ord, text, model, vector_dims(embedding) FROM bookmark_chunks
             WHERE bookmark_id = $1 ORDER BY ord",
        )
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn embedded_model(pool: &PgPool, id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT embedded_model FROM bookmark_contents WHERE bookmark_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn search(
        repo: &PostgresPool,
        user_id: Uuid,
        text: &str,
        vector: Option<QueryVector>,
        tags: Option<&[&str]>,
    ) -> Vec<SearchHit> {
        repo.search(
            user_id,
            &SearchQuery {
                text: text.to_string(),
                tags: tags.map(|t| t.iter().map(|s| s.to_string()).collect()),
                sort: BookmarkSort::Relevance,
                limit: MAX_SEARCH_RESULTS,
                offset: 0,
                vector,
            },
        )
        .await
        .expect("search")
    }

    fn passage(topic: &str, words: usize) -> String {
        (0..words)
            .map(|i| format!("{topic}{}", i % 7))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run with a database migrated through 018 (pgvector):
    /// `DATABASE_URL=... cargo test -p boopmark-server adapters::postgres::embedding_repo::embedding_tests -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database with pgvector (DATABASE_URL)"]
    async fn chunks_feed_a_model_scoped_vector_leg() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let embedder = StubEmbedder::new(DIMS);
        let alice = user(&pool).await;
        let bob = user(&pool).await;

        let garden = bookmark(
            &repo,
            alice,
            "https://example.org/compost",
            "Composting at home",
            None,
            &["garden"],
        )
        .await;
        let kitchen = bookmark(
            &repo,
            alice,
            "https://example.org/bread",
            "Baking bread",
            None,
            &["food"],
        )
        .await;
        let failed = bookmark(
            &repo,
            alice,
            "https://example.org/paywalled",
            "Quantum annealing explained",
            Some("A paywalled feature"),
            &["physics"],
        )
        .await;
        let bobs = bookmark(
            &repo,
            bob,
            "https://example.com/bob-compost",
            "Bob's compost",
            None,
            &["garden"],
        )
        .await;
        let jobs = claim_mine(&pool, &repo, &[alice, bob]).await;
        // Long enough to be chunked: 3 windows.
        let garden_text = format!("{} {}", passage("mulch", 200), passage("worm", 200));
        store(&repo, &jobs[&garden.id], &garden_text).await;
        store(&repo, &jobs[&kitchen.id], &passage("dough", 120)).await;
        store(&repo, &jobs[&bobs.id], &garden_text).await;
        assert!(
            repo.record_ingest_failure(
                &jobs[&failed.id],
                &IngestFailure {
                    status: IngestStatus::Failed,
                    error: "HTTP 403".into(),
                    retry_at: None
                },
            )
            .await
            .unwrap()
        );

        embed_all(&pool, &repo, &embedder, &[alice, bob]).await;
        let garden_chunks = chunks_of(&pool, garden.id).await;
        assert_eq!(garden_chunks.len(), 3);
        assert_eq!(
            garden_chunks.iter().map(|c| c.0).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(
            garden_chunks
                .iter()
                .all(|c| c.2 == embedder.model_id && c.3 == DIMS as i32)
        );
        assert!(
            garden_chunks[0].1.starts_with("mulch0 mulch1"),
            "stored without the header"
        );
        // A failed fetch has one chunk made of its metadata.
        let meta = chunks_of(&pool, failed.id).await;
        assert_eq!(meta.len(), 1);
        assert_eq!(
            meta[0].1,
            "Quantum annealing explained\nA paywalled feature\nhttps://example.org/paywalled\nphysics"
        );
        assert_eq!(
            embedded_model(&pool, garden.id).await.as_deref(),
            Some(embedder.model_id.as_str())
        );
        // Nothing is left to embed for this model.
        assert!(
            claim_one(&pool, &repo, &embedder.model_id, garden.id)
                .await
                .is_none()
        );

        let vector = |text: &str| {
            Some(QueryVector {
                model_id: embedder.model_id.clone(),
                values: embedder.vector(text),
            })
        };
        // No keyword of the query is in any bookmark: only the vector leg
        // finds one, labelled semantic, with its best chunk as the snippet.
        let hits = search(&repo, alice, "zzqx", vector("worm3 worm4 worm5"), None).await;
        assert_eq!(ids(&hits)[0], garden.id);
        assert!(hits[0].semantic);
        let snippet = hits[0].snippet.as_ref().unwrap().plain_text();
        // One of its article chunks (not the metadata), cut to an excerpt.
        assert!(
            snippet.starts_with("mulch") || snippet.starts_with("worm"),
            "{snippet}"
        );
        assert!(snippet.chars().count() <= crate::domain::search::SNIPPET_EXCERPT_CHARS + 2);
        assert!(!ids(&hits).contains(&bobs.id), "user isolation");
        // The metadata chunk finds the failed fetch, with no snippet.
        let hits = search(&repo, alice, "zzqx", vector("quantum annealing"), None).await;
        assert_eq!(ids(&hits)[0], failed.id);
        assert!(hits[0].snippet.is_none());

        // Another model's query vector never meets these chunks, even at the
        // same size.
        let other_model = QueryVector {
            model_id: "other:model@384".into(),
            values: embedder.vector("worm3"),
        };
        assert!(
            search(&repo, alice, "zzqx", Some(other_model), None)
                .await
                .is_empty()
        );
        // Without a vector, the lexical legs alone (nothing matches).
        assert!(search(&repo, alice, "zzqx", None, None).await.is_empty());

        // Tags and exclusions constrain the vector leg too.
        let hits = search(
            &repo,
            alice,
            "zzqx",
            vector("worm3 dough1"),
            Some(&["food"]),
        )
        .await;
        assert_eq!(ids(&hits), [kitchen.id]);
        let hits = search(&repo, alice, "zzqx -compost", vector("worm3"), None).await;
        assert!(!ids(&hits).contains(&garden.id));
        let hits = search(&repo, alice, "zzqx - worm1", vector("worm3"), None).await;
        assert!(
            !ids(&hits).contains(&garden.id),
            "a spaced minus excludes too"
        );

        // A keyword hit found by both legs is not labelled semantic.
        let hits = search(&repo, alice, "compost", vector("compost"), None).await;
        assert_eq!(ids(&hits)[0], garden.id);
        assert!(!hits[0].semantic);

        // The leg's cast and size filter match the partial HNSW index for its
        // size. (For one small library the planner rightly prefers an exact
        // scan of the user's rows, so the user filter is left out here.)
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL enable_seqscan = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let plan: Vec<String> = sqlx::query_scalar(
            "EXPLAIN SELECT bookmark_id FROM bookmark_chunks
             WHERE model = $1 AND vector_dims(embedding) = 384
             ORDER BY embedding::halfvec(384) <=> $2::real[]::halfvec(384) LIMIT 200",
        )
        .bind(&embedder.model_id)
        .bind(embedder.vector("worm"))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert!(
            plan.join("\n").contains("idx_bookmark_chunks_hnsw_384"),
            "{plan:?}"
        );

        // Deleting a bookmark deletes its chunks, and a result for it that
        // arrives later is dropped.
        sqlx::query("UPDATE bookmark_contents SET embedded_model = NULL WHERE bookmark_id = $1")
            .bind(kitchen.id)
            .execute(&pool)
            .await
            .unwrap();
        let job = claim_one(&pool, &repo, &embedder.model_id, kitchen.id)
            .await
            .unwrap();
        repo.delete(kitchen.id, alice).await.unwrap();
        assert!(chunks_of(&pool, kitchen.id).await.is_empty());
        let late = [EmbeddedChunk {
            text: "late".into(),
            embedding: embedder.vector("late"),
        }];
        assert!(
            !repo
                .store_chunks(&job, &embedder.model_id, &late)
                .await
                .unwrap()
        );
        assert!(chunks_of(&pool, kitchen.id).await.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires a migrated Postgres database with pgvector (DATABASE_URL)"]
    async fn edits_requeue_chunks_and_stale_results_are_dropped() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let embedder = StubEmbedder::new(DIMS);
        let model = embedder.model_id.clone();
        let owner = user(&pool).await;
        let article = bookmark(
            &repo,
            owner,
            "https://example.org/a",
            "Article",
            None,
            &["one"],
        )
        .await;
        let failed = bookmark(
            &repo,
            owner,
            "https://example.org/f",
            "Failed page",
            None,
            &["one"],
        )
        .await;
        let jobs = claim_mine(&pool, &repo, &[owner]).await;
        let text = passage("prose", 300);
        assert!(
            repo.store_article(&jobs[&article.id], &text, "h1")
                .await
                .unwrap()
        );
        assert!(
            repo.record_ingest_failure(
                &jobs[&failed.id],
                &IngestFailure {
                    status: IngestStatus::Skipped,
                    error: "PDF".into(),
                    retry_at: None
                },
            )
            .await
            .unwrap()
        );
        // Pending rows are not embedded yet.
        let pending = bookmark(&repo, owner, "https://example.org/p", "Pending", None, &[]).await;
        assert!(claim_one(&pool, &repo, &model, pending.id).await.is_none());

        // An edit after the claim makes the result stale.
        let job = claim_one(&pool, &repo, &model, article.id)
            .await
            .expect("queued");
        assert_eq!(job.text.as_deref(), Some(text.as_str()));
        update(&pool, article.id, Some("Article, retitled"), None).await;
        assert!(!repo.store_chunks(&job, &model, &[]).await.unwrap());
        embed_all(&pool, &repo, &embedder, &[owner]).await;
        let first = chunks_of(&pool, article.id).await;
        assert_eq!(first.len(), 2);

        let tags = |pool: PgPool, id: Uuid, tags: &'static [&'static str]| async move {
            sqlx::query("UPDATE bookmarks SET tags = $2 WHERE id = $1")
                .bind(id)
                .bind(tags)
                .execute(&pool)
                .await
                .unwrap();
        };
        // A tag edit re-embeds only a metadata chunk (tags are only in it).
        tags(pool.clone(), article.id, &["two"]).await;
        tags(pool.clone(), failed.id, &["two"]).await;
        assert_eq!(
            embedded_model(&pool, article.id).await.as_deref(),
            Some(model.as_str())
        );
        assert_eq!(embedded_model(&pool, failed.id).await, None);
        embed_all(&pool, &repo, &embedder, &[owner]).await;
        assert!(chunks_of(&pool, failed.id).await[0].1.ends_with("\ntwo"));
        // A title edit re-embeds an article (the title is in every header).
        update(&pool, article.id, Some("New title"), None).await;
        assert_eq!(embedded_model(&pool, article.id).await, None);
        embed_all(&pool, &repo, &embedder, &[owner]).await;

        // A refetch of the same text keeps the chunks; new text needs new ones.
        let refetch = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "UPDATE bookmark_contents SET status = 'pending' WHERE bookmark_id = $1",
                )
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        refetch(article.id).await;
        let jobs = claim_mine(&pool, &repo, &[owner]).await;
        assert!(
            repo.store_article(&jobs[&article.id], &text, "h1")
                .await
                .unwrap()
        );
        assert_eq!(
            embedded_model(&pool, article.id).await.as_deref(),
            Some(model.as_str())
        );
        refetch(article.id).await;
        let jobs = claim_mine(&pool, &repo, &[owner]).await;
        assert!(
            repo.store_article(&jobs[&article.id], &passage("other", 50), "h2")
                .await
                .unwrap()
        );
        assert_eq!(embedded_model(&pool, article.id).await, None);
        embed_all(&pool, &repo, &embedder, &[owner]).await;
        assert_eq!(
            chunks_of(&pool, article.id).await.len(),
            1,
            "short text: metadata chunk"
        );

        // A model change re-embeds everything, replacing the old chunks.
        let next = StubEmbedder {
            model_id: "stub:next@384".into(),
            ..StubEmbedder::new(DIMS)
        };
        assert!(
            claim_one(&pool, &repo, &next.model_id, failed.id)
                .await
                .is_some()
        );
        release_one(&pool, failed.id).await;
        embed_all(&pool, &repo, &next, &[owner]).await;
        let replaced = chunks_of(&pool, article.id).await;
        assert!(replaced.iter().all(|c| c.2 == "stub:next@384"));

        // Failures: a transient one is retried later, a final one marks the
        // row done for this model and drops its old chunks.
        sqlx::query("UPDATE bookmark_contents SET embedded_model = NULL WHERE bookmark_id = $1")
            .bind(failed.id)
            .execute(&pool)
            .await
            .unwrap();
        let job = claim_one(&pool, &repo, &model, failed.id).await.unwrap();
        let later = Utc::now() + chrono::Duration::hours(1);
        assert!(
            repo.record_embed_failure(&job, &model, "HTTP 503", Some(later))
                .await
                .unwrap()
        );
        assert!(
            claim_one(&pool, &repo, &model, failed.id).await.is_none(),
            "not due yet"
        );
        assert_eq!(
            chunks_of(&pool, failed.id).await.len(),
            1,
            "kept until it is replaced"
        );
        release_one(&pool, failed.id).await;
        let job = claim_one(&pool, &repo, &model, failed.id).await.unwrap();
        assert!(
            repo.record_embed_failure(&job, &model, "HTTP 400", None)
                .await
                .unwrap()
        );
        assert_eq!(
            embedded_model(&pool, failed.id).await.as_deref(),
            Some(model.as_str())
        );
        assert!(chunks_of(&pool, failed.id).await.is_empty());

        // A URL change drops the old page's chunks and queues a fetch.
        update(&pool, article.id, None, Some("https://example.org/moved")).await;
        assert!(chunks_of(&pool, article.id).await.is_empty());
        assert_eq!(embedded_model(&pool, article.id).await, None);
    }

    /// The vector leg's query embedding times out after 1.5 s; the search
    /// then answers from the lexical legs.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database with pgvector (DATABASE_URL)"]
    async fn a_slow_embedder_falls_back_to_lexical_results() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = Arc::new(PostgresPool::new(pool.clone()));
        let owner = user(&pool).await;
        let hit = bookmark(
            &repo,
            owner,
            "https://example.org/ownership",
            "Understanding ownership",
            None,
            &[],
        )
        .await;
        let slow = StubEmbedder {
            delay: std::time::Duration::from_secs(30),
            ..StubEmbedder::new(DIMS)
        };
        let service = SearchService::new(repo.clone(), Arc::new(slow));
        let started = std::time::Instant::now();
        let hits = service
            .search(
                owner,
                BookmarkFilter {
                    search: Some("ownership".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(ids(&hits), [hit.id]);
        assert!(!hits[0].semantic);
    }

    #[tokio::test]
    #[ignore = "requires a migrated Postgres database with pgvector (DATABASE_URL)"]
    async fn index_status_retry_and_refetch() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let embedder = StubEmbedder::new(DIMS);
        let owner = user(&pool).await;
        let other = user(&pool).await;
        let fetched = bookmark(&repo, owner, "https://ok.example/a", "Fetched", None, &[]).await;
        let gone = bookmark(&repo, owner, "https://gone.example/1", "Gone 1", None, &[]).await;
        let gone2 = bookmark(&repo, owner, "https://gone.example/2", "Gone 2", None, &[]).await;
        let pdf = bookmark(
            &repo,
            owner,
            "https://files.example/x.pdf",
            "PDF",
            None,
            &[],
        )
        .await;
        let waiting = bookmark(
            &repo,
            owner,
            "https://later.example/w",
            "Waiting",
            None,
            &[],
        )
        .await;
        let theirs = bookmark(
            &repo,
            other,
            "https://gone.example/3",
            "Not mine",
            None,
            &[],
        )
        .await;
        let jobs = claim_mine(&pool, &repo, &[owner, other]).await;
        store(&repo, &jobs[&fetched.id], &passage("text", 100)).await;
        for (b, status) in [
            (&gone, IngestStatus::Failed),
            (&gone2, IngestStatus::Failed),
            (&pdf, IngestStatus::Skipped),
            (&theirs, IngestStatus::Failed),
        ] {
            let failure = IngestFailure {
                status,
                error: "x".into(),
                retry_at: None,
            };
            assert!(
                repo.record_ingest_failure(&jobs[&b.id], &failure)
                    .await
                    .unwrap()
            );
        }
        // Still pending: its retry is due later.
        let failure = IngestFailure {
            status: IngestStatus::Pending,
            error: "HTTP 503".into(),
            retry_at: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        assert!(
            repo.record_ingest_failure(&jobs[&waiting.id], &failure)
                .await
                .unwrap()
        );
        embed_all(&pool, &repo, &embedder, &[owner]).await;
        // A final embedding failure is done for the model but has no vectors.
        sqlx::query("UPDATE bookmark_contents SET embedded_model = NULL WHERE bookmark_id = $1")
            .bind(pdf.id)
            .execute(&pool)
            .await
            .unwrap();
        let job = claim_one(&pool, &repo, &embedder.model_id, pdf.id)
            .await
            .unwrap();
        assert!(
            repo.record_embed_failure(&job, &embedder.model_id, "HTTP 400", None)
                .await
                .unwrap()
        );

        let status = repo
            .index_status(owner, Some(&embedder.model_id))
            .await
            .unwrap();
        assert_eq!(
            (
                status.total,
                status.fetched,
                status.pending,
                status.unfetched
            ),
            (5, 1, 1, 3)
        );
        assert_eq!(status.embedded, Some(3), "non-pending rows with vectors");
        assert_eq!(
            status.failing_domains,
            vec![
                ("gone.example".to_string(), 2),
                ("files.example".to_string(), 1)
            ]
        );
        assert_eq!(repo.index_status(owner, None).await.unwrap().embedded, None);

        // Retry queues only the user's failed and skipped rows, at once.
        assert_eq!(repo.retry_failed_fetches(owner).await.unwrap(), 3);
        let status = repo.index_status(owner, None).await.unwrap();
        assert_eq!((status.pending, status.unfetched), (4, 0));
        assert_eq!(repo.index_status(other, None).await.unwrap().unfetched, 1);
        // Their metadata chunks stay searchable meanwhile.
        assert_eq!(chunks_of(&pool, gone.id).await.len(), 1);

        // Refetch works on any state, but only on the user's own bookmark.
        assert!(repo.refetch(owner, fetched.id).await.unwrap());
        assert!(!repo.refetch(owner, theirs.id).await.unwrap());
        assert!(!repo.refetch(owner, Uuid::new_v4()).await.unwrap());
        let refetched: (String, i32) =
            sqlx::query_as("SELECT status, attempts FROM bookmark_contents WHERE bookmark_id = $1")
                .bind(fetched.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(refetched, ("pending".to_string(), 0));

        let versions = repo.extension_versions().await.unwrap();
        let names: Vec<&str> = versions.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["pg_trgm", "vector"]);
        let service = SearchService::new(Arc::new(repo.clone()), Arc::new(embedder));
        let line = service.deployment_line().await.unwrap();
        assert!(line.starts_with("stub:hash@384 · vector 0."), "{line}");
        assert!(line.contains(" · pg_trgm 1."), "{line}");
    }

    async fn release_one(pool: &PgPool, id: Uuid) {
        sqlx::query(
            "UPDATE bookmark_contents SET embed_next_attempt_at = now() WHERE bookmark_id = $1",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// An edit through SQL, which fires the sync trigger like every write
    /// path (the repository cannot change a URL outside an import).
    async fn update(pool: &PgPool, id: Uuid, title: Option<&str>, url: Option<&str>) {
        sqlx::query(
            "UPDATE bookmarks SET title = coalesce($2, title), url = coalesce($3, url) WHERE id = $1",
        )
        .bind(id)
        .bind(title)
        .bind(url)
        .execute(pool)
        .await
        .unwrap();
    }
}
