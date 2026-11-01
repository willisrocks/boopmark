//! Read models for the filter bar and the `/tags` library page: canonical
//! tags with their counts and aliases, the top tags by use, and the
//! alias-aware tag search.
//!
//! Bookmarks store canonical names, so the tags on bookmarks are the
//! canonical tags. Alias rows add the other spellings that resolve to each
//! one, and a canonical that only lives in the alias table (its bookmarks
//! were deleted) is listed with a count of zero.

use crate::domain::tag_alias::TagAlias;
use crate::domain::tag_normalize::compact_key;
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// One canonical tag: its bookmark count and the other spellings that
/// resolve to it, sorted A–Z.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    pub name: String,
    pub count: i64,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagSort {
    MostUsed,
    Az,
    LeastUsed,
}

impl TagSort {
    /// Unknown values fall back to the default, most used.
    pub fn parse(value: &str) -> Self {
        match value {
            "az" => Self::Az,
            "least" => Self::LeastUsed,
            _ => Self::MostUsed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MostUsed => "most",
            Self::Az => "az",
            Self::LeastUsed => "least",
        }
    }
}

fn by_name(a: &TagRow, b: &TagRow) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
}

fn compare(sort: TagSort, a: &TagRow, b: &TagRow) -> Ordering {
    match sort {
        TagSort::MostUsed => b.count.cmp(&a.count).then_with(|| by_name(a, b)),
        TagSort::Az => by_name(a, b),
        TagSort::LeastUsed => a.count.cmp(&b.count).then_with(|| by_name(a, b)),
    }
}

/// Build the library from the tags on bookmarks and the alias table,
/// sorted by most used.
pub fn library_rows(tag_counts: &[(String, i64)], aliases: &[TagAlias]) -> Vec<TagRow> {
    let mut rows: BTreeMap<&str, TagRow> = tag_counts
        .iter()
        .map(|(name, count)| {
            (
                name.as_str(),
                TagRow {
                    name: name.clone(),
                    count: *count,
                    aliases: Vec::new(),
                },
            )
        })
        .collect();
    for alias in aliases {
        let row = rows
            .entry(alias.canonical.as_str())
            .or_insert_with(|| TagRow {
                name: alias.canonical.clone(),
                count: 0,
                aliases: Vec::new(),
            });
        if alias.alias != row.name && !row.aliases.contains(&alias.alias) {
            row.aliases.push(alias.alias.clone());
        }
    }
    let mut rows: Vec<TagRow> = rows.into_values().collect();
    for row in &mut rows {
        row.aliases
            .sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    }
    sort_rows(&mut rows, TagSort::MostUsed);
    rows
}

pub fn sort_rows(rows: &mut [TagRow], sort: TagSort) {
    rows.sort_by(|a, b| compare(sort, a, b));
}

/// The `n` most used tags with bookmarks, skipping the active ones (they
/// are shown separately, first).
pub fn top_tags<'a>(rows: &'a [TagRow], n: usize, active: &[String]) -> Vec<&'a TagRow> {
    let mut used: Vec<&TagRow> = rows
        .iter()
        .filter(|r| r.count > 0 && !active.contains(&r.name))
        .collect();
    used.sort_by(|a, b| compare(TagSort::MostUsed, a, b));
    used.truncate(n);
    used
}

/// A search query, lowercased and as a compact key, so `k 8 s` and `K8s`
/// match the same spellings and `nextjs` matches `next.js`.
struct Query {
    lower: String,
    compact: String,
}

impl Query {
    fn new(raw: &str) -> Self {
        Self {
            lower: raw.trim().to_lowercase(),
            compact: compact_key(raw),
        }
    }

    fn is_empty(&self) -> bool {
        self.lower.is_empty()
    }

    fn matches(&self, spelling: &str) -> bool {
        spelling.to_lowercase().contains(&self.lower)
            || (!self.compact.is_empty() && compact_key(spelling).contains(&self.compact))
    }

    /// 0: the name starts with the query; 1: the name contains it;
    /// 2: only an alias matches.
    fn rank(&self, row: &TagRow) -> Option<u8> {
        if row.name.to_lowercase().starts_with(&self.lower) {
            Some(0)
        } else if self.matches(&row.name) {
            Some(1)
        } else if row.aliases.iter().any(|a| self.matches(a)) {
            Some(2)
        } else {
            None
        }
    }
}

/// Rows whose name or any alias contains `query`. An empty query keeps
/// every row.
pub fn filter_rows(rows: Vec<TagRow>, query: &str) -> Vec<TagRow> {
    let query = Query::new(query);
    if query.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter(|row| query.rank(row).is_some())
        .collect()
}

/// The filter popover's search over tags with bookmarks: name prefix
/// first, then name substring, then alias matches, each by count. An empty
/// query lists the most used tags.
pub fn search<'a>(rows: &'a [TagRow], query: &str, limit: usize) -> Vec<&'a TagRow> {
    let query = Query::new(query);
    let mut hits: Vec<(u8, &TagRow)> = rows
        .iter()
        .filter(|r| r.count > 0)
        .filter_map(|r| {
            if query.is_empty() {
                Some((0, r))
            } else {
                query.rank(r).map(|rank| (rank, r))
            }
        })
        .collect();
    hits.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then_with(|| compare(TagSort::MostUsed, a, b)));
    hits.truncate(limit);
    hits.into_iter().map(|(_, r)| r).collect()
}

