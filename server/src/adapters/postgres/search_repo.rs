use super::PostgresPool;
use crate::domain::bookmark::{Bookmark, BookmarkSort};
use crate::domain::error::DomainError;
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{
    IngestFailure, IngestJob, MAX_INGEST_ATTEMPTS, MAX_SEARCH_RESULTS, SearchHit, SearchQuery,
    Snippet, websearch_constraints,
};
use std::time::Duration;
use uuid::Uuid;

/// Each leg contributes its top `LEG_DEPTH` to the fusion, and RRF scores a
/// rank `r` as `1 / (RRF_K + r)` (the measured configuration).
const LEG_DEPTH: i64 = 50;
const RRF_K: i64 = 60;
/// A claim considers the next rows of at most this many users (those
/// waiting longest) when it picks its jobs round-robin by user and host.
const MAX_CLAIM_USERS: i64 = 100;
/// The start of an ingest result write ($1 bookmark, $2 claimed URL): it
/// share-locks the bookmark, so its URL cannot change until the write
/// commits, and matches nothing once the bookmark was deleted or moved. The
/// write then also requires the claim token ($3), which a later claim
/// replaces.
const CLAIMED_BOOKMARK: &str = "WITH b AS (
    SELECT id, title, description, url, tags FROM bookmarks
    WHERE id = $1 AND url = $2
    FOR SHARE
)";

/// `ts_headline` options: one fragment of about 20 words, with matches wrapped
/// in the `SNIPPET_START`/`SNIPPET_STOP` control characters.
const HEADLINE_OPTIONS: &str = "'StartSel=' || chr(2) || ', StopSel=' || chr(3) || \
     ', MaxWords=28, MinWords=12, MaxFragments=1, FragmentDelimiter=\" … \"'";
/// `ts_headline` re-parses the text it is given, so it sees only this much of
/// each article (the measured maximum was 99k characters). A match further in
/// gets no snippet.
const HEADLINE_SOURCE_CHARS: usize = 100_000;

