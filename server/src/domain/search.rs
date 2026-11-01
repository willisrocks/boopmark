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
            WebsearchToken::Word => group.has_terms = true,
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

enum WebsearchToken {
    /// A quoted phrase or an `-exclusion`, as written.
    Operator(String),
    /// The `or` keyword.
    Or,
    /// Any other term.
    Word,
}

fn websearch_tokens(text: &str) -> Vec<WebsearchToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let negated = chars[i] == '-';
        let body = if negated { i + 1 } else { i };
        if chars.get(body) == Some(&'"') {
            let close = chars[body + 1..].iter().position(|c| *c == '"');
            let end = close.map_or(chars.len(), |p| body + 1 + p);
            let phrase: String = chars[body + 1..end].iter().collect();
            if !phrase.trim().is_empty() {
                let sign = if negated { "-" } else { "" };
                tokens.push(WebsearchToken::Operator(format!(
                    "{sign}\"{}\"",
                    phrase.trim()
                )));
            }
            i = end + 1;
            continue;
        }
        let end = chars[i..]
            .iter()
            .position(|c| c.is_whitespace())
            .map_or(chars.len(), |p| i + p);
        let word: String = chars[i..end].iter().collect();
        if negated && word.len() > 1 {
            tokens.push(WebsearchToken::Operator(word));
        } else if word.eq_ignore_ascii_case("or") {
            tokens.push(WebsearchToken::Or);
        } else if word != "-" {
            tokens.push(WebsearchToken::Word);
        }
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
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub bookmark: Bookmark,
    pub snippet: Option<Snippet>,
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

    pub fn plain_text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn snippet_keeps_markup_as_text() {
        let snippet = Snippet::from_marked("<script>\u{2}alert\u{3}</script>").unwrap();
        assert_eq!(snippet.segments[0].text, "<script>");
        assert_eq!(snippet.segments[1].text, "alert");
    }
}
