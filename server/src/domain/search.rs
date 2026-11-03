use crate::domain::bookmark::{Bookmark, BookmarkSort};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use uuid::Uuid;

/// A search returns at most this many fused results. Every leg contributes
/// its own top 50, and an uncapped result set would page through the whole
/// library on vague queries.
pub const MAX_SEARCH_RESULTS: i64 = 50;

/// Extracted article text is truncated to this many characters before it is
/// stored. The measured maximum was 99k; the lexical leg keeps all of it.
pub const ARTICLE_TEXT_CAP_CHARS: usize = 200_000;
/// ... and to this many bytes, which keeps its tsvector well under
/// Postgres's 1 MB limit even for text in a multibyte script.
pub const ARTICLE_TEXT_CAP_BYTES: usize = 512 * 1024;

/// A queue row is given up (`failed`) once it has been claimed this many
/// times. The retry schedule ends after 4 attempts; the margin covers leases
/// that ran out because a worker died mid-job.
pub const MAX_INGEST_ATTEMPTS: i32 = 6;

/// The database wraps each highlighted term of a snippet in these control
/// characters. They never occur in extracted text, and they keep the markup
/// out of SQL so that the template layer does all of the escaping.
pub const SNIPPET_START: char = '\u{2}';
pub const SNIPPET_STOP: char = '\u{3}';
/// A snippet without highlights (a chunk found by meaning) is cut to about
/// this many characters.
pub const SNIPPET_EXCERPT_CHARS: usize = 160;

/// The queue states an ingest attempt can leave behind without an article.
/// (A stored article is `fetched`; `store_article` sets that itself.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestStatus {
    Pending,
    Failed,
    Skipped,
}

impl IngestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// When to retry after a transient fetch failure, given how many attempts
/// have been made so far: after 1 minute, then 1 hour, then 24 hours. `None`
/// means give up and mark the article `failed`.
pub fn retry_delay(attempts: i32) -> Option<Duration> {
    match attempts {
        ..=1 => Some(Duration::minutes(1)),
        2 => Some(Duration::hours(1)),
        3 => Some(Duration::hours(24)),
        _ => None,
    }
}

/// One claimed row of the ingest queue. The result is only stored while the
/// bookmark still has `url` and the row still carries `claim_token`: a URL
/// edit or a later claim (after the lease ran out) makes this job stale.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct IngestJob {
    pub bookmark_id: Uuid,
    pub url: String,
    /// Attempts including this one.
    pub attempts: i32,
    pub claim_token: Uuid,
}

/// How an ingest attempt ended when no article was stored. A `Pending`
/// status with `retry_at` puts the job back in the queue; `Failed` and
/// `Skipped` are final, and the bookmark stays searchable by its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestFailure {
    pub status: IngestStatus,
    pub error: String,
    pub retry_at: Option<DateTime<Utc>>,
}

/// When to retry embedding after a transient error (an outage, 429, a
/// timeout), given the attempts so far: after 1 minute, 10 minutes, 1 hour,
/// then every 6 hours. Never given up, so an outage leaves no bookmark
/// without vectors for good.
pub fn embed_retry_delay(attempts: i32) -> Duration {
    match attempts {
        ..=1 => Duration::minutes(1),
        2 => Duration::minutes(10),
        3 => Duration::hours(1),
        _ => Duration::hours(6),
    }
}

/// One bookmark claimed from the embed queue: a row that is no longer
/// pending and whose chunks were not built by the configured embedder. The
/// result is stored only while the row still has `version`; any edit since
/// the claim makes the job stale, and the next claim redoes it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct EmbedJob {
    pub bookmark_id: Uuid,
    pub user_id: Uuid,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// The extracted article, if one was stored.
    pub text: Option<String>,
    /// Attempts including this one.
    pub embed_attempts: i32,
    /// `bookmark_contents.updated_at` when it was claimed.
    pub version: DateTime<Utc>,
}

/// A stored passage and its vector.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedChunk {
    pub text: String,
    pub embedding: Vec<f32>,
}

/// The configured embedder's vector space, for the vector leg: the query's
/// vector, and the `model_id` its chunks must have been built with.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryVector {
    pub model_id: String,
    pub values: Vec<f32>,
}

/// Where a stored article text came from (`bookmark_contents.source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentSource {
    /// The server fetched the page itself.
    #[default]
    Fetched,
    /// The browser that displayed the page sent its HTML.
    Captured,
    /// The server-side scraper rendered the page.
    Scraped,
}

impl ContentSource {
    /// The column's value; the migration's `CHECK` lists the same three.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fetched => "fetched",
            Self::Captured => "captured",
            Self::Scraped => "scraped",
        }
    }
}

