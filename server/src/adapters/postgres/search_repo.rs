use super::PostgresPool;
use crate::domain::bookmark::{Bookmark, BookmarkSort};
use crate::domain::error::DomainError;
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{
    IngestFailure, IngestJob, MAX_SEARCH_RESULTS, SearchHit, SearchQuery, Snippet,
    websearch_constraints,
};
use std::time::Duration;
use uuid::Uuid;

/// Each leg contributes its top `LEG_DEPTH` to the fusion, and RRF scores a
/// rank `r` as `1 / (RRF_K + r)` (the measured configuration).
const LEG_DEPTH: i64 = 50;
const RRF_K: i64 = 60;

/// `ts_headline` options: one fragment of about 20 words, with matches wrapped
/// in the `SNIPPET_START`/`SNIPPET_STOP` control characters.
const HEADLINE_OPTIONS: &str = "'StartSel=' || chr(2) || ', StopSel=' || chr(3) || \
     ', MaxWords=28, MinWords=12, MaxFragments=1, FragmentDelimiter=\" … \"'";

#[derive(sqlx::FromRow)]
struct SearchRow {
    #[sqlx(flatten)]
    bookmark: Bookmark,
    snippet: Option<String>,
}

/// The positional parameters of the search statement, in bind order.
struct SearchSql {
    sql: String,
    binds_tags: bool,
    binds_constraints: bool,
}

