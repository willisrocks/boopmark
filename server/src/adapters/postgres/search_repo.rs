use super::PostgresPool;
use crate::domain::bookmark::{Bookmark, BookmarkSort};
use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
use crate::domain::error::DomainError;
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{
    ContentSource, IngestFailure, IngestJob, MAX_INGEST_ATTEMPTS, MAX_SEARCH_RESULTS, SearchHit,
    SearchQuery, Snippet, trigram_variants, websearch_constraints,
};
use std::time::Duration;
use uuid::Uuid;

fn db_error(e: sqlx::Error) -> DomainError {
    DomainError::Internal(e.to_string())
}

/// Each leg contributes its top `LEG_DEPTH` to the fusion, and RRF scores a
/// rank `r` as `1 / (RRF_K + r)` (the measured configuration).
const LEG_DEPTH: i64 = 50;
const RRF_K: i64 = 60;
/// The vector leg ranks bookmarks by their best chunk among this many nearest
/// chunks (about 10 a bookmark, so enough for its top `LEG_DEPTH`).
const NEAREST_CHUNKS: i64 = 200;
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
    /// The bookmark's nearest article chunk, when the vector leg found it.
    chunk_text: Option<String>,
    semantic: bool,
}

/// What `search_sql` binds besides `$1` (user) and `$2` (query text).
#[derive(Debug, Clone, Copy)]
struct SearchShape {
    has_tags: bool,
    has_constraints: bool,
    /// The query vector's size, when the vector leg runs.
    vector_dims: Option<usize>,
    sort: BookmarkSort,
}

