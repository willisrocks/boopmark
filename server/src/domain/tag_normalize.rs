//! Deterministic tag normalization.
//!
//! Three levels, each looser about what it treats as "the same tag":
//! - L1 `tag_key`: case, surrounding whitespace, `-`/`_`/space runs, spaces
//!   around `/`. `AI Coding-Agents` and `ai coding agents` share a key.
//! - L2 `compact_key`: L1 with spaces, `/` and in-word `.` removed, so
//!   `self hosted`/`selfhosted`, `next.js`/`nextjs`, `CI/CD`/`cicd` match.
//!   Symbols that change meaning are kept: `c#`, `c++` and `.net` (a leading
//!   dot) never collapse into `c` or `net`.
//! - L3 plural folding: a tag whose last word looks plural joins the group of
//!   its singular form, but only when that singular form already exists in
//!   the library. `kubernetes` never becomes `kubernete` and `news` never
//!   becomes `new`, because those singulars do not exist.
//!
//! Slash tags are not split: `UI/UX` folds with `ui-ux` and `ui ux`, never
//! with `ux`. Whether those mean the same is left to a reviewed merge.

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

/// L2 key: the L1 key without spaces and `/`, and without `.` inside a word.
/// A `.` that starts a word is kept so `.net` stays distinct from `net`.
pub fn compact_key(raw: &str) -> String {
    let key = tag_key(raw);
    let mut out = String::with_capacity(key.len());
    let mut word_start = true;
    for ch in key.chars() {
        match ch {
            ' ' | '/' => word_start = true,
            '.' if !word_start => {}
            _ => {
                out.push(ch);
                word_start = false;
            }
        }
    }
    out
}

/// Singular candidates for the last word of an L1 key, most likely first.
///
/// Only alphabetic words longer than three characters that end in a single
/// `s` are considered, so `ios`, `aws`, `css`, `k8s`, `2010s` and `process`
/// never fold. `-es` is only stripped after a sibilant (`boxes`, `classes`,
/// `matches`), so `notes` cannot fold into `not`.
pub fn singular_candidates(key: &str) -> Vec<String> {
    let (head, last) = match key.rfind([' ', '/']) {
        Some(i) => (&key[..=i], &key[i + 1..]),
        None => ("", key),
    };
    if last.chars().count() <= 3
        || !last.chars().all(char::is_alphabetic)
        || !last.ends_with('s')
        || last.ends_with("ss")
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(stem) = last.strip_suffix("ies") {
        out.push(format!("{head}{stem}y"));
    }
    out.push(format!("{head}{}", &last[..last.len() - 1]));
    if let Some(stem) = last.strip_suffix("es")
        && ["s", "x", "z", "ch", "sh"]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
    {
        out.push(format!("{head}{stem}"));
    }
    out
}

/// The compact keys a tag folds into under L3, given a predicate telling
/// which compact keys already exist. Returns the first existing singular.
pub fn existing_singular(raw: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    let own = compact_key(raw);
    singular_candidates(&tag_key(raw))
        .into_iter()
        .map(|singular| compact_key(&singular))
        .find(|sk| *sk != own && exists(sk))
}

/// Trim a tag as written by a client. Empty tags and tags containing control
/// characters are not tags; they return `None`.
pub fn clean_tag(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    Some(trimmed)
}

/// Pick the display form for a group of spellings with their bookmark counts.
///
/// 1. Spellings written with spaces win over ones with `-` or `_`.
/// 2. The all-lowercase spellings win, unless deliberately cased spellings
///    (a capital after a word's first letter: `AWS`, `GitHub`, `macOS`) have
///    more uses. Title or sentence case (`Rust`, `Coding Agents`) is treated
///    as incidental and never beats a lowercase spelling.
/// 3. Then the most used, then the shortest, then the first in sort order.
///
/// Panics on an empty slice; every group has at least one member.
pub fn choose_display(spellings: &[(String, i64)]) -> String {
    let spaced: Vec<&(String, i64)> = spellings
        .iter()
        .filter(|(s, _)| !s.contains(['-', '_']))
        .collect();
    let pool: Vec<&(String, i64)> = if spaced.is_empty() {
        spellings.iter().collect()
    } else {
        spaced
    };

    let uses = |keep: fn(&str) -> bool| -> i64 {
        pool.iter().filter(|(s, _)| keep(s)).map(|(_, c)| c).sum()
    };
    let lower_uses = uses(is_lowercase);
    let keep: fn(&str) -> bool = if uses(is_deliberately_cased) > lower_uses {
        is_deliberately_cased
    } else if lower_uses > 0 {
        is_lowercase
    } else {
        |_| true
    };

    pool.into_iter()
        .filter(|(s, _)| keep(s))
        .min_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.chars().count().cmp(&b.0.chars().count()))
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|(s, _)| s.clone())
        .expect("choose_display needs at least one spelling")
}