/// Build the hybrid search statement. Each leg is one CTE that applies the
/// user and tag scope itself, so that its index stays usable:
///
/// - `lex_and` / `lex_or` / `lex`: weighted full text, the websearch (AND)
///   query, falling back to the OR of the same terms when AND finds nothing;
/// - `trg`: `pg_trgm` word similarity over title + description, which
///   catches prefixes and typos.
///
/// `fused` combines the legs with Reciprocal Rank Fusion and keeps the top
/// `MAX_SEARCH_RESULTS`. A further leg (the vector leg) is one more CTE that
/// returns `(bookmark_id, r)`, and one more arm in `legs`. Quoted phrases and
/// `-exclusions` filter the fused union, so no leg can bring back a bookmark
/// the query excludes.
fn search_sql(has_tags: bool, has_constraints: bool, sort: BookmarkSort) -> SearchSql {
    // $1 user, $2 query text, then the optional tags and constraints, then
    // limit and offset.
    let mut next = 3;
    let mut param = || {
        let p = format!("${next}");
        next += 1;
        p
    };
    let (scope_join, scope_filter) = if has_tags {
        let tags = param();
        (
            " JOIN bookmarks tb ON tb.id = c.bookmark_id".to_string(),
            format!(" AND tb.tags && {tags}"),
        )
    } else {
        (String::new(), String::new())
    };
    let constraint_filter = if has_constraints {
        let constraints = param();
        format!(
            " WHERE EXISTS (SELECT 1 FROM bookmark_contents fc, \
                  websearch_to_tsquery('english', {constraints}) cq \
              WHERE fc.bookmark_id = u.bookmark_id AND (numnode(cq) = 0 OR fc.search @@ cq))"
        )
    } else {
        String::new()
    };
    let limit = param();
    let offset = param();

    let order = match sort {
        BookmarkSort::Relevance => "f.score DESC, b.id",
        BookmarkSort::Newest => "b.created_at DESC, b.id",
        BookmarkSort::Oldest => "b.created_at ASC, b.id",
        BookmarkSort::Title => "b.title ASC NULLS LAST, f.score DESC, b.id",
        BookmarkSort::Domain => "b.domain ASC NULLS LAST, f.score DESC, b.id",
    };

    let legs = [
        "SELECT bookmark_id, r, true AS lexical FROM lex",
        "SELECT bookmark_id, r, false AS lexical FROM trg",
    ]
    .join(" UNION ALL ");

    let sql = format!(
        "WITH q AS (
             SELECT websearch_to_tsquery('english', $2) AS and_q,
                    to_tsquery('english',
                        replace(plainto_tsquery('english', $2)::text, ' & ', ' | ')) AS or_q
         ),
         lex_and AS (
             SELECT c.bookmark_id, ts_rank(c.search, q.and_q, 1) AS rank
             FROM bookmark_contents c CROSS JOIN q{scope_join}
             WHERE c.user_id = $1 AND c.search @@ q.and_q{scope_filter}
             ORDER BY rank DESC, c.bookmark_id
             LIMIT {LEG_DEPTH}
         ),
         lex_or AS (
             SELECT c.bookmark_id, ts_rank(c.search, q.or_q, 1) AS rank
             FROM bookmark_contents c CROSS JOIN q{scope_join}
             WHERE NOT EXISTS (SELECT 1 FROM lex_and)
               AND c.user_id = $1 AND c.search @@ q.or_q{scope_filter}
             ORDER BY rank DESC, c.bookmark_id
             LIMIT {LEG_DEPTH}
         ),
         lex AS (
             SELECT bookmark_id, row_number() OVER (ORDER BY rank DESC, bookmark_id) AS r
             FROM (SELECT * FROM lex_and UNION ALL SELECT * FROM lex_or) l
         ),
         lex_q AS (
             SELECT CASE WHEN EXISTS (SELECT 1 FROM lex_and) THEN and_q ELSE or_q END AS q
             FROM q
         ),
         trg AS (
             SELECT c.bookmark_id,
                    row_number() OVER (ORDER BY word_similarity($2, c.meta_text) DESC,
                                                c.bookmark_id) AS r
             FROM bookmark_contents c{scope_join}
             WHERE c.user_id = $1 AND $2 <% c.meta_text{scope_filter}
             ORDER BY word_similarity($2, c.meta_text) DESC, c.bookmark_id
             LIMIT {LEG_DEPTH}
         ),
         fused AS (
             SELECT u.bookmark_id, sum(1.0 / ({RRF_K} + u.r)) AS score,
                    bool_or(u.lexical) AS lexical
             FROM ({legs}) u{constraint_filter}
             GROUP BY u.bookmark_id
             ORDER BY score DESC, u.bookmark_id
             LIMIT {MAX_SEARCH_RESULTS}
         ),
         page AS (
             SELECT f.bookmark_id, f.lexical, row_number() OVER (ORDER BY {order}) AS pos
             FROM fused f JOIN bookmarks b ON b.id = f.bookmark_id
             ORDER BY pos
             LIMIT {limit} OFFSET {offset}
         )
         SELECT b.id, b.user_id, b.url, b.title, b.description, b.image_url,
                b.override_image_url, b.domain, b.tags, b.created_at, b.updated_at,
                CASE WHEN p.lexical AND c.status = 'fetched' AND c.text IS NOT NULL
                     THEN ts_headline('english', c.text, (SELECT q FROM lex_q), {HEADLINE_OPTIONS})
                END AS snippet
         FROM page p
         JOIN bookmarks b ON b.id = p.bookmark_id
         JOIN bookmark_contents c ON c.bookmark_id = p.bookmark_id
         ORDER BY p.pos"
    );
    SearchSql {
        sql,
        binds_tags: has_tags,
        binds_constraints: has_constraints,
    }
}

