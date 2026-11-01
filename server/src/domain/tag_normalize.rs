//! Deterministic tag normalization (prototype for the tag-management spec).
//!
//! Three levels, each stricter about what it treats as "the same tag":
//! - L1 `tag_key`: case, surrounding whitespace, `-`/`_`/space runs, spaces
//!   around `/`. `AI Coding-Agents` and `ai coding agents` share a key.
//! - L2 `compact_key`: L1 with separators, `/` and `.` removed, so
//!   `self hosted`/`selfhosted`, `next.js`/`nextjs`, `CI/CD`/`cicd` match.
//! - L3 plural folding: a tag whose last word looks plural joins the group of
//!   its singular form, but only when that singular form already exists in
//!   the library. `kubernetes` never becomes `kubernete` and `news` never
//!   becomes `new`, because those singulars do not exist.
//!
//! The module has no crate dependencies so `examples/tag_lab.rs` can include
//! it by path. Nothing in the server calls it yet, hence the dead-code allow.
#![allow(dead_code)]

use std::collections::HashMap;

/// L1 key: lowercase, trimmed, `-`/`_`/whitespace runs collapsed to one
/// space, no spaces around `/`.
pub fn tag_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.trim().chars().flat_map(char::to_lowercase) {
        if ch == '-' || ch == '_' || ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if ch == '/' {
            pending_space = false;
            out.push('/');
            continue;
        }
        if pending_space && !out.ends_with('/') {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

/// L2 key: the L1 key without spaces, `/` and `.`.
pub fn compact_key(raw: &str) -> String {
    tag_key(raw)
        .chars()
        .filter(|c| !matches!(c, ' ' | '/' | '.'))
        .collect()
}

/// Singular candidates for the last word of an L1 key, most specific first.
/// Words of three characters or fewer are never folded (`ios`, `aws`, `css`).
fn singular_candidates(key: &str) -> Vec<String> {
    let (head, last) = match key.rfind(' ') {
        Some(i) => (&key[..=i], &key[i + 1..]),
        None => ("", key),
    };
    if last.chars().count() <= 3 || last.ends_with("ss") || !last.ends_with('s') {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(stem) = last.strip_suffix("ies") {
        out.push(format!("{head}{stem}y"));
    }
    if let Some(stem) = last.strip_suffix("es") {
        out.push(format!("{head}{stem}"));
    }
    out.push(format!("{head}{}", &last[..last.len() - 1]));
    out
}

/// One group of raw tags that normalization treats as one tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagGroup {
    /// The display form: the most-used raw spelling, ties broken by the
    /// spelling that sorts first.
    pub canonical: String,
    /// Every raw spelling in the group, sorted.
    pub members: Vec<String>,
}

/// Which levels `group_tags` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Case,
    Compact,
    Plural,
}

/// Group `(raw tag, bookmark count)` pairs by the requested normalization
/// level. Levels are cumulative: `Plural` also applies `Compact` and `Case`.
pub fn group_tags(tags: &[(String, i64)], level: Level) -> Vec<TagGroup> {
    let key_of = |raw: &str| match level {
        Level::Case => tag_key(raw),
        Level::Compact | Level::Plural => compact_key(raw),
    };

    // Union-find over keys.
    let mut parent: HashMap<String, String> = HashMap::new();
    fn find(parent: &mut HashMap<String, String>, k: &str) -> String {
        let p = parent.get(k).cloned().unwrap_or_else(|| k.to_string());
        if p == k {
            return p;
        }
        let root = find(parent, &p);
        parent.insert(k.to_string(), root.clone());
        root
    }
    for (raw, _) in tags {
        let k = key_of(raw);
        parent.entry(k.clone()).or_insert(k);
    }

    if level == Level::Plural {
        let keys: Vec<(String, String)> = tags
            .iter()
            .map(|(raw, _)| (tag_key(raw), key_of(raw)))
            .collect();
        for (l1, k) in &keys {
            for singular in singular_candidates(l1) {
                let sk = compact_key(&singular);
                if parent.contains_key(&sk) && sk != *k {
                    let a = find(&mut parent, k);
                    let b = find(&mut parent, &sk);
                    if a != b {
                        parent.insert(a, b);
                    }
                    break;
                }
            }
        }
    }

    let mut groups: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for (raw, count) in tags {
        let root = find(&mut parent, &key_of(raw));
        groups.entry(root).or_default().push((raw.clone(), *count));
    }

    let mut out: Vec<TagGroup> = groups
        .into_values()
        .map(|mut members| {
            members.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let canonical = members[0].0.clone();
            let mut names: Vec<String> = members.into_iter().map(|(n, _)| n).collect();
            names.sort();
            names.dedup();
            TagGroup {
                canonical,
                members: names,
            }
        })
        .collect();
    out.sort_by(|a, b| a.canonical.cmp(&b.canonical));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[(&str, i64)]) -> Vec<(String, i64)> {
        names.iter().map(|(n, c)| ((*n).to_string(), *c)).collect()
    }

    fn group_of<'a>(groups: &'a [TagGroup], raw: &str) -> &'a TagGroup {
        groups
            .iter()
            .find(|g| g.members.iter().any(|m| m == raw))
            .expect("raw tag is in some group")
    }

    #[test]
    fn key_folds_case_hyphen_underscore_and_spaces() {
        assert_eq!(tag_key("  AI Coding-Agents "), "ai coding agents");
        assert_eq!(tag_key("software_engineering"), "software engineering");
        assert_eq!(tag_key("local--AI"), "local ai");
    }

    #[test]
    fn key_keeps_slash_but_drops_spaces_around_it() {
        assert_eq!(tag_key("UI / UX"), "ui/ux");
        assert_eq!(tag_key("CI/CD"), "ci/cd");
    }

    #[test]
    fn compact_key_joins_split_words_and_dotted_names() {
        assert_eq!(compact_key("self hosted"), compact_key("selfhosted"));
        assert_eq!(compact_key("Next.js"), compact_key("nextjs"));
        assert_eq!(compact_key("CI/CD"), compact_key("CICD"));
        assert_ne!(compact_key("c++"), compact_key("c"));
        assert_ne!(compact_key("c#"), compact_key("c"));
    }

    #[test]
    fn case_level_merges_only_case_and_separator_variants() {
        let groups = group_tags(
            &tags(&[("GitHub", 9), ("github", 3), ("git hub", 1), ("rust", 5)]),
            Level::Case,
        );
        assert_eq!(group_of(&groups, "github").members, ["GitHub", "github"]);
        assert_eq!(group_of(&groups, "git hub").members, ["git hub"]);
    }

    #[test]
    fn canonical_is_most_used_spelling() {
        let groups = group_tags(&tags(&[("GitHub", 9), ("github", 3)]), Level::Case);
        assert_eq!(group_of(&groups, "github").canonical, "GitHub");
    }

    #[test]
    fn plural_folds_only_into_an_existing_singular() {
        let groups = group_tags(
            &tags(&[
                ("coding agents", 4),
                ("coding-agent", 2),
                ("kubernetes", 7),
                ("news", 3),
                ("ios", 2),
                ("physics", 1),
                ("libraries", 1),
                ("library", 2),
            ]),
            Level::Plural,
        );
        assert_eq!(
            group_of(&groups, "coding-agent").members,
            ["coding agents", "coding-agent"]
        );
        assert_eq!(group_of(&groups, "libraries").members, ["libraries", "library"]);
        assert_eq!(group_of(&groups, "kubernetes").members, ["kubernetes"]);
        assert_eq!(group_of(&groups, "news").members, ["news"]);
        assert_eq!(group_of(&groups, "ios").members, ["ios"]);
    }

    #[test]
    fn plural_never_folds_double_s_or_short_words() {
        let groups = group_tags(
            &tags(&[("process", 1), ("proces", 1), ("aws", 1), ("aw", 1)]),
            Level::Plural,
        );
        assert_eq!(group_of(&groups, "process").members, ["process"]);
        assert_eq!(group_of(&groups, "aws").members, ["aws"]);
    }
}
