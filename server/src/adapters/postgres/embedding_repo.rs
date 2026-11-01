use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::embedding_repo::EmbeddingRepository;
use crate::domain::search::{EmbedJob, EmbeddedChunk};
use chrono::{DateTime, Utc};
use std::fmt::Write;
use std::time::Duration;

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