impl SearchRepository for PostgresPool {
    async fn search(
        &self,
        user_id: Uuid,
        query: &SearchQuery,
    ) -> Result<Vec<SearchHit>, DomainError> {
        let constraints = websearch_constraints(&query.text);
        let statement = search_sql(query.tags.is_some(), constraints.is_some(), query.sort);
        let mut q = sqlx::query_as::<_, SearchRow>(&statement.sql)
            .bind(user_id)
            .bind(&query.text);
        if statement.binds_tags {
            q = q.bind(&query.tags);
        }
        if statement.binds_constraints {
            q = q.bind(&constraints);
        }
        let rows = q
            .bind(query.limit)
            .bind(query.offset)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| SearchHit {
                bookmark: row.bookmark,
                snippet: row.snippet.as_deref().and_then(Snippet::from_marked),
            })
            .collect())
    }

    async fn claim_ingest_batch(
        &self,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<IngestJob>, DomainError> {
        sqlx::query_as::<_, IngestJob>(
            "UPDATE bookmark_contents c
             SET attempts = c.attempts + 1,
                 next_attempt_at = now() + make_interval(secs => $2),
                 updated_at = now()
             FROM bookmarks b
             WHERE b.id = c.bookmark_id
               AND c.bookmark_id IN (
                   SELECT bookmark_id FROM bookmark_contents
                   WHERE status = 'pending' AND next_attempt_at <= now()
                   ORDER BY next_attempt_at, bookmark_id
                   LIMIT $1
                   FOR UPDATE SKIP LOCKED)
             RETURNING c.bookmark_id, b.url, c.attempts",
        )
        .bind(limit)
        .bind(lease.as_secs_f64())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn store_article(
        &self,
        job: &IngestJob,
        text: &str,
        content_hash: &str,
    ) -> Result<bool, DomainError> {
        let result = sqlx::query(
            "UPDATE bookmark_contents c
             SET status = 'fetched', error = NULL, text = $3, text_chars = char_length($3),
                 content_hash = $4,
                 search = bookmark_search_document(b.title, b.description, b.url, b.tags, $3),
                 fetched_at = now(), updated_at = now()
             FROM bookmarks b
             WHERE c.bookmark_id = $1 AND b.id = c.bookmark_id AND b.url = $2
               AND c.status = 'pending'",
        )
        .bind(job.bookmark_id)
        .bind(&job.url)
        .bind(text)
        .bind(content_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    async fn record_ingest_failure(
        &self,
        job: &IngestJob,
        failure: &IngestFailure,
    ) -> Result<bool, DomainError> {
        let result = sqlx::query(
            "UPDATE bookmark_contents c
             SET status = $3, error = $4, next_attempt_at = coalesce($5, now()), updated_at = now()
             FROM bookmarks b
             WHERE c.bookmark_id = $1 AND b.id = c.bookmark_id AND b.url = $2
               AND c.status = 'pending'",
        )
        .bind(job.bookmark_id)
        .bind(&job.url)
        .bind(failure.status.as_str())
        .bind(&failure.error)
        .bind(failure.retry_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::domain::bookmark::CreateBookmark;
    use crate::domain::ports::bookmark_repo::BookmarkRepository;
    use crate::domain::search::IngestStatus;
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    async fn pool() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for the ignored Postgres search test");
        PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must point at a migrated database")
    }

    async fn user(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, 'search test')")
            .bind(id)
            .bind(format!("search-{id}@example.test"))
            .execute(pool)
            .await
            .expect("insert test user");
        id
    }

    async fn bookmark(
        repo: &PostgresPool,
        user_id: Uuid,
        url: &str,
        title: &str,
        description: Option<&str>,
        tags: &[&str],
    ) -> Bookmark {
        repo.create(
            user_id,
            CreateBookmark {
                url: url.to_string(),
                title: Some(title.to_string()),
                description: description.map(str::to_string),
                image_url: None,
                domain: None,
                tags: Some(tags.iter().map(|t| t.to_string()).collect()),
            },
        )
        .await
        .expect("create bookmark")
    }

    fn job(b: &Bookmark) -> IngestJob {
        IngestJob {
            bookmark_id: b.id,
            url: b.url.clone(),
            attempts: 1,
        }
    }

    async fn store(repo: &PostgresPool, b: &Bookmark, text: &str) {
        assert!(repo.store_article(&job(b), text, "hash").await.unwrap());
    }

    async fn search(repo: &PostgresPool, user_id: Uuid, text: &str) -> Vec<SearchHit> {
        search_with(repo, user_id, text, None, BookmarkSort::Relevance).await
    }

    async fn search_with(
        repo: &PostgresPool,
        user_id: Uuid,
        text: &str,
        tags: Option<&[&str]>,
        sort: BookmarkSort,
    ) -> Vec<SearchHit> {
        repo.search(
            user_id,
            &SearchQuery {
                text: text.to_string(),
                tags: tags.map(|t| t.iter().map(|s| s.to_string()).collect()),
                sort,
                limit: MAX_SEARCH_RESULTS,
                offset: 0,
            },
        )
        .await
        .expect("search")
    }

    fn ids(hits: &[SearchHit]) -> Vec<Uuid> {
        hits.iter().map(|h| h.bookmark.id).collect()
    }

    async fn status(pool: &PgPool, id: Uuid) -> Option<(String, Option<String>)> {
        sqlx::query_as("SELECT status, text FROM bookmark_contents WHERE bookmark_id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    /// Run with a migrated database:
    /// `DATABASE_URL=... cargo test -p boopmark-server search_tests -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn hybrid_search_ranks_falls_back_filters_and_isolates_users() {
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let alice = user(&pool).await;
        let bob = user(&pool).await;

        let book = bookmark(
            &repo,
            alice,
            "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html",
            "Understanding Ownership",
            Some("How the borrow checker keeps references valid"),
            &["rust"],
        )
        .await;
        let runtime = bookmark(
            &repo,
            alice,
            "https://tokio.rs/tokio/tutorial",
            "Tokio tutorial",
            Some("Writing async applications"),
            &["rust", "async"],
        )
        .await;
        let garden = bookmark(
            &repo,
            alice,
            "https://example.org/compost",
            "Composting at home",
            Some("Turn kitchen scraps into soil"),
            &["garden"],
        )
        .await;
        let cafe = bookmark(
            &repo,
            alice,
            "https://example.org/brewing",
            "Brewing guide",
            Some("Pour-over coffee and loose-leaf tea at home"),
            &[],
        )
        .await;
        let espresso = bookmark(
            &repo,
            alice,
            "https://example.org/espresso",
            "Espresso basics",
            Some("Dialing in coffee grind size"),
            &[],
        )
        .await;
        // A failed fetch: only the metadata is indexed.
        let failed = bookmark(
            &repo,
            alice,
            "https://news.example.com/microsoft-github",
            "Microsoft to acquire GitHub for $7.5 billion",
            None,
            &[],
        )
        .await;
        let bobs = bookmark(
            &repo,
            bob,
            "https://example.com/bob-ownership",
            "Ownership and borrowing, Bob's notes",
            None,
            &["rust"],
        )
        .await;

        store(
            &repo,
            &book,
            "Ownership is Rust's most unique feature. The borrow checker enforces the rules.",
        )
        .await;
        store(
            &repo,
            &runtime,
            "A runtime schedules tasks. Holding a mutex across an await point needs care with \
             ownership of the guard. <img src=x onerror=alert(1)> Spawned tasks must be 'static.",
        )
        .await;
        store(
            &repo,
            &garden,
            "Mix greens and browns. Turning the pile adds oxygen and speeds decomposition.",
        )
        .await;
        store(
            &repo,
            &cafe,
            "Rinse the filter, bloom the grounds, then pour slowly. Steep the tea for three minutes.",
        )
        .await;
        store(
            &repo,
            &espresso,
            "Grind finer when the shot runs fast. Coffee extraction depends on grind size.",
        )
        .await;
        assert!(
            repo.record_ingest_failure(
                &job(&failed),
                &IngestFailure {
                    status: IngestStatus::Failed,
                    error: "HTTP 403".into(),
                    retry_at: None,
                },
            )
            .await
            .unwrap()
        );

        // Title and description (weights A, B) outrank the body (D).
        let hits = search(&repo, alice, "ownership").await;
        assert_eq!(ids(&hits)[..2], [book.id, runtime.id]);
        assert!(!ids(&hits).contains(&bobs.id), "user isolation");

        // Body-only match, with a snippet that highlights it and keeps page
        // markup as plain text for the template to escape.
        let hits = search(&repo, alice, "mutex await").await;
        assert_eq!(ids(&hits), [runtime.id]);
        let snippet = hits[0].snippet.as_ref().expect("lexical hit has a snippet");
        assert!(
            snippet
                .segments
                .iter()
                .any(|s| s.highlighted && s.text == "mutex")
        );
        let hits = search(&repo, alice, "onerror").await;
        assert!(
            hits[0]
                .snippet
                .as_ref()
                .unwrap()
                .plain_text()
                .contains("<img src=x onerror")
        );

        // No bookmark holds every term: AND finds nothing, OR still does.
        let hits = search(&repo, alice, "oxygen decomposition borrow checker").await;
        assert_eq!(ids(&hits).len(), 2);
        assert!(ids(&hits).contains(&garden.id) && ids(&hits).contains(&book.id));

        // Typo and prefix queries reach the trigram leg over title + description.
        assert_eq!(ids(&search(&repo, alice, "Microsft").await), [failed.id]);
        assert_eq!(ids(&search(&repo, alice, "Composti").await), [garden.id]);
        // A failed fetch is found by its metadata, without a snippet.
        let hits = search(&repo, alice, "github acquisition").await;
        assert_eq!(ids(&hits), [failed.id]);
        assert!(hits[0].snippet.is_none());

        // Exclusions and phrases constrain every leg.
        let hits = search(&repo, alice, "coffee").await;
        assert!(ids(&hits).contains(&cafe.id) && ids(&hits).contains(&espresso.id));
        assert_eq!(
            ids(&search(&repo, alice, "coffee -tea").await),
            [espresso.id]
        );
        assert_eq!(
            ids(&search(&repo, alice, "coffee \"grind size\"").await),
            [espresso.id]
        );

        // Tag filter.
        let hits = search_with(
            &repo,
            alice,
            "ownership",
            Some(&["async"]),
            BookmarkSort::Relevance,
        )
        .await;
        assert_eq!(ids(&hits), [runtime.id]);

        // Another sort re-orders the fused set without widening it.
        let hits = search_with(&repo, alice, "ownership", None, BookmarkSort::Title).await;
        assert_eq!(ids(&hits), [runtime.id, book.id]);

        // Paging stays inside the fused set.
        let page = repo
            .search(
                alice,
                &SearchQuery {
                    text: "ownership".into(),
                    tags: None,
                    sort: BookmarkSort::Relevance,
                    limit: 1,
                    offset: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(ids(&page), [runtime.id]);

        // Editing the title rebuilds the document around the stored body.
        repo.update(
            garden.id,
            alice,
            crate::domain::bookmark::UpdateBookmark {
                title: Some("Hot composting".into()),
                description: None,
                tags: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            ids(&search(&repo, alice, "hot composting").await)[0],
            garden.id
        );
        assert_eq!(
            ids(&search(&repo, alice, "decomposition").await),
            [garden.id]
        );

        // A URL change re-queues the bookmark and drops the stale body; a
        // result for the old URL is not stored.
        let mut moved = garden.clone();
        moved.url = "https://example.org/compost-v2".into();
        moved.title = Some("Hot composting".into());
        repo.upsert_full(moved).await.unwrap();
        assert_eq!(
            status(&pool, garden.id).await,
            Some(("pending".to_string(), None))
        );
        assert!(
            !repo
                .store_article(&job(&garden), "stale", "h")
                .await
                .unwrap()
        );
        assert!(search(&repo, alice, "decomposition").await.is_empty());

        // Deleting the bookmark removes its contents row.
        repo.delete(cafe.id, alice).await.unwrap();
        assert_eq!(status(&pool, cafe.id).await, None);

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(alice)
            .bind(bob)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn concurrent_claims_never_share_a_job_and_leases_hide_claimed_rows() {
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        let mut mine = Vec::new();
        for i in 0..6 {
            let b = bookmark(
                &repo,
                owner,
                &format!("https://example.net/claim-{i}"),
                "claim",
                None,
                &[],
            )
            .await;
            mine.push(b.id);
        }
        // Put this test's rows at the head of the queue, ahead of any other
        // pending rows in a shared database.
        sqlx::query(
            "UPDATE bookmark_contents SET next_attempt_at = '1970-01-01' WHERE user_id = $1",
        )
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();

        let lease = Duration::from_secs(600);
        let (a, b) = tokio::join!(
            repo.claim_ingest_batch(3, lease),
            repo.claim_ingest_batch(3, lease)
        );
        let mut claimed: Vec<Uuid> = a
            .unwrap()
            .into_iter()
            .chain(b.unwrap())
            .map(|j| j.bookmark_id)
            .collect();
        claimed.sort();
        let mut expected = mine.clone();
        expected.sort();
        assert_eq!(claimed, expected, "each row claimed exactly once");

        // Leased rows are not handed out again, and the attempt was counted.
        let again = repo.claim_ingest_batch(100, lease).await.unwrap();
        assert!(again.iter().all(|j| !mine.contains(&j.bookmark_id)));
        let attempts: Vec<i32> =
            sqlx::query_scalar("SELECT attempts FROM bookmark_contents WHERE user_id = $1")
                .bind(owner)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(attempts.iter().all(|a| *a == 1));
        // Release whatever the sweep above claimed from other tests' rows.
        for j in again {
            sqlx::query(
                "UPDATE bookmark_contents SET next_attempt_at = now(), attempts = attempts - 1
                 WHERE bookmark_id = $1",
            )
            .bind(j.bookmark_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