fn is_lowercase(s: &str) -> bool {
    !s.chars().any(char::is_uppercase)
}

/// True when some word has a capital after its first letter (`AWS`,
/// `GitHub`, `macOS`, `iOS`), which signals an acronym or brand.
fn is_deliberately_cased(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .any(|word| word.chars().skip(1).any(char::is_uppercase))
}

/// One group of raw tags that normalization treats as one tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagGroup {
    /// The display form chosen by [`choose_display`].
    pub canonical: String,
    /// Every raw spelling in the group, sorted.
    pub members: Vec<String>,
    /// Bookmark uses summed over the members.
    pub count: i64,
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
/// Groups are sorted by canonical name.
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
        let keys: Vec<String> = parent.keys().cloned().collect();
        for (raw, _) in tags {
            if let Some(sk) = existing_singular(raw, |k| keys.iter().any(|e| e == k)) {
                let a = find(&mut parent, &key_of(raw));
                let b = find(&mut parent, &sk);
                if a != b {
                    parent.insert(a, b);
                }
            }
        }
    }

    let mut groups: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for (raw, count) in tags {
        let root = find(&mut parent, &key_of(raw));
        let members = groups.entry(root).or_default();
        match members.iter_mut().find(|(m, _)| m == raw) {
            Some(existing) => existing.1 += count,
            None => members.push((raw.clone(), *count)),
        }
    }

    let mut out: Vec<TagGroup> = groups
        .into_values()
        .map(|members| {
            let canonical = choose_display(&members);
            let count = members.iter().map(|(_, c)| c).sum();
            let mut names: Vec<String> = members.into_iter().map(|(n, _)| n).collect();
            names.sort();
            TagGroup {
                canonical,
                members: names,
                count,
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
        assert_eq!(tag_key("-leading and trailing-"), "leading and trailing");
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
        assert_eq!(compact_key("ASP.NET"), compact_key("aspnet"));
    }

    #[test]
    fn compact_key_keeps_symbols_that_change_meaning() {
        assert_ne!(compact_key("c++"), compact_key("c"));
        assert_ne!(compact_key("c#"), compact_key("c"));
        assert_ne!(compact_key("c#"), compact_key("c++"));
        assert_ne!(compact_key(".NET"), compact_key("net"));
        assert_eq!(compact_key(".NET"), compact_key(".net"));
        assert_eq!(compact_key("C#"), compact_key("c#"));
    }

    #[test]
    fn screenshot_duplicates_share_a_key() {
        for (a, b) in [
            ("ai coding agents", "AI coding agents"),
            ("github", "GitHub"),
            ("rust", "Rust"),
            ("software engineering", "software-engineering"),
            ("local ai", "local-AI"),
            ("UI/UX", "ui-ux"),
            ("UI/UX", "ui ux"),
        ] {
            assert_eq!(compact_key(a), compact_key(b), "{a} vs {b}");
        }
        assert_ne!(compact_key("UI/UX"), compact_key("ux"));
    }

    #[test]
    fn screenshot_list_groups_without_wrong_merges() {
        let groups = group_tags(
            &tags(&[
                ("ai coding agents", 3),
                ("AI coding agents", 2),
                ("coding-agent", 2),
                ("coding agents", 4),
                ("github", 3),
                ("GitHub", 9),
                ("rust", 5),
                ("Rust", 1),
                ("software engineering", 2),
                ("software-engineering", 1),
                ("local ai", 1),
                ("local-AI", 1),
                ("UI/UX", 2),
                ("ux", 1),
            ]),
            Level::Plural,
        );
        assert_eq!(
            group_of(&groups, "AI coding agents").members,
            ["AI coding agents", "ai coding agents"]
        );
        assert_eq!(
            group_of(&groups, "coding-agent").members,
            ["coding agents", "coding-agent"]
        );
        assert_eq!(group_of(&groups, "github").members, ["GitHub", "github"]);
        assert_eq!(group_of(&groups, "rust").members, ["Rust", "rust"]);
        assert_eq!(
            group_of(&groups, "software engineering").members,
            ["software engineering", "software-engineering"]
        );
        assert_eq!(
            group_of(&groups, "local ai").members,
            ["local ai", "local-AI"]
        );
        assert_eq!(group_of(&groups, "UI/UX").members, ["UI/UX"]);
        assert_eq!(group_of(&groups, "ux").members, ["ux"]);
        assert_eq!(groups.len(), 8);
    }

    #[test]
    fn case_level_merges_only_case_and_separator_variants() {
        let groups = group_tags(
            &tags(&[("GitHub", 9), ("github", 3), ("git hub", 1), ("rust", 5)]),
            Level::Case,
        );
        assert_eq!(group_of(&groups, "github").members, ["GitHub", "github"]);
        assert_eq!(group_of(&groups, "git hub").members, ["git hub"]);
        let compact = group_tags(
            &tags(&[("GitHub", 9), ("github", 3), ("git hub", 1)]),
            Level::Compact,
        );
        assert_eq!(compact.len(), 1);
        assert_eq!(compact[0].count, 13);
    }

    #[test]
    fn display_keeps_case_when_mostly_written_cased() {
        assert_eq!(
            choose_display(&tags(&[("GitHub", 9), ("github", 3)])),
            "GitHub"
        );
        assert_eq!(choose_display(&tags(&[("AWS", 4), ("aws", 1)])), "AWS");
        assert_eq!(choose_display(&tags(&[("macOS", 2)])), "macOS");
        assert_eq!(
            choose_display(&tags(&[("AI coding agents", 3), ("ai coding agents", 2)])),
            "AI coding agents"
        );
    }

    #[test]
    fn display_prefers_lowercase_when_lowercase_is_common() {
        assert_eq!(choose_display(&tags(&[("Rust", 1), ("rust", 5)])), "rust");
        assert_eq!(choose_display(&tags(&[("Rust", 9), ("rust", 1)])), "rust");
        assert_eq!(choose_display(&tags(&[("Rust", 2)])), "Rust");
        assert_eq!(
            choose_display(&tags(&[("Startup", 2), ("startup", 2)])),
            "startup"
        );
        assert_eq!(
            choose_display(&tags(&[
                ("Coding Agents", 3),
                ("coding agents", 2),
                ("coding agent", 1)
            ])),
            "coding agents"
        );
    }

    #[test]
    fn display_prefers_spaces_over_hyphens_and_underscores() {
        assert_eq!(
            choose_display(&tags(&[
                ("software-engineering", 9),
                ("software engineering", 1)
            ])),
            "software engineering"
        );
        assert_eq!(
            choose_display(&tags(&[("AI-art", 5), ("ai art", 1)])),
            "ai art"
        );
        assert_eq!(
            choose_display(&tags(&[("ui-ux", 1), ("ui_ux", 1)])),
            "ui-ux"
        );
    }

    #[test]
    fn display_tie_breaks_on_shortest_then_alphabetical() {
        assert_eq!(
            choose_display(&tags(&[("selfhosted", 1), ("self hosted", 1)])),
            "selfhosted"
        );
        assert_eq!(choose_display(&tags(&[("b", 1), ("a", 1)])), "a");
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
                ("analytics", 1),
                ("libraries", 1),
                ("library", 2),
                ("databases", 1),
                ("database", 1),
                ("boxes", 1),
                ("box", 1),
            ]),
            Level::Plural,
        );
        assert_eq!(
            group_of(&groups, "coding-agent").members,
            ["coding agents", "coding-agent"]
        );
        assert_eq!(
            group_of(&groups, "libraries").members,
            ["libraries", "library"]
        );
        assert_eq!(
            group_of(&groups, "databases").members,
            ["database", "databases"]
        );
        assert_eq!(group_of(&groups, "boxes").members, ["box", "boxes"]);
        for alone in ["kubernetes", "news", "ios", "physics", "analytics"] {
            assert_eq!(group_of(&groups, alone).members, [alone]);
        }
    }

    #[test]
    fn plural_never_folds_double_s_short_or_non_alphabetic_words() {
        let groups = group_tags(
            &tags(&[
                ("process", 1),
                ("proces", 1),
                ("aws", 1),
                ("aw", 1),
                ("k8s", 1),
                ("k8", 1),
                ("2010s", 1),
                ("2010", 1),
                ("notes", 1),
                ("not", 1),
            ]),
            Level::Plural,
        );
        assert_eq!(groups.len(), 10);
    }

    #[test]
    fn plural_folds_the_last_word_after_a_slash() {
        assert_eq!(singular_candidates("ui/tools"), ["ui/tool"]);
        assert_eq!(singular_candidates("web apis"), ["web api"]);
        assert_eq!(singular_candidates("movies"), ["movy", "movie"]);
        assert_eq!(singular_candidates("classes"), ["classe", "class"]);
    }

    #[test]
    fn clean_tag_trims_and_rejects_empty_or_control_characters() {
        assert_eq!(clean_tag("  rust "), Some("rust"));
        assert_eq!(clean_tag("   "), None);
        assert_eq!(clean_tag("bad\u{0007}tag"), None);
        assert_eq!(clean_tag("line\nbreak"), None);
    }
}
