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

/// The database wraps each highlighted term of a snippet in these control
/// characters. They never occur in extracted text, and they keep the markup
/// out of SQL so that the template layer does all of the escaping.
pub const SNIPPET_START: char = '\u{2}';
pub const SNIPPET_STOP: char = '\u{3}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestStatus {
    Pending,
    Fetched,
    Failed,
    Skipped,
}

impl IngestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fetched => "fetched",
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

/// One claimed row of the ingest queue. `url` is the URL at claim time; the
/// result is only stored while the bookmark still has it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct IngestJob {
    pub bookmark_id: Uuid,
    pub url: String,
    /// Attempts including this one.
    pub attempts: i32,
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
pub fn websearch_constraints(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let at_word_start = i == 0 || chars[i - 1].is_whitespace();
        let negated = chars[i] == '-' && at_word_start;
        let body = if negated { i + 1 } else { i };
        if chars.get(body) == Some(&'"') && (negated || at_word_start) {
            let close = chars[body + 1..].iter().position(|c| *c == '"');
            let end = close.map_or(chars.len(), |p| body + 1 + p);
            let phrase: String = chars[body + 1..end].iter().collect();
            if !phrase.trim().is_empty() {
                parts.push(format!(
                    "{}\"{}\"",
                    if negated { "-" } else { "" },
                    phrase.trim()
                ));
            }
            i = end + 1;
        } else if negated && chars.get(body).is_some_and(|c| !c.is_whitespace()) {
            let end = chars[body..]
                .iter()
                .position(|c| c.is_whitespace())
                .map_or(chars.len(), |p| body + p);
            parts.push(chars[i..end].iter().collect());
            i = end;
        } else {
            i += 1;
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
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