/// Articles with stored text, by where the text came from. Settings → Search
/// index breaks the indexed count down this way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentSourceCounts {
    pub fetched: i64,
    pub captured: i64,
    pub scraped: i64,
}

impl ContentSourceCounts {
    pub fn total(&self) -> i64 {
        self.fetched + self.captured + self.scraped
    }
}

/// One user's search index, for Settings → Search index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchIndexStatus {
    pub total: i64,
    /// Article text stored, by source.
    pub indexed: ContentSourceCounts,
    pub pending: i64,
    /// Failed or skipped: searchable by their metadata only.
    pub unfetched: i64,
    /// Embedded by the configured model; `None` with embeddings disabled.
    pub embedded: Option<i64>,
    /// The hosts with the most unfetched bookmarks, most first.
    pub failing_domains: Vec<(String, i64)>,
}

/// The parts of a `websearch_to_tsquery` query that must hold for every
/// result, whichever leg found it: quoted phrases and `-exclusions`, as a
/// websearch string of their own (`coffee "cold brew" -tea` gives
/// `"cold brew" -tea`). `None` when the query has neither.
///
/// `or` keeps its meaning. websearch binds AND tighter than OR, so the query
/// is a disjunction of groups; the constraint keeps the operators of every
/// group, joined by `or`. A group without operators constrains nothing, and
/// then neither does the whole query (`"cold brew" or espresso`).
pub fn websearch_constraints(text: &str) -> Option<String> {
    #[derive(Default)]
    struct Group {
        operators: Vec<String>,
        has_terms: bool,
    }
    let mut groups = vec![Group::default()];
    for token in websearch_tokens(text) {
        let group = groups.last_mut().unwrap();
        match token {
            WebsearchToken::Or => groups.push(Group::default()),
            WebsearchToken::Operator(op) => {
                group.operators.push(op);
                group.has_terms = true;
            }
            WebsearchToken::Word(_) => group.has_terms = true,
        }
    }
    let mut constraint = Vec::new();
    for group in groups.into_iter().filter(|g| g.has_terms) {
        if group.operators.is_empty() {
            return None;
        }
        constraint.push(group.operators.join(" "));
    }
    (!constraint.is_empty()).then(|| constraint.join(" or "))
}

