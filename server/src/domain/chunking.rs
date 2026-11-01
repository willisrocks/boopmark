//! Splits a bookmark into the passages the vector leg embeds. A port of the
//! prototype's `scripts/search-eval/chunking.py`, which the measured nDCG
//! numbers come from: paragraph-aware windows of about 200 words with a
//! 30-word overlap, each embedded under a header (the title, plus the
//! description on the first chunk) so a deep passage keeps its article's
//! topic. The chunk's own text, without the header, is the snippet.

/// Words per window, and how many the next window repeats. Paragraph breaks
/// count as words here, as in the prototype.
pub const WINDOW_WORDS: usize = 200;
pub const OVERLAP_WORDS: usize = 30;
/// Chunks embedded per bookmark (6 of the prototype's 490 hit it). The rest
/// of the text stays in the lexical index.
pub const MAX_CHUNKS: usize = 64;
/// A window also ends at this many characters. Models embed tokens, not
/// words: prose (about 6 characters a word) keeps its 200-word windows, but
/// dense code is cut well inside BGE's 512-token limit, which it would
/// otherwise truncate silently.
pub const MAX_CHUNK_CHARS: usize = 1600;
/// Shorter article text is not chunked; the bookmark gets the metadata chunk
/// instead. Migration 017's sync trigger repeats this number.
pub const MIN_CHUNKED_TEXT_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The passage as stored and shown.
    pub text: String,
    /// What is embedded: the passage under its header.
    pub input: String,
}

/// The bookmark fields a chunk is built from.
#[derive(Debug, Clone, Copy)]
pub struct ChunkSource<'a> {
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub url: &'a str,
    pub tags: &'a [String],
    /// The extracted article, if it was fetched.
    pub text: Option<&'a str>,
}

impl ChunkSource<'_> {
    /// The article's passages when it has at least `MIN_CHUNKED_TEXT_CHARS`
    /// of text, otherwise the one metadata chunk (a failed or skipped fetch,
    /// or a page with almost no text).
    pub fn chunks(&self) -> Vec<Chunk> {
        let article = self
            .text
            .filter(|text| text.chars().count() >= MIN_CHUNKED_TEXT_CHARS)
            .map(|text| chunk_article(self.title, self.description, text))
            .unwrap_or_default();
        if article.is_empty() {
            vec![self.metadata_chunk()]
        } else {
            article
        }
    }

    /// `title\ndescription\nurl\ntags`, the measured fallback for a page
    /// that could not be fetched: it takes failed-link queries from 0.061 to
    /// 0.918 nDCG@10.
    pub fn metadata_chunk(&self) -> Chunk {
        let tags = self.tags.join(" ");
        let text = [
            self.title.unwrap_or_default(),
            self.description.unwrap_or_default(),
            self.url,
            &tags,
        ]
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        let text = truncate_chars(&text, MAX_CHUNK_CHARS);
        Chunk {
            input: text.clone(),
            text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a str),
    /// The end of a paragraph.
    Break,
}

impl Token<'_> {
    fn chars(&self) -> usize {
        match self {
            Token::Word(word) => word.chars().count(),
            Token::Break => 1,
        }
    }
}

/// The text's words, each paragraph followed by a `Break`. A paragraph ends
/// at a blank line, or before a line that starts with `=` (a wiki heading),
/// like the prototype's `\n\s*\n|\n(?==)` split.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut in_paragraph = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('=') {
            if in_paragraph {
                tokens.push(Token::Break);
                in_paragraph = false;
            }
            if line.trim().is_empty() {
                continue;
            }
        }
        for word in line.split_whitespace() {
            tokens.push(Token::Word(word));
            in_paragraph = true;
        }
    }
    if in_paragraph {
        tokens.push(Token::Break);
    }
    tokens
}

