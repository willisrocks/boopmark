use crate::domain::error::DomainError;

pub use crate::domain::ports::llm_client::BoxFuture;

/// The scraper's spend, one row per calendar month (`scraper_usage`,
/// migration 019), so a restart cannot reset the counter. Only the monthly
/// page cap is enforced today; the row also records failures, which cost
/// nothing.
///
/// Boxed futures rather than `#[trait_variant::make(Send)]`: the fallback
/// decorator holds an `Arc<dyn ScraperUsageRepository>`, like the `Embedder`
/// it is modelled on.
pub trait ScraperUsageRepository: Send + Sync {
    /// Reserve one page of the current UTC month. `budget` is the month's
    /// page cap, 0 meaning no cap. Returns false when the month's count has
    /// already reached the cap; the caller then skips the scrape. The check
    /// and the increment are one statement, so concurrent workers never
    /// exceed the cap.
    fn reserve_page(&self, budget: u64) -> BoxFuture<'_, Result<bool, DomainError>>;

    /// Count a scrape that produced nothing usable. `billed` is the vendor's
    /// billing: a page it answered 2xx for was paid for and stays counted; a
    /// request that never got a 2xx is refunded. Either way the failure is
    /// recorded.
    fn record_failure(&self, billed: bool) -> BoxFuture<'_, Result<(), DomainError>>;
}