/// Build the hybrid search statement. Each leg is one CTE that applies the
/// user and tag scope itself, so that its index stays usable:
///
/// - `lex_and` / `lex_or` / `lex`: weighted full text, the websearch (AND)
///   query. Without a vector leg it falls back to the OR of the same terms
///   when AND finds nothing (the measured lexical-only setup); with one, it
///   is AND only, as measured for the three legs.
/// - `trg`: `pg_trgm` word similarity over title + description, which
///   catches prefixes and typos.
/// - `nn` / `vec`: the nearest chunks built by the configured embedder
///   (`model` *and* size must match, so a re-embed to another model of the
///   same size never compares two vector spaces), ranked per bookmark by its
///   best chunk. The size is inlined so the matching partial HNSW index is
///   used.
///
/// `fused` combines the legs with Reciprocal Rank Fusion and keeps the top
/// `MAX_SEARCH_RESULTS`.
///
/// Quoted phrases and `-exclusions` (`cq`) filter inside each leg, so
/// excluded bookmarks do not use up a leg's top 50, and again on the fused
/// union, so no leg can bring back a bookmark the query excludes.
fn search_sql(shape: SearchShape) -> String {
    // $1 user, $2 query text, then the optional tags, constraints, query
    // vector and model, then limit and offset.
    let mut next = 3;
    let mut param = || {
        let p = format!("${next}");
        next += 1;
        p
    };
    let tags = shape.has_tags.then(&mut param);
    let constraints = shape.has_constraints.then(&mut param);
    let variants = param();
    let vector = shape.vector_dims.map(|dims| (dims, param(), param()));
    let limit = param();
    let offset = param();

    let (scope_join, scope_filter, chunk_join, chunk_filter) = match &tags {
        Some(tags) => (
            " JOIN bookmarks tb ON tb.id = c.bookmark_id",
            format!(" AND tb.tags && {tags}"),
            " JOIN bookmarks tb ON tb.id = ch.bookmark_id",
            format!(" AND tb.tags && {tags}"),
        ),
        None => ("", String::new(), "", String::new()),
    };
    let (constraint_column, leg_constraint, chunk_constraint, fused_constraint) = match &constraints
    {
        Some(constraints) => (
            format!(",\n                    websearch_to_tsquery('english', {constraints}) AS cq"),
            " AND (numnode(q.cq) = 0 OR c.search @@ q.cq)",
            " AND (numnode(q.cq) = 0 OR EXISTS (SELECT 1 FROM bookmark_contents fc
                     WHERE fc.bookmark_id = ch.bookmark_id AND fc.search @@ q.cq))",
            " WHERE EXISTS (SELECT 1 FROM bookmark_contents fc CROSS JOIN q
                     WHERE fc.bookmark_id = u.bookmark_id
                       AND (numnode(q.cq) = 0 OR fc.search @@ q.cq))",
        ),
        None => (String::new(), "", "", ""),
    };

    let order = match shape.sort {
        BookmarkSort::Relevance => "f.score DESC, b.id",
        BookmarkSort::Newest => "b.created_at DESC, b.id",
        BookmarkSort::Oldest => "b.created_at ASC, b.id",
        BookmarkSort::Title => "b.title ASC NULLS LAST, f.score DESC, b.id",
        BookmarkSort::Domain => "b.domain ASC NULLS LAST, f.score DESC, b.id",
    };

    let mut legs = vec![
        "SELECT bookmark_id, r, 'lex' AS leg FROM lex",
        "SELECT bookmark_id, r, 'trg' AS leg FROM trg",
    ];
    // Without a vector leg, OR fills in for an AND that finds nothing.
    let (lex_or_gate, vector_ctes, chunk_select, chunk_from) = match &vector {
        Some((dims, query_vector, model)) => {
            legs.push("SELECT bookmark_id, r, 'vec' AS leg FROM vec");
            let distance = format!(
                "ch.embedding::halfvec({dims}) <=> {query_vector}::real[]::halfvec({dims})"
            );
            (
                "false",
                format!(
                    ",
         nn AS (
             SELECT ch.bookmark_id, ch.ord, {distance} AS dist
             FROM bookmark_chunks ch CROSS JOIN q{chunk_join}
             WHERE ch.user_id = $1 AND ch.model = {model}
               AND vector_dims(ch.embedding) = {dims}{chunk_filter}{chunk_constraint}
             ORDER BY {distance}
             LIMIT {NEAREST_CHUNKS}
         ),
         vec AS (
             SELECT bookmark_id, best_ord, row_number() OVER (ORDER BY dist, bookmark_id) AS r
             FROM (
                 SELECT bookmark_id, min(dist) AS dist,
                        (array_agg(ord ORDER BY dist, ord))[1] AS best_ord
                 FROM nn GROUP BY bookmark_id
             ) v
             ORDER BY dist, bookmark_id
             LIMIT {LEG_DEPTH}
         )"
                ),
                // Never the metadata chunk: it says nothing the card does not.
                format!(
                    "CASE WHEN c.status = 'fetched' AND c.text_chars >= {MIN_CHUNKED_TEXT_CHARS}
                          THEN bc.text END"
                ),
                " LEFT JOIN vec v ON v.bookmark_id = p.bookmark_id
         LEFT JOIN bookmark_chunks bc ON bc.bookmark_id = v.bookmark_id AND bc.ord = v.best_ord",
            )
        }
        None => ("true", String::new(), "NULL::text".to_string(), ""),
    };
    let legs = legs.join(" UNION ALL ");

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
             WHERE {lex_or_gate} AND NOT EXISTS (SELECT 1 FROM lex_and)
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
                 SELECT c.bookmark_id,
                        (SELECT max(word_similarity(v, c.meta_text)) FROM unnest({variants}::text[]) v) AS sim
                 FROM bookmark_contents c CROSS JOIN q{scope_join}
                 WHERE c.user_id = $1 AND c.meta_text %> ANY({variants}::text[]){scope_filter}{leg_constraint}
                 ORDER BY sim DESC, c.bookmark_id
                 LIMIT {LEG_DEPTH}
             ) t
         ){vector_ctes},
         fused AS (
             SELECT u.bookmark_id, sum(1.0 / ({RRF_K} + u.r)) AS score,
                    bool_or(u.leg = 'lex') AS lexical, bool_and(u.leg = 'vec') AS semantic
             FROM ({legs}) u{fused_constraint}
             GROUP BY u.bookmark_id
             ORDER BY score DESC, u.bookmark_id
             LIMIT {MAX_SEARCH_RESULTS}
         ),
         page AS (
             SELECT f.bookmark_id, f.lexical, f.semantic, row_number() OVER (ORDER BY {order}) AS pos
             FROM fused f JOIN bookmarks b ON b.id = f.bookmark_id
             ORDER BY pos
             LIMIT {limit} OFFSET {offset}
         )
         SELECT b.id, b.user_id, b.url, b.title, b.description, b.image_url,
                b.override_image_url, b.domain, b.tags, b.created_at, b.updated_at,
                CASE WHEN p.lexical AND c.status = 'fetched' AND c.text IS NOT NULL
                     THEN ts_headline('english', left(c.text, {HEADLINE_SOURCE_CHARS}),
                                      (SELECT q FROM lex_q), {HEADLINE_OPTIONS})
                END AS snippet,
                {chunk_select} AS chunk_text,
                p.semantic
         FROM page p
         JOIN bookmarks b ON b.id = p.bookmark_id
         JOIN bookmark_contents c ON c.bookmark_id = p.bookmark_id{chunk_from}
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
        let sql = search_sql(SearchShape {
            has_tags: query.tags.is_some(),
            has_constraints: constraints.is_some(),
            vector_dims: query.vector.as_ref().map(|v| v.values.len()),
            sort: query.sort,
        });
        let mut q = sqlx::query_as::<_, SearchRow>(&sql)
            .bind(user_id)
            .bind(&query.text);
        if query.tags.is_some() {
            q = q.bind(&query.tags);
        }
        if constraints.is_some() {
            q = q.bind(&constraints);
        }
        q = q.bind(trigram_variants(&query.text));
        if let Some(vector) = &query.vector {
            q = q.bind(&vector.values).bind(&vector.model_id);
        }
        let q = q.bind(query.limit).bind(query.offset);
        let rows = if query.vector.is_some() {
            let mut tx = self.pool.begin().await.map_err(db_error)?;
            // pgvector >= 0.8: keep scanning the HNSW graph until the user
            // filter has enough rows, so other users' chunks cannot starve it.
            sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query("SET LOCAL hnsw.ef_search = 100")
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            let rows = q.fetch_all(&mut *tx).await.map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
            rows
        } else {
            q.fetch_all(&self.pool).await.map_err(db_error)?
        };
        Ok(rows
            .into_iter()
            .map(|row| SearchHit {
                bookmark: row.bookmark,
                snippet: row
                    .snippet
                    .as_deref()
                    .and_then(Snippet::from_marked)
                    .or_else(|| row.chunk_text.as_deref().and_then(Snippet::excerpt)),
                semantic: row.semantic,
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
        // is, every other waiting user is a candidate; within a user, the share
        // is taken by host turn, so an old single-host backlog does not hold
        // back that user's other hosts. `picked` takes one row per user in
        // rank order, then a second, and so on. `turns` records the users served. Every
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
                 SELECT c.user_id,
                        row_number() OVER (ORDER BY t.last_claimed_at NULLS FIRST,
                                                    min(c.next_attempt_at), c.user_id) AS user_rank
                 FROM bookmark_contents c
                 LEFT JOIN bookmark_ingest_turns t ON t.user_id = c.user_id
                 WHERE c.status = 'pending' AND c.next_attempt_at <= now() AND c.attempts < $3
                 GROUP BY c.user_id, t.last_claimed_at
                 ORDER BY user_rank
                 LIMIT {MAX_CLAIM_USERS}
             ),
             due AS (
                 SELECT w.user_rank, d.* FROM waiting w CROSS JOIN LATERAL (
                     SELECT c.bookmark_id, c.user_id, c.next_attempt_at, r.host_turn
                     FROM (
                         -- Every due row of this user, numbered within its
                         -- host, so the share below rotates through hosts
                         -- however old a single-host backlog is.
                         SELECT q.bookmark_id,
                                row_number() OVER (
                                    PARTITION BY lower(substring(b.url from '^[^:]+://(?:[^@/]*@)?([^/:?#]+)'))
                                    ORDER BY q.next_attempt_at, q.bookmark_id) AS host_turn
                         FROM bookmark_contents q JOIN bookmarks b ON b.id = q.bookmark_id
                         WHERE q.user_id = w.user_id AND q.status = 'pending'
                           AND q.next_attempt_at <= now() AND q.attempts < $3
                     ) r
                     JOIN bookmark_contents c ON c.bookmark_id = r.bookmark_id
                     -- Rechecked on the locked row version.
                     WHERE c.status = 'pending' AND c.next_attempt_at <= now() AND c.attempts < $3
                     ORDER BY r.host_turn, c.next_attempt_at, c.bookmark_id
                     -- Each user's share of the batch.
                     LIMIT ceil($1::numeric / (SELECT count(*) FROM waiting))::bigint
                     FOR UPDATE OF c SKIP LOCKED
                 ) d
             ),
             -- A second level, because window functions cannot sit next to
             -- FOR UPDATE in `due`.
             picked AS (
                 SELECT bookmark_id FROM (
                     SELECT bookmark_id, user_rank,
                            row_number() OVER (PARTITION BY user_id
                                               ORDER BY host_turn, next_attempt_at, bookmark_id)
                                AS user_turn
                     FROM due
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
        source: ContentSource,
    ) -> Result<bool, DomainError> {
        let result = sqlx::query(&format!(
            "{CLAIMED_BOOKMARK}
             UPDATE bookmark_contents c
             SET status = 'fetched', source = $6, error = NULL, claim_token = NULL,
                 text = $4, text_chars = char_length($4), content_hash = $5,
                 -- New text needs new chunks; a refetch of the same text keeps them.
                 embedded_model = CASE WHEN c.content_hash IS DISTINCT FROM $5
                                       THEN NULL ELSE c.embedded_model END,
                 embed_attempts = 0, embed_next_attempt_at = now(),
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
        .bind(source.as_str())
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

    async fn store_capture(
        &self,
        bookmark_id: Uuid,
        user_id: Uuid,
        url: &str,
        text: &str,
        content_hash: &str,
    ) -> Result<bool, DomainError> {
        // The row's search document is rebuilt from the bookmark as it is at
        // write time, and `claim_token = NULL` drops a fetch already in
        // flight (its `store_article` then updates no row). `b.url = $5` is
        // the page the text was checked against: a bookmark moved to another
        // URL in the meantime keeps its own state.
        let result = sqlx::query(
            "UPDATE bookmark_contents c
             SET status = 'fetched', source = 'captured', claim_token = NULL,
                 attempts = 0, error = NULL,
                 text = $3, text_chars = char_length($3), content_hash = $4,
                 -- New text needs new chunks; an unchanged capture keeps them.
                 embedded_model = CASE WHEN c.content_hash IS DISTINCT FROM $4
                                       THEN NULL ELSE c.embedded_model END,
                 embed_attempts = 0, embed_next_attempt_at = now(), embed_error = NULL,
                 search = bookmark_search_document(b.title, b.description, b.url, b.tags, $3),
                 fetched_at = now(), updated_at = now()
             FROM bookmarks b
             WHERE b.id = $1 AND c.bookmark_id = b.id AND c.user_id = $2 AND b.url = $5",
        )
        .bind(bookmark_id)
        .bind(user_id)
        .bind(text)
        .bind(content_hash)
        .bind(url)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
pub(crate) mod search_tests {
    use super::*;
    use crate::domain::bookmark::{BookmarkFilter, CreateBookmark};
    use crate::domain::ports::bookmark_repo::BookmarkRepository;
    use crate::domain::search::IngestStatus;
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashMap;

    pub(crate) async fn pool() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for the ignored Postgres search test");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must point at a migrated database");
        // What main.rs does after migrating; Unavailable on a database without pgvector.
        crate::adapters::postgres::vector_schema::ensure_vector_schema(&pool).await;
        pool
    }

    pub(crate) async fn user(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, 'search test')")
            .bind(id)
            .bind(format!("search-{id}@example.test"))
            .execute(pool)
            .await
            .expect("insert test user");
        id
    }

    pub(crate) async fn bookmark(
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
    pub(crate) static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    pub(crate) const LEASE: Duration = Duration::from_secs(600);

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

    /// Forget every claim of these users' rows.
    async fn reset_claims(pool: &PgPool, users: &[Uuid]) {
        sqlx::query(
            "UPDATE bookmark_contents SET next_attempt_at = now(), attempts = 0, claim_token = NULL
             WHERE user_id = ANY($1)",
        )
        .bind(users)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Move these users' pending rows to the head of the queue (ahead of
    /// anything else in a shared database), claim them, and release whatever
    /// else the claim took.
    pub(crate) async fn claim_mine(
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

    pub(crate) async fn store(repo: &PostgresPool, job: &IngestJob, text: &str) {
        assert!(
            repo.store_article(job, text, "hash", ContentSource::Fetched)
                .await
                .unwrap()
        );
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
                vector: None,
            },
        )
        .await
        .expect("search")
    }

    pub(crate) fn ids(hits: &[SearchHit]) -> Vec<Uuid> {
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
        // One-letter swaps in short words share too few trigrams for the
        // plain threshold (issue #50).
        assert_eq!(ids(&search(&repo, alice, "tokoi").await), [runtime.id]);
        assert_eq!(ids(&search(&repo, alice, "hoem").await), [garden.id]);
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
                    vector: None,
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
                .store_article(&jobs[&garden.id], "stale", "h", ContentSource::Fetched)
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

    /// Within one user, the claim's share rotates through hosts before it
    /// looks at age: an old single-host backlog does not hold back the same
    /// user's link to another host.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn a_users_share_of_a_claim_rotates_through_hosts() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        let mut backlog = Vec::new();
        for i in 0..20 {
            let b = bookmark(
                &repo,
                owner,
                &format!("https://one-host.example.net/page-{i}"),
                "backlog",
                None,
                &[],
            )
            .await;
            backlog.push(b.id);
        }
        let other_host = bookmark(
            &repo,
            owner,
            "https://another-host.example.org/new",
            "new",
            None,
            &[],
        )
        .await;
        sqlx::query(
            "UPDATE bookmark_contents SET next_attempt_at = '1970-01-01' WHERE bookmark_id = ANY($1)",
        )
        .bind(&backlog)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE bookmark_contents SET next_attempt_at = '1970-01-02' WHERE bookmark_id = $1",
        )
        .bind(other_host.id)
        .execute(&pool)
        .await
        .unwrap();
        // Two rows for every waiting user (other tests' rows may wait too),
        // so this user's share is exactly two.
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT user_id) FROM bookmark_contents
             WHERE status = 'pending' AND next_attempt_at <= now() AND attempts < $1",
        )
        .bind(MAX_INGEST_ATTEMPTS)
        .fetch_one(&pool)
        .await
        .unwrap();
        let jobs = repo.claim_ingest_batch(2 * waiting, LEASE).await.unwrap();
        let mut mine = backlog.clone();
        mine.push(other_host.id);
        release_others(&pool, &jobs, &mine).await;
        let claimed: Vec<Uuid> = jobs
            .iter()
            .map(|j| j.bookmark_id)
            .filter(|id| mine.contains(id))
            .collect();
        assert_eq!(claimed.len(), 2);
        assert!(claimed.contains(&other_host.id), "{claimed:?}");
        assert!(claimed.iter().any(|id| backlog.contains(id)));
        reset_claims(&pool, &[owner]).await;
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
        let mine_now: Vec<Uuid> = big_import
            .iter()
            .copied()
            .chain([single.id, second.id])
            .collect();
        let mut served = Vec::new();
        for _ in 0..50 {
            let one = repo.claim_ingest_batch(1, LEASE).await.unwrap();
            for job in &one {
                if big_import.contains(&job.bookmark_id) {
                    served.push(importer);
                } else if [single.id, second.id].contains(&job.bookmark_id) {
                    served.push(other);
                }
            }
            // Rows of other tests are put back; this test's stay leased.
            release_others(&pool, &one, &mine_now).await;
            if served.len() == 4 {
                break;
            }
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
        reset_claims(&pool, &[importer, other]).await;

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
        assert!(
            !repo
                .store_article(stale, "late", "h", ContentSource::Fetched)
                .await
                .unwrap()
        );
        assert!(
            repo.store_article(fresh, "on time", "h", ContentSource::Fetched)
                .await
                .unwrap()
        );

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

    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn a_captured_page_stores_wins_over_a_fetch_in_flight_and_re_embeds() {
        const PAGE_URL: &str = "https://medium.com/@writer/a-post";
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        let other = user(&pool).await;
        let page = bookmark(
            &repo,
            owner,
            "https://medium.com/@writer/a-post",
            "A post",
            None,
            &["reading"],
        )
        .await;
        let fetched = bookmark(
            &repo,
            owner,
            "https://example.org/plain",
            "Plain",
            None,
            &[],
        )
        .await;
        let jobs = claim_mine(&pool, &repo, &[owner]).await;

        // A fetch stores its text as `fetched`.
        store(&repo, &jobs[&fetched.id], "a fetched passage about compost").await;
        let (status, source, text) = contents(&pool, fetched.id).await;
        assert_eq!(status, "fetched");
        assert_eq!(source.as_deref(), Some("fetched"));
        assert!(text.unwrap().contains("compost"));

        // The capture wins: the row is fetched-with-captured-text, and the
        // claim the fetch held is gone, so its late result updates nothing.
        let captured = "capture-only passage about the borrow checker";
        assert!(
            repo.store_capture(page.id, owner, PAGE_URL, captured, "h-capture")
                .await
                .unwrap()
        );
        let (status, source, text) = contents(&pool, page.id).await;
        assert_eq!(status, "fetched");
        assert_eq!(source.as_deref(), Some("captured"));
        assert_eq!(text.as_deref(), Some(captured));
        assert!(
            !repo
                .store_article(
                    &jobs[&page.id],
                    "late fetch",
                    "h-late",
                    ContentSource::Fetched
                )
                .await
                .unwrap(),
            "a fetch already in flight loses to the capture"
        );
        assert_eq!(contents(&pool, page.id).await.2.as_deref(), Some(captured));
        // The lexical leg finds the captured body at once.
        let hits = search(&repo, owner, "borrow checker").await;
        assert_eq!(ids(&hits), [page.id]);

        // A capture of another user's bookmark, or of a bookmark that does
        // not exist, stores nothing.
        assert!(
            !repo
                .store_capture(
                    fetched.id,
                    other,
                    "https://example.org/plain",
                    "not mine",
                    "h"
                )
                .await
                .unwrap()
        );
        assert!(
            !repo
                .store_capture(Uuid::new_v4(), owner, "https://nowhere.test/", "x", "h")
                .await
                .unwrap()
        );
        assert_eq!(
            contents(&pool, fetched.id).await.1.as_deref(),
            Some("fetched")
        );

        // The same text again keeps the vectors; new text drops them.
        sqlx::query("UPDATE bookmark_contents SET embedded_model = 'stub' WHERE bookmark_id = $1")
            .bind(page.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            repo.store_capture(page.id, owner, PAGE_URL, captured, "h-capture")
                .await
                .unwrap()
        );
        let model: Option<String> = sqlx::query_scalar(
            "SELECT embedded_model FROM bookmark_contents WHERE bookmark_id = $1",
        )
        .bind(page.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            model.as_deref(),
            Some("stub"),
            "unchanged text keeps chunks"
        );

        let recaptured = "a longer capture that says something else entirely";
        assert!(
            repo.store_capture(page.id, owner, PAGE_URL, recaptured, "h-recapture")
                .await
                .unwrap()
        );
        let model: Option<String> = sqlx::query_scalar(
            "SELECT embedded_model FROM bookmark_contents WHERE bookmark_id = $1",
        )
        .bind(page.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(model, None, "new text re-embeds");

        // A URL change is a new page: the captured text and its source go.
        sqlx::query(
            "UPDATE bookmarks SET url = 'https://medium.com/@writer/a-post-v2' WHERE id = $1",
        )
        .bind(page.id)
        .execute(&pool)
        .await
        .unwrap();
        let (status, source, text) = contents(&pool, page.id).await;
        assert_eq!(status, "pending");
        assert_eq!(source, None);
        assert_eq!(text, None);
        // A capture already in flight for the old page stores nothing: the
        // text belonged to the URL the bookmark no longer has.
        assert!(
            !repo
                .store_capture(page.id, owner, PAGE_URL, "the old page's text", "h-old")
                .await
                .unwrap()
        );
        let (status, source, text) = contents(&pool, page.id).await;
        assert_eq!((status.as_str(), source, text), ("pending", None, None));

        // Each source is recorded as given.
        let scraped = bookmark(&repo, owner, "https://example.org/s", "S", None, &[]).await;
        let jobs = claim_mine(&pool, &repo, &[owner]).await;
        assert!(
            repo.store_article(
                &jobs[&scraped.id],
                "a scraped passage",
                "h-scraped",
                ContentSource::Scraped
            )
            .await
            .unwrap()
        );
        assert_eq!(
            contents(&pool, scraped.id).await.1.as_deref(),
            Some("scraped")
        );

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(owner)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn the_page_url_lookup_is_normalized_and_scoped_to_the_owner() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        let other = user(&pool).await;
        let exact = bookmark(
            &repo,
            owner,
            "https://medium.com/@writer/first",
            "First",
            None,
            &[],
        )
        .await;
        let slashed = bookmark(
            &repo,
            owner,
            "https://medium.com/@writer/second/",
            "Second",
            None,
            &[],
        )
        .await;
        let theirs = bookmark(
            &repo,
            other,
            "https://medium.com/@writer/first",
            "Not mine",
            None,
            &[],
        )
        .await;

        let lookup = |url: &'static str| {
            let repo = repo.clone();
            async move {
                repo.list(
                    owner,
                    BookmarkFilter {
                        url: Some(url.to_string()),
                        limit: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
            }
        };

        // An exact match answers first.
        let hits = lookup("https://medium.com/@writer/first").await;
        assert_eq!(ids_of(&hits), [exact.id]);
        // Case in the scheme, a trailing slash and tracking parameters still
        // find the saved URL; nothing of another user's comes back.
        let hits = lookup("http://medium.com/@writer/first?utm_source=chrome").await;
        assert_eq!(ids_of(&hits), [exact.id]);
        let hits = lookup("https://medium.com/@writer/second?fbclid=abc").await;
        assert_eq!(ids_of(&hits), [slashed.id]);
        // A different page on the same host is not this bookmark.
        assert!(lookup("https://medium.com/@writer/third").await.is_empty());
        assert!(lookup("not a url").await.is_empty());
        // The other user's bookmark of the same URL is not visible.
        let theirs_list = repo
            .list(
                other,
                BookmarkFilter {
                    url: Some("https://medium.com/@writer/first".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(ids_of(&theirs_list), [theirs.id]);

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(owner)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    /// `(status, source, text)` of one bookmark's content row.
    async fn contents(pool: &PgPool, id: Uuid) -> (String, Option<String>, Option<String>) {
        sqlx::query_as("SELECT status, source, text FROM bookmark_contents WHERE bookmark_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn ids_of(bookmarks: &[Bookmark]) -> Vec<Uuid> {
        bookmarks.iter().map(|b| b.id).collect()
    }
}