#[derive(sqlx::FromRow)]
struct SearchRow {
    #[sqlx(flatten)]
    bookmark: Bookmark,
    snippet: Option<String>,
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
/// returns `(bookmark_id, r)`, and one more arm in `legs`.
///
/// Quoted phrases and `-exclusions` (`cq`) filter inside each lexical leg,
/// so excluded bookmarks do not use up a leg's top 50, and again on the
/// fused union, so no leg can bring back a bookmark the query excludes.
fn search_sql(has_tags: bool, has_constraints: bool, sort: BookmarkSort) -> String {
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
    let (constraint_column, leg_constraint, fused_constraint) = if has_constraints {
        let constraints = param();
        (
            format!(",\n                    websearch_to_tsquery('english', {constraints}) AS cq"),
            " AND (numnode(q.cq) = 0 OR c.search @@ q.cq)",
            " WHERE EXISTS (SELECT 1 FROM bookmark_contents fc CROSS JOIN q
                     WHERE fc.bookmark_id = u.bookmark_id
                       AND (numnode(q.cq) = 0 OR fc.search @@ q.cq))",
        )
    } else {
        (String::new(), "", "")
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

    format!(
        "WITH q AS (
             SELECT websearch_to_tsquery('english', $2) AS and_q,
                    to_tsquery('english',
                        replace(plainto_tsquery('english', $2)::text, ' & ', ' | ')) AS or_q{constraint_column}
         ),
         lex_and AS (
             SELECT c.bookmark_id, ts_rank(c.search, q.and_q, 1) AS rank
             FROM bookmark_contents c CROSS JOIN q{scope_join}
             WHERE c.user_id = $1 AND c.search @@ q.and_q{scope_filter}{leg_constraint}
             ORDER BY rank DESC, c.bookmark_id
             LIMIT {LEG_DEPTH}
         ),
         lex_or AS (
             SELECT c.bookmark_id, ts_rank(c.search, q.or_q, 1) AS rank
             FROM bookmark_contents c CROSS JOIN q{scope_join}
             WHERE NOT EXISTS (SELECT 1 FROM lex_and)
               AND c.user_id = $1 AND c.search @@ q.or_q{scope_filter}{leg_constraint}
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
             SELECT bookmark_id, row_number() OVER (ORDER BY sim DESC, bookmark_id) AS r
             FROM (
                 SELECT c.bookmark_id, word_similarity($2, c.meta_text) AS sim
                 FROM bookmark_contents c CROSS JOIN q{scope_join}
                 WHERE c.user_id = $1 AND $2 <% c.meta_text{scope_filter}{leg_constraint}
                 ORDER BY sim DESC, c.bookmark_id
                 LIMIT {LEG_DEPTH}
             ) t
         ),
         fused AS (
             SELECT u.bookmark_id, sum(1.0 / ({RRF_K} + u.r)) AS score,
                    bool_or(u.lexical) AS lexical
             FROM ({legs}) u{fused_constraint}
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
                     THEN ts_headline('english', left(c.text, {HEADLINE_SOURCE_CHARS}),
                                      (SELECT q FROM lex_q), {HEADLINE_OPTIONS})
                END AS snippet
         FROM page p
         JOIN bookmarks b ON b.id = p.bookmark_id
         JOIN bookmark_contents c ON c.bookmark_id = p.bookmark_id
         ORDER BY p.pos"
    )
}

impl SearchRepository for PostgresPool {
    async fn search(
        &self,
        user_id: Uuid,
        query: &SearchQuery,
    ) -> Result<Vec<SearchHit>, DomainError> {
        let constraints = websearch_constraints(&query.text);
        let sql = search_sql(query.tags.is_some(), constraints.is_some(), query.sort);
        let mut q = sqlx::query_as::<_, SearchRow>(&sql)
            .bind(user_id)
            .bind(&query.text);
        if query.tags.is_some() {
            q = q.bind(&query.tags);
        }
        if constraints.is_some() {
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
        // `swept` gives up on rows claimed MAX_INGEST_ATTEMPTS times without a
        // result (leases that ran out, results that could never be stored).
        // `waiting` ranks the users with due rows, the one whose turn came
        // longest ago first (`bookmark_ingest_turns`), so fairness holds
        // across claims, even when each claim takes a single job. `due` locks
        // each waiting user's share of the batch, so however big one import
        // is, every other waiting user is a candidate. `picked` takes one row
        // per user in rank order, then a second, and so on; within a user,
        // rows rotate through hosts. `turns` records the users served. Every
        // claim gets a fresh token, so a worker whose lease ran out can no
        // longer write.
        let sql = format!(
            "WITH swept AS (
                 UPDATE bookmark_contents
                 SET status = 'failed', claim_token = NULL, updated_at = now(),
                     error = 'gave up after ' || attempts || ' attempts'
                 WHERE status = 'pending' AND attempts >= $3 AND next_attempt_at <= now()
                 RETURNING bookmark_id
             ),
             waiting AS (
                 SELECT q.user_id,
                        row_number() OVER (ORDER BY t.last_claimed_at NULLS FIRST,
                                                    q.oldest, q.user_id) AS user_rank
                 FROM (
                     SELECT user_id, min(next_attempt_at) AS oldest FROM bookmark_contents
                     WHERE status = 'pending' AND next_attempt_at <= now() AND attempts < $3
                     GROUP BY user_id
                 ) q
                 LEFT JOIN bookmark_ingest_turns t ON t.user_id = q.user_id
                 ORDER BY user_rank
                 LIMIT {MAX_CLAIM_USERS}
             ),
             due AS (
                 SELECT w.user_rank, d.* FROM waiting w CROSS JOIN LATERAL (
                     SELECT c.bookmark_id, c.user_id, c.next_attempt_at,
                            lower(substring(b.url from '^[^:]+://(?:[^@/]*@)?([^/:?#]+)')) AS host
                     FROM bookmark_contents c JOIN bookmarks b ON b.id = c.bookmark_id
                     WHERE c.user_id = w.user_id AND c.status = 'pending'
                       AND c.next_attempt_at <= now() AND c.attempts < $3
                     ORDER BY c.next_attempt_at, c.bookmark_id
                     -- Each user's share of the batch.
                     LIMIT ceil($1::numeric / (SELECT count(*) FROM waiting))::bigint
                     FOR UPDATE OF c SKIP LOCKED
                 ) d
             ),
             picked AS (
                 SELECT bookmark_id FROM (
                     SELECT bookmark_id, user_rank,
                            row_number() OVER (PARTITION BY user_id
                                               ORDER BY host_turn, next_attempt_at, bookmark_id)
                                AS user_turn
                     FROM (
                         SELECT due.*,
                                row_number() OVER (PARTITION BY user_id, host
                                                   ORDER BY next_attempt_at, bookmark_id)
                                    AS host_turn
                         FROM due
                     ) h
                 ) u
                 ORDER BY user_turn, user_rank
                 LIMIT $1
             ),
             claimed AS (
                 UPDATE bookmark_contents c
                 SET attempts = c.attempts + 1,
                     next_attempt_at = now() + make_interval(secs => $2),
                     claim_token = gen_random_uuid(),
                     updated_at = now()
                 FROM bookmarks b
                 WHERE b.id = c.bookmark_id AND c.bookmark_id IN (SELECT bookmark_id FROM picked)
                 RETURNING c.bookmark_id, c.user_id, b.url, c.attempts, c.claim_token
             ),
             turns AS (
                 INSERT INTO bookmark_ingest_turns (user_id, last_claimed_at)
                 SELECT user_id, clock_timestamp() FROM (SELECT DISTINCT user_id FROM claimed) u
                 ORDER BY user_id
                 ON CONFLICT (user_id) DO UPDATE SET last_claimed_at = EXCLUDED.last_claimed_at
             )
             SELECT bookmark_id, url, attempts, claim_token FROM claimed"
        );
        sqlx::query_as::<_, IngestJob>(&sql)
            .bind(limit)
            .bind(lease.as_secs_f64())
            .bind(MAX_INGEST_ATTEMPTS)
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
        let result = sqlx::query(&format!(
            "{CLAIMED_BOOKMARK}
             UPDATE bookmark_contents c
             SET status = 'fetched', error = NULL, claim_token = NULL,
                 text = $4, text_chars = char_length($4), content_hash = $5,
                 search = bookmark_search_document(b.title, b.description, b.url, b.tags, $4),
                 fetched_at = now(), updated_at = now()
             FROM b
             WHERE c.bookmark_id = b.id AND c.claim_token = $3 AND c.status = 'pending'"
        ))
        .bind(job.bookmark_id)
        .bind(&job.url)
        .bind(job.claim_token)
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
        let result = sqlx::query(&format!(
            "{CLAIMED_BOOKMARK}
             UPDATE bookmark_contents c
             SET status = $4, error = $5, next_attempt_at = coalesce($6, now()),
                 claim_token = NULL, updated_at = now()
             FROM b
             WHERE c.bookmark_id = b.id AND c.claim_token = $3 AND c.status = 'pending'"
        ))
        .bind(job.bookmark_id)
        .bind(&job.url)
        .bind(job.claim_token)
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
    use std::collections::HashMap;

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

    /// The tests claim from the shared queue, so they must not interleave.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    const LEASE: Duration = Duration::from_secs(600);

    /// Release claims that belong to rows outside these tests, as if they
    /// had never been claimed.
    async fn release_others(pool: &PgPool, jobs: &[IngestJob], mine: &[Uuid]) {
        let others: Vec<Uuid> = jobs
            .iter()
            .map(|j| j.bookmark_id)
            .filter(|id| !mine.contains(id))
            .collect();
        sqlx::query(
            "UPDATE bookmark_contents
             SET next_attempt_at = now(), attempts = attempts - 1, claim_token = NULL
             WHERE bookmark_id = ANY($1)",
        )
        .bind(&others)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Move these users' pending rows to the head of the queue (ahead of
    /// anything else in a shared database), claim them, and release whatever
    /// else the claim took.
    async fn claim_mine(
        pool: &PgPool,
        repo: &PostgresPool,
        users: &[Uuid],
    ) -> HashMap<Uuid, IngestJob> {
        let mine: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE bookmark_contents SET next_attempt_at = '1970-01-01'
             WHERE user_id = ANY($1) AND status = 'pending' RETURNING bookmark_id",
        )
        .bind(users)
        .fetch_all(pool)
        .await
        .unwrap();
        let jobs = repo.claim_ingest_batch(1000, LEASE).await.unwrap();
        release_others(pool, &jobs, &mine).await;
        jobs.into_iter()
            .filter(|j| mine.contains(&j.bookmark_id))
            .map(|j| (j.bookmark_id, j))
            .collect()
    }

    async fn store(repo: &PostgresPool, job: &IngestJob, text: &str) {
        assert!(repo.store_article(job, text, "hash").await.unwrap());
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
    /// `DATABASE_URL=... cargo test -p boopmark-server adapters::postgres::search_repo::search_tests -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn hybrid_search_ranks_falls_back_filters_and_isolates_users() {
        let _serial = SERIAL.lock().await;
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

        let jobs = claim_mine(&pool, &repo, &[alice]).await;
        store(
            &repo,
            &jobs[&book.id],
            "Ownership is Rust's most unique feature. The borrow checker enforces the rules.",
        )
        .await;
        store(
            &repo,
            &jobs[&runtime.id],
            "A runtime schedules tasks. Holding a mutex across an await point needs care with \
             ownership of the guard. <img src=x onerror=alert(1)> Spawned tasks must be 'static.",
        )
        .await;
        store(
            &repo,
            &jobs[&garden.id],
            "Mix greens and browns. Turning the pile adds oxygen and speeds decomposition.",
        )
        .await;
        store(
            &repo,
            &jobs[&cafe.id],
            "Rinse the filter, bloom the grounds, then pour slowly. Steep the tea for three minutes.",
        )
        .await;
        store(
            &repo,
            &jobs[&espresso.id],
            "Grind finer when the shot runs fast. Coffee extraction depends on grind size.",
        )
        .await;
        assert!(
            repo.record_ingest_failure(
                &jobs[&failed.id],
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
        // `or` between phrases keeps its meaning in the filter.
        let either = ids(&search(&repo, alice, "\"grind size\" or \"loose-leaf tea\"").await);
        assert!(either.contains(&espresso.id) && either.contains(&cafe.id));

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
                .store_article(&jobs[&garden.id], "stale", "h")
                .await
                .unwrap()
        );
        assert!(search(&repo, alice, "decomposition").await.is_empty());

        // A blank search box lists the whole library, newest first.
        let blank = BookmarkRepository::list(
            &repo,
            alice,
            crate::domain::bookmark::BookmarkFilter {
                search: Some("   ".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(blank.len(), 6, "every one of alice's bookmarks");

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
    async fn claims_are_exclusive_fair_tokened_and_bounded() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let importer = user(&pool).await;
        let other = user(&pool).await;
        let mut big_import = Vec::new();
        for i in 0..6 {
            let b = bookmark(
                &repo,
                importer,
                &format!("https://import.example.net/claim-{i}"),
                "claim",
                None,
                &[],
            )
            .await;
            big_import.push(b.id);
        }
        let single = bookmark(
            &repo,
            other,
            "https://elsewhere.example.org/one",
            "claim",
            None,
            &[],
        )
        .await;
        let mut mine = big_import.clone();
        mine.push(single.id);
        let to_head = |ids: Vec<Uuid>, at: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "UPDATE bookmark_contents SET next_attempt_at = $2::timestamptz
                     WHERE bookmark_id = ANY($1)",
                )
                .bind(ids)
                .bind(at)
                .execute(&pool)
                .await
                .unwrap();
            }
        };

        // Fairness: the importer has a backlog of hundreds of older rows, but a
        // claim of two takes one row from each user.
        let mut backlog = Vec::new();
        for i in 0..300 {
            let b = bookmark(
                &repo,
                importer,
                &format!("https://import.example.net/backlog-{i}"),
                "backlog",
                None,
                &[],
            )
            .await;
            backlog.push(b.id);
        }
        to_head(backlog.clone(), "1970-01-01").await;
        to_head(big_import.clone(), "1970-01-01").await;
        to_head(vec![single.id], "1970-01-02").await;
        let two = repo.claim_ingest_batch(2, LEASE).await.unwrap();
        assert_eq!(two.len(), 2);
        assert!(two.iter().any(|j| j.bookmark_id == single.id));
        assert!(
            two.iter()
                .any(|j| big_import.contains(&j.bookmark_id) || backlog.contains(&j.bookmark_id))
        );
        release_others(&pool, &two, &[]).await;
        sqlx::query("DELETE FROM bookmarks WHERE id = ANY($1)")
            .bind(&backlog)
            .execute(&pool)
            .await
            .unwrap();

        // Fairness holds across claims: single-job claims alternate between the
        // two users even though every importer row is older.
        to_head(big_import.clone(), "1970-01-01").await;
        to_head(vec![single.id], "1970-01-02").await;
        let second = bookmark(
            &repo,
            other,
            "https://elsewhere.example.org/two",
            "claim",
            None,
            &[],
        )
        .await;
        to_head(vec![second.id], "1970-01-02").await;
        let mut served = Vec::new();
        for _ in 0..50 {
            if served.len() == 4 {
                break;
            }
            let one = repo.claim_ingest_batch(1, LEASE).await.unwrap();
            for job in &one {
                if big_import.contains(&job.bookmark_id) {
                    served.push(importer);
                } else if [single.id, second.id].contains(&job.bookmark_id) {
                    served.push(other);
                }
            }
            // Rows of other tests are put back; this test's stay leased.
            let mine_now: Vec<Uuid> = big_import
                .iter()
                .copied()
                .chain([single.id, second.id])
                .collect();
            release_others(&pool, &one, &mine_now).await;
        }
        assert_eq!(served.len(), 4);
        assert!(
            served.windows(2).all(|w| w[0] != w[1]),
            "claims alternate between users: {served:?}"
        );
        sqlx::query("DELETE FROM bookmarks WHERE id = $1")
            .bind(second.id)
            .execute(&pool)
            .await
            .unwrap();
        // Put this step's claims back for the steps below.
        sqlx::query(
            "UPDATE bookmark_contents SET next_attempt_at = now(), attempts = 0, claim_token = NULL
             WHERE user_id = ANY($1)",
        )
        .bind([importer, other])
        .execute(&pool)
        .await
        .unwrap();

        // Two concurrent claimers never share a row.
        to_head(mine.clone(), "1970-01-01").await;
        let (a, b) = tokio::join!(
            repo.claim_ingest_batch(100, LEASE),
            repo.claim_ingest_batch(100, LEASE)
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(
            a.iter()
                .all(|j| b.iter().all(|k| k.bookmark_id != j.bookmark_id))
        );
        let all: Vec<IngestJob> = a.into_iter().chain(b).collect();
        let mut claimed: Vec<Uuid> = all
            .iter()
            .map(|j| j.bookmark_id)
            .filter(|id| mine.contains(id))
            .collect();
        claimed.sort();
        let mut sorted_mine = mine.clone();
        sorted_mine.sort();
        assert_eq!(claimed, sorted_mine, "each row claimed exactly once");
        release_others(&pool, &all, &mine).await;

        // Leased rows are not handed out again.
        let again = repo.claim_ingest_batch(1000, LEASE).await.unwrap();
        assert!(again.iter().all(|j| !mine.contains(&j.bookmark_id)));
        release_others(&pool, &again, &mine).await;

        // After the lease runs out, a new claim takes the row over and the
        // first worker can no longer write to it.
        let stale = all.iter().find(|j| j.bookmark_id == single.id).unwrap();
        to_head(vec![single.id], "1970-01-01").await;
        let fresh = claim_mine(&pool, &repo, &[other]).await;
        let fresh = &fresh[&single.id];
        assert_ne!(fresh.claim_token, stale.claim_token);
        assert_eq!(fresh.attempts, stale.attempts + 1);
        assert!(!repo.store_article(stale, "late", "h").await.unwrap());
        assert!(repo.store_article(fresh, "on time", "h").await.unwrap());

        // A row claimed MAX_INGEST_ATTEMPTS times is given up.
        let worn = big_import[1];
        sqlx::query(
            "UPDATE bookmark_contents SET attempts = $2, next_attempt_at = '1970-01-01'
             WHERE bookmark_id = $1",
        )
        .bind(worn)
        .bind(MAX_INGEST_ATTEMPTS)
        .execute(&pool)
        .await
        .unwrap();
        let swept = repo.claim_ingest_batch(1, LEASE).await.unwrap();
        assert!(swept.iter().all(|j| j.bookmark_id != worn));
        release_others(&pool, &swept, &[]).await;
        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM bookmark_contents WHERE bookmark_id = $1")
                .bind(worn)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("gave up after 6 attempts"));

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(importer)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