/// Up to `max` aliases to show next to a tag, the ones matching `query`
/// first, plus how many more there are.
pub fn alias_preview<'a>(row: &'a TagRow, query: &str, max: usize) -> (Vec<&'a str>, usize) {
    let query = Query::new(query);
    let (mut shown, rest): (Vec<&str>, Vec<&str>) = row
        .aliases
        .iter()
        .map(String::as_str)
        .partition(|a| !query.is_empty() && query.matches(a));
    shown.extend(rest);
    let more = shown.len().saturating_sub(max);
    shown.truncate(max);
    (shown, more)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tag_alias::AliasSource;

    fn alias(spelling: &str, canonical: &str) -> TagAlias {
        TagAlias {
            alias_key: compact_key(spelling),
            alias: spelling.into(),
            canonical: canonical.into(),
            source: AliasSource::Manual,
            batch_id: None,
        }
    }

    fn counts(values: &[(&str, i64)]) -> Vec<(String, i64)> {
        values.iter().map(|(t, c)| ((*t).to_string(), *c)).collect()
    }

    fn names(rows: &[&TagRow]) -> Vec<String> {
        rows.iter().map(|r| r.name.clone()).collect()
    }

    fn library() -> Vec<TagRow> {
        library_rows(
            &counts(&[
                ("kubernetes", 7),
                ("rust", 12),
                ("GitHub", 18),
                ("coding agents", 18),
                ("next.js", 3),
                ("docker", 7),
            ]),
            &[
                alias("k8s", "kubernetes"),
                alias("kubernetes", "kubernetes"),
                alias("github", "GitHub"),
                alias("git hub", "GitHub"),
                alias("coding-agent", "coding agents"),
                alias("AI coding agents", "coding agents"),
                alias("old tag", "retired"),
            ],
        )
    }

    #[test]
    fn rows_join_counts_and_aliases_and_keep_alias_only_canonicals() {
        let rows = library();
        let summary: Vec<(&str, i64, Vec<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.count,
                    r.aliases.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "coding agents",
                    18,
                    vec!["AI coding agents", "coding-agent"]
                ),
                ("GitHub", 18, vec!["git hub", "github"]),
                ("rust", 12, vec![]),
                ("docker", 7, vec![]),
                ("kubernetes", 7, vec!["k8s"]),
                ("next.js", 3, vec![]),
                ("retired", 0, vec!["old tag"]),
            ]
        );
    }

    #[test]
    fn sorts_by_name_and_least_used() {
        let mut rows = library();
        sort_rows(&mut rows, TagSort::Az);
        let az: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            az,
            vec![
                "coding agents",
                "docker",
                "GitHub",
                "kubernetes",
                "next.js",
                "retired",
                "rust"
            ]
        );
        sort_rows(&mut rows, TagSort::LeastUsed);
        assert_eq!(rows[0].name, "retired");
        assert_eq!(rows[1].name, "next.js");
        assert_eq!(TagSort::parse("least"), TagSort::LeastUsed);
        assert_eq!(TagSort::parse("bogus"), TagSort::MostUsed);
    }

    #[test]
    fn top_tags_skip_active_and_unused_tags_and_break_ties_by_name() {
        let rows = library();
        assert_eq!(
            names(&top_tags(&rows, 3, &[])),
            vec!["coding agents", "GitHub", "rust"]
        );
        assert_eq!(
            names(&top_tags(&rows, 3, &["GitHub".to_string()])),
            vec!["coding agents", "rust", "docker"]
        );
        assert_eq!(top_tags(&rows, 100, &[]).len(), 6);
    }

    #[test]
    fn search_finds_a_tag_by_its_alias() {
        let rows = library();
        assert_eq!(names(&search(&rows, "k8s", 20)), vec!["kubernetes"]);
        assert_eq!(names(&search(&rows, "K8S", 20)), vec!["kubernetes"]);
        assert_eq!(names(&search(&rows, "githu", 20)), vec!["GitHub"]);
    }

    #[test]
    fn search_matches_compact_keys() {
        let rows = library();
        assert_eq!(names(&search(&rows, "nextjs", 20)), vec!["next.js"]);
        assert_eq!(
            names(&search(&rows, "coding-agents", 20)),
            vec!["coding agents"]
        );
    }

    #[test]
    fn search_ranks_prefix_then_substring_then_alias() {
        let rows = library_rows(
            &counts(&[
                ("ai agents", 15),
                ("coding agents", 18),
                ("agentic", 2),
                ("llm", 9),
            ]),
            &[alias("agent frameworks", "llm")],
        );
        assert_eq!(
            names(&search(&rows, "agent", 20)),
            vec!["agentic", "coding agents", "ai agents", "llm"]
        );
        assert_eq!(search(&rows, "agent", 2).len(), 2);
    }

    #[test]
    fn empty_search_lists_used_tags_by_count() {
        let rows = library();
        let hits = search(&rows, "  ", 3);
        assert_eq!(names(&hits), vec!["coding agents", "GitHub", "rust"]);
        assert!(search(&rows, "old tag", 20).is_empty());
    }

    #[test]
    fn filter_rows_matches_names_and_aliases() {
        let filtered = filter_rows(library(), "hub");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "GitHub");
        assert_eq!(filter_rows(library(), "old")[0].name, "retired");
        assert_eq!(filter_rows(library(), "").len(), 7);
    }

    #[test]
    fn alias_preview_puts_matches_first_and_counts_the_rest() {
        let row = TagRow {
            name: "ai agents".into(),
            count: 15,
            aliases: vec![
                "agentic ai".into(),
                "AI bots".into(),
                "Agentic-AI".into(),
                "autonomous agents".into(),
            ],
        };
        assert_eq!(
            alias_preview(&row, "", 2),
            (vec!["agentic ai", "AI bots"], 2)
        );
        assert_eq!(alias_preview(&row, "bot", 1), (vec!["AI bots"], 3));
    }
}
