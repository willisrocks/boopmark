//! Member-only preview detection, for both paths that produce article text:
//! the direct fetch and the scraper fallback.
//!
//! Medium serves a teaser to a reader who is not signed in and marks it in
//! the page state it embeds (`window.__APOLLO_STATE__`): the current post's
//! entry has `"isLocked": true`. That state also holds *other* posts — the
//! cards of related stories, member-only ones included — so a marker anywhere
//! in the document is not enough: it is read only from the entry whose
//! `canonicalUrl` is this page. Without the state (any other site) the answer
//! is "not locked", because guessing would discard real text.

/// Where Medium puts the server-rendered store.
const STATE_MARKER: &str = "window.__APOLLO_STATE__";
/// The state's key prefix for a story.
const POST_KEY: &str = "\"Post:";
const LOCK_FIELD: &str = "\"isLocked\"";
const CANONICAL_FIELD: &str = "\"canonicalUrl\"";

/// True when `html` is a member-only preview of `page_url`.
pub fn is_locked_html(html: &str, page_url: &str) -> bool {
    let Some(state) = page_state(html) else {
        return false;
    };
    let mut search = 0;
    while let Some(offset) = state[search..].find(POST_KEY) {
        let key_at = search + offset;
        let Some(open) = state[key_at..].find('{') else {
            break;
        };
        let start = key_at + open;
        let Some(length) = balanced_object_len(&state[start..]) else {
            break;
        };
        let entry = &state[start..start + length];
        if canonical_url(entry).is_some_and(|url| same_page(url, page_url))
            && has_lock_marker(entry)
        {
            return true;
        }
        search = start + length;
    }
    false
}

/// The page-state object: from `window.__APOLLO_STATE__` to its closing brace.
fn page_state(html: &str) -> Option<&str> {
    let at = html.find(STATE_MARKER)?;
    let rest = &html[at..];
    let start = rest.find('{')?;
    let length = balanced_object_len(&rest[start..])?;
    Some(&rest[start..start + length])
}

/// Length of the JSON object at the start of `text`, string-aware, so braces
/// inside values do not end it early.
fn balanced_object_len(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `canonicalUrl` of one state entry.
fn canonical_url(entry: &str) -> Option<&str> {
    let at = entry.find(CANONICAL_FIELD)?;
    let rest = entry[at + CANONICAL_FIELD.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// A lock marker, tolerating the whitespace a serializer may leave around the
/// colon or the value: `"isLocked":true`, `"isLocked": true`, `"isLocked" : true`.
fn has_lock_marker(entry: &str) -> bool {
    let mut search = 0;
    while let Some(offset) = entry[search..].find(LOCK_FIELD) {
        let at = search + offset;
        let rest = entry[at + LOCK_FIELD.len()..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("true")
                && !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                return true;
            }
        }
        search = at + LOCK_FIELD.len();
    }
    false
}

/// Whether two URL strings name the same page: same host and path, with the
/// query and fragment ignored (a canonical URL rarely carries them).
fn same_page(a: &str, b: &str) -> bool {
    let parsed = |raw: &str| {
        url::Url::parse(raw).ok().map(|url| {
            (
                url.host_str().unwrap_or_default().to_ascii_lowercase(),
                url.path().trim_end_matches('/').to_string(),
            )
        })
    };
    match (parsed(a), parsed(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(entries: &str) -> String {
        format!("<html><script>window.__APOLLO_STATE__={{{entries}}}</script><p>teaser</p></html>")
    }

    const FREE: &str = "https://medium.com/@writer/a-free-story-1a2b3c4d5e6f";

    #[test]
    fn the_posts_own_locked_entry_is_a_preview() {
        let html = state(
            r#""Post:aaa":{"canonicalUrl":"https://medium.com/@writer/a-free-story-1a2b3c4d5e6f","isLocked":true}"#,
        );
        assert!(is_locked_html(&html, FREE));
    }

    /// Serde's pretty printing, and some proxies, put spaces around the colon.
    #[test]
    fn the_lock_marker_tolerates_whitespace() {
        for marker in [
            r#""isLocked": true"#,
            r#""isLocked" : true"#,
            r#""isLocked" :true"#,
        ] {
            let html = state(&format!(
                r#""Post:aaa":{{"canonicalUrl":"{FREE}",{marker}}}"#
            ));
            assert!(is_locked_html(&html, FREE), "{marker}");
        }
    }

    /// A free story's page embeds related member-only posts; their markers
    /// must not turn the free story into a preview.
    #[test]
    fn a_related_locked_post_does_not_mark_this_page() {
        let html = state(&format!(
            r#""Post:aaa":{{"canonicalUrl":"{FREE}","isLocked":false}},
               "Post:bbb":{{"canonicalUrl":"https://medium.com/@writer/another-member-story-9876543210ab","isLocked":true}}"#
        ));
        assert!(!is_locked_html(&html, FREE));
    }

    #[test]
    fn an_unlocked_entry_is_not_a_preview() {
        let html = state(&format!(
            r#""Post:aaa":{{"canonicalUrl":"{FREE}","isLocked":false}}"#
        ));
        assert!(!is_locked_html(&html, FREE));
    }

    /// A query string on either side still names the same page.
    #[test]
    fn the_canonical_url_matches_ignoring_the_query_and_a_trailing_slash() {
        let html = state(
            r#""Post:aaa":{"canonicalUrl":"https://medium.com/@writer/a-free-story-1a2b3c4d5e6f/","isLocked":true}"#,
        );
        assert!(is_locked_html(
            &html,
            "https://medium.com/@writer/a-free-story-1a2b3c4d5e6f?source=bookmark"
        ));
    }

    /// Another host with the same path is another page.
    #[test]
    fn another_host_is_not_this_page() {
        let html = state(
            r#""Post:aaa":{"canonicalUrl":"https://medium.com/@writer/a-free-story-1a2b3c4d5e6f","isLocked":true}"#,
        );
        assert!(!is_locked_html(
            &html,
            "https://example.test/@writer/a-free-story-1a2b3c4d5e6f"
        ));
    }

    /// No page state (any other site): never guess a preview.
    #[test]
    fn a_page_without_the_state_is_not_a_preview() {
        let html = r#"<html><script>{"isLocked":true}</script></html>"#;
        assert!(!is_locked_html(html, FREE));
    }

    /// Braces inside a value must not end the state early.
    #[test]
    fn braces_inside_a_value_do_not_end_the_state() {
        let html = state(&format!(
            r#""Post:aaa":{{"canonicalUrl":"{FREE}","preview":"a }} brace","isLocked":true}}"#
        ));
        assert!(is_locked_html(&html, FREE));
    }
}