/// Words joined by spaces, paragraphs by newlines.
fn render(tokens: &[Token<'_>]) -> String {
    let mut out = String::new();
    for token in tokens {
        match token {
            Token::Word(word) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(word);
            }
            Token::Break => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    truncate_chars(out.trim_end(), MAX_CHUNK_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// Paragraph-aware windows over `text`, at most `MAX_CHUNKS`, each with its
/// embedding header.
pub fn chunk_article(title: Option<&str>, description: Option<&str>, text: &str) -> Vec<Chunk> {
    let tokens = tokens(text);
    let mut passages: Vec<String> = Vec::new();
    let mut start = 0;
    while start < tokens.len() && passages.len() < MAX_CHUNKS {
        let mut end = start;
        let mut chars = 0;
        while end < tokens.len() && end - start < WINDOW_WORDS {
            let len = tokens[end].chars() + 1;
            if end > start && chars + len > MAX_CHUNK_CHARS {
                break;
            }
            chars += len;
            end += 1;
        }
        let window = &tokens[start..end];
        if window.iter().any(|t| matches!(t, Token::Word(_))) {
            passages.push(render(window));
        }
        // Unlike the prototype, which also emitted a last window made only
        // of the overlap, every window holds text the previous one did not.
        if end == tokens.len() {
            break;
        }
        // Repeat the overlap, but always move forward.
        start = if end - start > OVERLAP_WORDS {
            end - OVERLAP_WORDS
        } else {
            end
        };
    }

    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let description = description.map(str::trim).filter(|d| !d.is_empty());
    passages
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mut input = String::new();
            if let Some(title) = title {
                input.push_str(title);
                input.push('\n');
            }
            if i == 0
                && let Some(description) = description
            {
                input.push_str(description);
                input.push('\n');
            }
            input.push_str(&text);
            Chunk { text, input }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(range: std::ops::Range<usize>) -> String {
        range.map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")
    }

    fn source<'a>(text: Option<&'a str>, tags: &'a [String]) -> ChunkSource<'a> {
        ChunkSource {
            title: Some("Borrowing in Rust"),
            description: Some("How references stay valid"),
            url: "https://example.com/borrowing",
            tags,
            text,
        }
    }

    #[test]
    fn windows_overlap_and_end_without_a_redundant_tail() {
        // 399 words + 1 paragraph break = 400 tokens: windows start at 0,
        // 170 and 340.
        let text = words(0..399);
        let chunks = chunk_article(None, None, &text);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].text.starts_with("w0 w1 "));
        assert!(chunks[0].text.ends_with(" w199"));
        assert!(chunks[1].text.starts_with("w170 "), "30-word overlap");
        assert!(chunks[1].text.ends_with(" w369"));
        assert!(chunks[2].text.starts_with("w340 ") && chunks[2].text.ends_with(" w398"));

        // An exact fit has no extra window made only of the overlap.
        let text = words(0..150);
        assert_eq!(chunk_article(None, None, &text).len(), 1);
    }

    #[test]
    fn paragraphs_count_as_words_and_render_as_newlines() {
        let text = "first para\nstill first\n\n  \nsecond para\n== Heading ==\nunder it";
        let chunks = chunk_article(None, None, text);
        assert_eq!(
            chunks[0].text,
            "first para still first\nsecond para\n== Heading == under it"
        );
    }

    #[test]
    fn header_has_the_title_and_the_description_on_the_first_chunk_only() {
        let text = words(0..300);
        let chunks = chunk_article(Some(" Title "), Some("About it"), &text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0].input,
            format!("Title\nAbout it\n{}", chunks[0].text)
        );
        assert_eq!(chunks[1].input, format!("Title\n{}", chunks[1].text));
        assert!(
            !chunks[0].text.contains("Title"),
            "the stored text has no header"
        );

        let bare = chunk_article(None, Some("  "), "just text");
        assert_eq!(bare[0].input, "just text");
    }

    #[test]
    fn long_words_end_windows_early_without_losing_text() {
        // 200 characters a word: eight fit in a window, and the next window
        // starts where it ended (no overlap is worth repeating).
        let long: Vec<String> = (0..12)
            .map(|i| format!("{i:02}{}", "x".repeat(197)))
            .collect();
        let chunks = chunk_article(None, None, &long.join(" "));
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|c| c.text.chars().count() <= MAX_CHUNK_CHARS)
        );
        assert_eq!(chunks[0].text, long[..8].join(" "));
        assert_eq!(chunks[1].text, long[8..].join(" "));
    }

    #[test]
    fn chunks_are_capped_in_count_and_characters() {
        let text = words(0..20_000);
        let chunks = chunk_article(None, None, &text);
        assert_eq!(chunks.len(), MAX_CHUNKS);

        // Dense text (code) ends a window by characters, not words.
        let dense = (0..400)
            .map(|i| format!("token_{i:04}_{}", "y".repeat(20)))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_article(None, None, &dense);
        assert!(chunks.len() > 2);
        assert!(
            chunks
                .iter()
                .all(|c| c.text.chars().count() <= MAX_CHUNK_CHARS)
        );
        assert!(chunks[0].text.split(' ').count() < WINDOW_WORDS);

        // A single enormous word is cut.
        let huge = "z".repeat(MAX_CHUNK_CHARS * 3);
        let chunks = chunk_article(None, None, &huge);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text.chars().count(), MAX_CHUNK_CHARS);
    }

    #[test]
    fn short_or_missing_text_gets_the_metadata_chunk() {
        let tags = vec!["rust".to_string(), "memory".to_string()];
        let expected = "Borrowing in Rust\nHow references stay valid\nhttps://example.com/borrowing\nrust memory";
        for text in [None, Some(""), Some("A teaser of a paywalled page.")] {
            let chunks = source(text, &tags).chunks();
            assert_eq!(chunks.len(), 1, "{text:?}");
            assert_eq!(chunks[0].text, expected);
            assert_eq!(chunks[0].input, expected);
        }
        let long = words(0..100);
        assert!(long.chars().count() >= MIN_CHUNKED_TEXT_CHARS);
        let chunks = source(Some(&long), &tags).chunks();
        assert_eq!(chunks[0].text, long);

        let sparse = ChunkSource {
            title: None,
            description: Some(" "),
            url: "https://example.com/x",
            tags: &[],
            text: None,
        };
        assert_eq!(sparse.metadata_chunk().text, "https://example.com/x");
    }
}