/// The query as the embedder should see it: its terms and phrases, without
/// quotes, `-exclusions` or `or`. An excluded term would pull the query
/// vector towards what the user does not want (`coffee -tea` embeds
/// `coffee`); the exclusion itself still filters the vector leg.
pub fn semantic_query_text(text: &str) -> String {
    websearch_tokens(text)
        .into_iter()
        .filter_map(|token| match token {
            WebsearchToken::Word(word) => Some(word),
            WebsearchToken::Operator(op) if !op.starts_with('-') => {
                Some(op.trim_matches('"').to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

enum WebsearchToken {
    /// A quoted phrase or an `-exclusion`, as written.
    Operator(String),
    /// The `or` keyword.
    Or,
    /// Any other term.
    Word(String),
}

/// Splits a query the way `websearch_to_tsquery` reads it. Minus signs
/// negate the next term or phrase even when spaced (`coffee - tea` is
/// `'coffe' & !'tea'` in Postgres), so a run of them is carried over to it,
/// and the operator is passed on as written, signs and all.
fn websearch_tokens(text: &str) -> Vec<WebsearchToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    // Minus signs seen since the last term.
    let mut signs = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let body = i + chars[i..].iter().take_while(|c| **c == '-').count();
        signs.extend(&chars[i..body]);
        if body == chars.len() || chars[body].is_whitespace() {
            // Only minus signs: they apply to whatever comes next.
            i = body;
            continue;
        }
        if chars[body] == '"' {
            let close = chars[body + 1..].iter().position(|c| *c == '"');
            let end = close.map_or(chars.len(), |p| body + 1 + p);
            let phrase: String = chars[body + 1..end].iter().collect();
            if !phrase.trim().is_empty() {
                tokens.push(WebsearchToken::Operator(format!(
                    "{signs}\"{}\"",
                    phrase.trim()
                )));
            }
            signs.clear();
            i = end + 1;
            continue;
        }
        // A quote ends a word and starts a phrase, as in Postgres
        // (`coffee"cold brew"` is `'coffe' & 'cold' <-> 'brew'`).
        let end = chars[body..]
            .iter()
            .position(|c| c.is_whitespace() || *c == '"')
            .map_or(chars.len(), |p| body + p);
        let word: String = chars[body..end].iter().collect();
        if !signs.is_empty() {
            tokens.push(WebsearchToken::Operator(format!("{signs}{word}")));
        } else if word.eq_ignore_ascii_case("or") {
            tokens.push(WebsearchToken::Or);
        } else {
            tokens.push(WebsearchToken::Word(word));
        }
        signs.clear();
        i = end;
    }
    tokens
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub tags: Option<Vec<String>>,
    pub sort: BookmarkSort,
    pub limit: i64,
    pub offset: i64,
    /// The query embedded by the configured embedder; `None` runs the
    /// lexical legs alone.
    pub vector: Option<QueryVector>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub bookmark: Bookmark,
    pub snippet: Option<Snippet>,
    /// Only the vector leg found it: no keyword of the query matched, so the
    /// card says why it is listed.
    pub semantic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnippetSegment {
    pub text: String,
    pub highlighted: bool,
}

/// A passage of the article around the matched terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub segments: Vec<SnippetSegment>,
}

impl Snippet {
    /// Parse a headline whose matches are wrapped in `SNIPPET_START` /
    /// `SNIPPET_STOP`. Returns `None` when nothing is highlighted: such a
    /// headline is just the start of the article and explains nothing.
    pub fn from_marked(marked: &str) -> Option<Self> {
        let collapsed = marked.split_whitespace().collect::<Vec<_>>().join(" ");
        let segments: Vec<SnippetSegment> = collapsed
            .split([SNIPPET_START, SNIPPET_STOP])
            .enumerate()
            .filter(|(_, text)| !text.is_empty())
            .map(|(i, text)| SnippetSegment {
                text: text.to_string(),
                highlighted: i % 2 == 1,
            })
            .collect();
        segments
            .iter()
            .any(|s| s.highlighted)
            .then_some(Self { segments })
    }

    /// A plain passage cut to about `SNIPPET_EXCERPT_CHARS` at a word
    /// boundary, for a hit with no highlighted terms (the vector leg's best
    /// chunk). `None` for blank text.
    pub fn excerpt(text: &str) -> Option<Self> {
        let mut excerpt = String::new();
        for word in text.split_whitespace() {
            if !excerpt.is_empty()
                && excerpt.chars().count() + 1 + word.chars().count() > SNIPPET_EXCERPT_CHARS
            {
                excerpt.push_str(" …");
                break;
            }
            if !excerpt.is_empty() {
                excerpt.push(' ');
            }
            excerpt.extend(word.chars().take(SNIPPET_EXCERPT_CHARS));
        }
        (!excerpt.is_empty()).then(|| Self {
            segments: vec![SnippetSegment {
                text: excerpt,
                highlighted: false,
            }],
        })
    }

    pub fn plain_text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_sources_are_the_column_values_and_add_up() {
        assert_eq!(ContentSource::Fetched.as_str(), "fetched");
        assert_eq!(ContentSource::Captured.as_str(), "captured");
        assert_eq!(ContentSource::Scraped.as_str(), "scraped");
        assert_eq!(ContentSource::default(), ContentSource::Fetched);

        let counts = ContentSourceCounts {
            fetched: 1140,
            captured: 48,
            scraped: 16,
        };
        assert_eq!(counts.total(), 1204);
        assert_eq!(ContentSourceCounts::default().total(), 0);
    }

    #[test]
    fn retry_schedule_is_one_minute_one_hour_one_day_then_final() {
        assert_eq!(retry_delay(1), Some(Duration::minutes(1)));
        assert_eq!(retry_delay(2), Some(Duration::hours(1)));
        assert_eq!(retry_delay(3), Some(Duration::hours(24)));
        assert_eq!(retry_delay(4), None);
        assert_eq!(retry_delay(9), None);
    }

    #[test]
    fn websearch_constraints_keep_phrases_and_exclusions_only() {
        assert_eq!(
            websearch_constraints("coffee -tea").as_deref(),
            Some("-tea")
        );
        assert_eq!(
            websearch_constraints("coffee \"cold brew\" -tea -\"green tea\"").as_deref(),
            Some("\"cold brew\" -tea -\"green tea\"")
        );
        assert_eq!(
            websearch_constraints("\"unterminated phrase").as_deref(),
            Some("\"unterminated phrase\"")
        );
        assert_eq!(websearch_constraints("well-known rust"), None);
        assert_eq!(websearch_constraints("plain words"), None);
        assert_eq!(websearch_constraints("dangling - \"\""), None);
        assert_eq!(websearch_constraints("coffee -"), None);
        // Glued quotes still make a phrase, as Postgres reads them.
        assert_eq!(
            websearch_constraints("coffee\"cold brew\"").as_deref(),
            Some("\"cold brew\"")
        );
        assert_eq!(
            websearch_constraints("coffee -\"cold brew\"tea").as_deref(),
            Some("-\"cold brew\"")
        );
        assert_eq!(
            semantic_query_text("coffee\"cold brew\""),
            "coffee cold brew"
        );
    }

    /// Postgres reads a spaced minus as an exclusion (`coffee - tea` is
    /// `'coffe' & !'tea'`), so every leg must exclude too.
    #[test]
    fn websearch_constraints_treat_a_spaced_minus_like_postgres() {
        assert_eq!(
            websearch_constraints("coffee - tea").as_deref(),
            Some("-tea")
        );
        assert_eq!(
            websearch_constraints("coffee - \"green tea\"").as_deref(),
            Some("-\"green tea\"")
        );
        assert_eq!(
            websearch_constraints("- tea coffee").as_deref(),
            Some("-tea")
        );
        // Double negation is passed on as written, for Postgres to read.
        assert_eq!(
            websearch_constraints("coffee - - tea").as_deref(),
            Some("--tea")
        );
        assert_eq!(
            websearch_constraints("\"cold brew\" OR \"grind size\" -tea").as_deref(),
            Some("\"cold brew\" or \"grind size\" -tea")
        );
        assert_eq!(websearch_constraints("\"cold brew\" or espresso"), None);
        assert_eq!(
            websearch_constraints("or coffee -tea or").as_deref(),
            Some("-tea")
        );
    }

    #[test]
    fn snippet_splits_highlights_and_collapses_whitespace() {
        let snippet =
            Snippet::from_marked("the  \n\u{2}borrow\u{3} checker\trejects \u{2}aliasing\u{3}.")
                .unwrap();
        assert_eq!(
            snippet.segments,
            vec![
                SnippetSegment {
                    text: "the ".into(),
                    highlighted: false
                },
                SnippetSegment {
                    text: "borrow".into(),
                    highlighted: true
                },
                SnippetSegment {
                    text: " checker rejects ".into(),
                    highlighted: false
                },
                SnippetSegment {
                    text: "aliasing".into(),
                    highlighted: true
                },
                SnippetSegment {
                    text: ".".into(),
                    highlighted: false
                },
            ]
        );
        assert_eq!(snippet.plain_text(), "the borrow checker rejects aliasing.");
    }

    #[test]
    fn snippet_merges_adjacent_highlights() {
        let snippet = Snippet::from_marked("\u{2}hybrid\u{3} \u{2}search\u{3} works").unwrap();
        assert_eq!(snippet.segments[0].text, "hybrid");
        assert_eq!(snippet.segments[1].text, " ");
        assert_eq!(snippet.segments[2].text, "search");
        assert_eq!(snippet.plain_text(), "hybrid search works");
    }

    #[test]
    fn snippet_without_highlight_is_none() {
        assert_eq!(Snippet::from_marked("just the opening paragraph"), None);
        assert_eq!(Snippet::from_marked(""), None);
    }

    #[test]
    fn semantic_query_text_drops_exclusions_quotes_and_or() {
        assert_eq!(semantic_query_text("coffee -tea"), "coffee");
        assert_eq!(semantic_query_text("coffee - \"green tea\""), "coffee");
        assert_eq!(
            semantic_query_text("\"cold brew\" or espresso  grind"),
            "cold brew espresso grind"
        );
        assert_eq!(
            semantic_query_text("how do lifetimes work"),
            "how do lifetimes work"
        );
        assert_eq!(semantic_query_text("-tea"), "");
    }

    #[test]
    fn excerpts_cut_at_a_word_boundary() {
        let short = Snippet::excerpt("  a short\n passage ").unwrap();
        assert_eq!(short.plain_text(), "a short passage");
        assert!(!short.segments[0].highlighted);
        let long = "word ".repeat(100);
        let cut = Snippet::excerpt(&long).unwrap().plain_text();
        assert!(cut.ends_with("word …"));
        assert!(cut.chars().count() <= SNIPPET_EXCERPT_CHARS + 2);
        let huge = "x".repeat(1000);
        assert_eq!(
            Snippet::excerpt(&huge)
                .unwrap()
                .plain_text()
                .chars()
                .count(),
            SNIPPET_EXCERPT_CHARS
        );
        assert_eq!(Snippet::excerpt(" \n "), None);
    }

    #[test]
    fn snippet_keeps_markup_as_text() {
        let snippet = Snippet::from_marked("<script>\u{2}alert\u{3}</script>").unwrap();
        assert_eq!(snippet.segments[0].text, "<script>");
        assert_eq!(snippet.segments[1].text, "alert");
    }
}
