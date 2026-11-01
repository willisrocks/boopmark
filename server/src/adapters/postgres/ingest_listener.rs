use sqlx::postgres::PgListener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// The channel that migration 016's trigger notifies when a bookmark is
/// queued for article ingest.
const CHANNEL: &str = "bookmark_ingest";
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Listen for `NOTIFY bookmark_ingest` on a dedicated connection and wake
/// the ingest worker, so a new bookmark is fetched within seconds instead of
/// at the next poll. If the connection fails, the worker still polls.
pub fn spawn_ingest_listener(database_url: String, wake: Arc<Notify>) {
    tokio::spawn(async move {
        loop {
            match PgListener::connect(&database_url).await {
                Ok(mut listener) => match listener.listen(CHANNEL).await {
                    Ok(()) => loop {
                        match listener.recv().await {
                            Ok(_) => wake.notify_one(),
                            Err(e) => {
                                tracing::warn!(error = %e, "ingest listener lost its connection");
                                break;
                            }
                        }
                    },
                    Err(e) => tracing::warn!(error = %e, "ingest listener could not LISTEN"),
                },
                Err(e) => tracing::warn!(error = %e, "ingest listener could not connect"),
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });
}
