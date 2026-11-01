//! Canonical tags, aliases, merge proposals and undoable change batches.
//!
//! Bookmarks keep storing canonical tag names in `bookmarks.tags`. The alias
//! table remembers every spelling seen, keyed by [`compact_key`], so a new
//! spelling of an existing tag resolves to it on the next write.
//!
//! Existing tags are never merged automatically. The normalization job only
//! writes [`TagMergeProposal`]s; a merge happens when the user applies one
//! (or merges by hand), and every merge is recorded in a [`TagChangeBatch`]
//! that can be undone as a unit.
//!
//! The functions here are pure; the repository runs them inside its
//! transactions so that the rules are tested once, without a database.

use crate::domain::error::DomainError;
use crate::domain::tag_normalize::{
    Level, choose_display, clean_tag, compact_key, existing_singular, group_tags,
    singular_candidates, tag_key,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Result<Self, DomainError> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    other => Err(DomainError::Internal(format!(
                        concat!("unknown ", stringify!($name), ": {}"),
                        other
                    ))),
                }
            }
        }
    };
}

string_enum!(
    /// Who decided that a spelling belongs to a canonical tag.
    AliasSource {
        Normalize => "normalize",
        Llm => "llm",
        Manual => "manual",
    }
);

string_enum!(
    /// Where a merge proposal came from.
    ProposalSource {
        Deterministic => "deterministic",
        Llm => "llm",
    }
);

string_enum!(
    ProposalStatus {
        Pending => "pending",
        Applied => "applied",
        Dismissed => "dismissed",
    }
);

string_enum!(
    Confidence {
        High => "high",
        Medium => "medium",
        Low => "low",
    }
);

string_enum!(
    /// What a change batch did; shown in the history list.
    ChangeKind {
        Normalize => "normalize",
        LlmMerge => "llm_merge",
        Rename => "rename",
        Merge => "merge",
        Delete => "delete",
    }
);

impl ChangeKind {
    /// The alias source recorded for spellings a batch of this kind maps.
    pub fn alias_source(self) -> AliasSource {
        match self {
            Self::Normalize => AliasSource::Normalize,
            Self::LlmMerge => AliasSource::Llm,
            Self::Rename | Self::Merge | Self::Delete => AliasSource::Manual,
        }
    }
}

/// One known spelling of a user's tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagAlias {
    /// `compact_key(alias)`; unique per user.
    pub alias_key: String,
    /// The spelling seen, for display ("also: GitHub").
    pub alias: String,
    /// The tag stored on bookmarks.
    pub canonical: String,
    pub source: AliasSource,
    /// The change batch that created or last repointed this alias.
    pub batch_id: Option<Uuid>,
}

/// An alias the write path wants to remember. Inserted only when the key is
/// not known yet, so a concurrent writer's choice wins consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTagAlias {
    pub alias_key: String,
    pub alias: String,
    pub canonical: String,
}

/// "Merge these spellings into `canonical`." `members` may include the
/// canonical itself; every other member is rewritten to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagMerge {
    pub canonical: String,
    pub members: Vec<String>,
}

/// A merge suggested by the normalization job or the LLM review step.
/// Nothing changes until it is applied with `apply_merge`.
#[derive(Debug, Clone, PartialEq)]
pub struct TagMergeProposal {
    pub id: Uuid,
    pub user_id: Uuid,
    pub canonical: String,
    pub members: Vec<String>,
    pub source: ProposalSource,
    pub confidence: Confidence,
    pub status: ProposalStatus,
    /// The batch that applied it, while it is applied.
    pub batch_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

impl TagMergeProposal {
    pub fn merge(&self) -> TagMerge {
        TagMerge {
            canonical: self.canonical.clone(),
            members: self.members.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTagMergeProposal {
    pub canonical: String,
    pub members: Vec<String>,
    pub confidence: Confidence,
}

/// One applied rename/merge, the unit of undo.
#[derive(Debug, Clone, PartialEq)]
pub struct TagChangeBatch {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: ChangeKind,
    pub summary: TagChangeSummary,
    pub undone_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Stored as `tag_change_batches.summary` for the history list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagChangeSummary {
    pub merges: Vec<TagMerge>,
    pub bookmarks_changed: u64,
}

/// What the write path knows about a user's tags: alias rows plus the
/// distinct tags already on bookmarks. The second source matters because
/// existing libraries are not backfilled into `tag_aliases`.
#[derive(Debug, Default, Clone)]
pub struct TagIndex {
    /// Compact key -> canonical, from alias rows and this write's results.
    aliases: HashMap<String, String>,
    existing: BTreeMap<String, Vec<(String, i64)>>,
    /// Singular compact key -> compact key of a known plural spelling, so
    /// a new singular finds an existing plural in one lookup.
    plurals: HashMap<String, String>,
}

impl TagIndex {
    pub fn new(aliases: &[TagAlias], tag_counts: &[(String, i64)]) -> Self {
        let mut index = Self::default();
        for alias in aliases {
            index.remember(&alias.alias_key, &alias.alias, &alias.canonical);
        }
        for (tag, count) in tag_counts {
            let key = compact_key(tag);
            index.index_plural(tag, &key);
            index
                .existing
                .entry(key)
                .or_default()
                .push((tag.clone(), *count));
        }
        index
    }

    fn remember(&mut self, key: &str, spelling: &str, canonical: &str) {
        self.aliases.insert(key.to_string(), canonical.to_string());
        self.index_plural(spelling, key);
    }

    fn index_plural(&mut self, spelling: &str, key: &str) {
        for singular in singular_candidates(&tag_key(spelling)) {
            let singular = compact_key(&singular);
            if singular != key {
                self.plurals
                    .entry(singular)
                    .or_insert_with(|| key.to_string());
            }
        }
    }

    fn contains_key(&self, key: &str) -> bool {
        self.aliases.contains_key(key) || self.existing.contains_key(key)
    }

    /// The canonical tag for a compact key: the alias row if there is one,
    /// otherwise the display form of the existing spellings with that key.
    pub(crate) fn canonical_for_key(&self, key: &str) -> Option<String> {
        self.aliases
            .get(key)
            .cloned()
            .or_else(|| self.existing.get(key).map(|s| choose_display(s)))
    }

    fn is_existing_spelling(&self, key: &str, raw: &str) -> bool {
        self.existing
            .get(key)
            .is_some_and(|spellings| spellings.iter().any(|(s, _)| s == raw))
    }
}

/// The outcome of resolving one written tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub canonical: String,
    /// An alias to remember, if the key was not known as an alias yet.
    pub record: Option<NewTagAlias>,
}

/// Resolve one cleaned tag against the index, in the spec's lookup order:
///
/// 1. an alias row for `compact_key(raw)`
/// 2. the exact spelling already on a bookmark (kept as is)
/// 3. another spelling on a bookmark with the same compact key
/// 4. L3: an existing singular of `raw`, or an existing plural whose
///    singular is `raw`
/// 5. otherwise a new tag, stored as typed
///
/// The index learns the result, so later tags in the same write see it.
/// A key with several existing spellings is a duplicate awaiting review:
/// it resolves to one of those spellings but is never pinned as an alias.
pub fn resolve_tag(raw: &str, index: &mut TagIndex) -> Resolution {
    let key = compact_key(raw);
    if let Some(canonical) = index.aliases.get(&key) {
        return Resolution {
            canonical: canonical.clone(),
            record: None,
        };
    }

    let canonical = if index.is_existing_spelling(&key, raw) {
        raw.to_string()
    } else if let Some(canonical) = index.canonical_for_key(&key) {
        canonical
    } else if let Some(singular) = existing_singular(raw, |k| index.contains_key(k)) {
        index
            .canonical_for_key(&singular)
            .expect("existing_singular only returns known keys")
    } else if let Some(plural_key) = index.plurals.get(&key) {
        index
            .canonical_for_key(plural_key)
            .expect("plurals only holds known keys")
    } else {
        raw.to_string()
    };

    if index.existing.get(&key).is_some_and(|s| s.len() > 1) {
        return Resolution {
            canonical,
            record: None,
        };
    }
    index.remember(&key, raw, &canonical);
    Resolution {
        record: Some(NewTagAlias {
            alias_key: key,
            alias: raw.to_string(),
            canonical: canonical.clone(),
        }),
        canonical,
    }
}

/// Resolve the tags of one write. Tags in `keep` (the bookmark's current
/// tags, on an edit) are kept verbatim, compared before any trimming, so
/// re-saving never rewrites existing data. Other empty and control-character
/// tags are dropped, and duplicates after resolution are removed, keeping
/// the first.
pub fn resolve_tags(
    raw: &[String],
    keep: &[String],
    index: &mut TagIndex,
) -> (Vec<String>, Vec<NewTagAlias>) {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    let mut records = Vec::new();
    for tag in raw {
        let canonical = if keep.contains(tag) {
            tag.clone()
        } else {
            let Some(tag) = clean_tag(tag) else { continue };
            let resolution = resolve_tag(tag, index);
            records.extend(resolution.record);
            resolution.canonical
        };
        if !out.contains(&canonical) {
            out.push(canonical);
        }
    }
    (out, records)
}

/// The tags of one bookmark write: as submitted, and the ones already on
/// the bookmark (empty for a new bookmark), which are never rewritten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagWrite {
    pub tags: Vec<String>,
    pub keep: Vec<String>,
}

/// Resolve several writes against one index, in order, so an import sees
/// the spellings earlier records introduced. Returns the resolved tags per
/// write and every alias to remember.
pub fn resolve_writes(
    writes: &[TagWrite],
    index: &mut TagIndex,
) -> (Vec<Vec<String>>, Vec<NewTagAlias>) {
    let mut records = Vec::new();
    let resolved = writes
        .iter()
        .map(|write| {
            let (tags, new) = resolve_tags(&write.tags, &write.keep, index);
            records.extend(new);
            tags
        })
        .collect();
    (resolved, records)
}

/// Validate a set of merges and flatten it into `spelling -> final tag`.
///
/// Members are matched exactly as stored on bookmarks (untrimmed), so a
/// stray `" rust "` can be merged into `rust`. The canonical is trimmed and
/// must be a valid tag. A spelling may be merged into only one tag. Chains
/// are followed (`a -> b` plus `b -> c` maps `a` to `c`), and cycles are
/// rejected. Spellings that share a compact key must end at the same tag,
/// because one alias key points at one canonical. A merge with nothing to
/// rewrite is an error.
pub fn merge_mapping(merges: &[TagMerge]) -> Result<BTreeMap<String, String>, DomainError> {
    let mut direct: BTreeMap<String, String> = BTreeMap::new();
    for merge in merges {
        let canonical = clean_tag(&merge.canonical)
            .ok_or_else(|| DomainError::InvalidInput("merge target is not a valid tag".into()))?;
        for member in &merge.members {
            if member.is_empty() {
                return Err(DomainError::InvalidInput("merge member is empty".into()));
            }
            if member == canonical {
                continue;
            }
            match direct.get(member) {
                Some(existing) if existing != canonical => {
                    return Err(DomainError::InvalidInput(format!(
                        "tag \"{member}\" is merged into both \"{existing}\" and \"{canonical}\""
                    )));
                }
                _ => {
                    direct.insert(member.to_string(), canonical.to_string());
                }
            }
        }
    }
    if direct.is_empty() {
        return Err(DomainError::InvalidInput("nothing to merge".into()));
    }

    let mut resolved = BTreeMap::new();
    for source in direct.keys() {
        let mut target = &direct[source];
        let mut seen = HashSet::from([source.as_str()]);
        while let Some(next) = direct.get(target) {
            if !seen.insert(target.as_str()) {
                return Err(DomainError::InvalidInput(format!(
                    "merges form a cycle through \"{target}\""
                )));
            }
            target = next;
        }
        resolved.insert(source.clone(), target.clone());
    }

    // One alias key points at one canonical: two spellings with the same
    // compact key cannot end up at different tags, and a spelling cannot
    // leave a key that a target of this batch keeps.
    let mut key_targets: HashMap<String, (&str, &str)> = HashMap::new();
    let ends = resolved
        .iter()
        .map(|(source, target)| (source.as_str(), target.as_str()))
        .chain(resolved.values().map(|target| (target.as_str(), target.as_str())));
    for (spelling, target) in ends {
        if let Some((other, other_target)) =
            key_targets.insert(compact_key(spelling), (spelling, target))
            && other_target != target
        {
            return Err(DomainError::InvalidInput(format!(
                "\"{other}\" and \"{spelling}\" are spellings of one tag but merge into \
                 \"{other_target}\" and \"{target}\""
            )));
        }
    }
    Ok(resolved)
}

/// Apply a merge mapping to one bookmark's tags, keeping order and
/// removing the duplicates the merge creates.
pub fn rewrite_tags(tags: &[String], mapping: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let mapped = mapping.get(tag).unwrap_or(tag);
        if !out.contains(mapped) {
            out.push(mapped.clone());
        }
    }
    out
}

/// Undo one bookmark's change. If the bookmark still has `after`, restore
/// `before`. If it was edited since, apply the inverse as a diff: drop the
/// tags the batch added and re-add the ones it removed, leaving the user's
/// later edits in place.
pub fn undo_tags(current: &[String], before: &[String], after: &[String]) -> Vec<String> {
    if current == after {
        return before.to_vec();
    }
    let added: Vec<&String> = after.iter().filter(|t| !before.contains(t)).collect();
    let mut out: Vec<String> = current
        .iter()
        .filter(|t| !added.contains(t))
        .cloned()
        .collect();
    for tag in before.iter().filter(|t| !after.contains(t)) {
        if !out.contains(tag) {
            out.push(tag.clone());
        }
    }
    out
}

/// The L1-L3 groups with more than one spelling, as proposals. This is what
/// the on-demand normalization job stores; it never changes bookmarks.
pub fn normalization_proposals(tag_counts: &[(String, i64)]) -> Vec<NewTagMergeProposal> {
    group_tags(tag_counts, Level::Plural)
        .into_iter()
        .filter(|group| group.members.len() > 1)
        .filter_map(|group| {
            // A stray-whitespace spelling can win the display rule; the
            // merge target is its trimmed form.
            let canonical = clean_tag(&group.canonical)?.to_string();
            Some(NewTagMergeProposal {
                canonical,
                members: group.members,
                confidence: Confidence::High,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    fn counts(values: &[(&str, i64)]) -> Vec<(String, i64)> {
        values.iter().map(|(t, c)| ((*t).to_string(), *c)).collect()
    }

    fn alias(spelling: &str, canonical: &str) -> TagAlias {
        TagAlias {
            alias_key: compact_key(spelling),
            alias: spelling.to_string(),
            canonical: canonical.to_string(),
            source: AliasSource::Manual,
            batch_id: None,
        }
    }

    fn merge(canonical: &str, members: &[&str]) -> TagMerge {
        TagMerge {
            canonical: canonical.to_string(),
            members: s(members),
        }
    }

    fn resolve(index: &mut TagIndex, raw: &str) -> String {
        resolve_tag(raw, index).canonical
    }

    #[test]
    fn alias_row_wins_over_existing_spellings() {
        let mut index = TagIndex::new(
            &[alias("github", "GitHub")],
            &counts(&[("github", 3), ("GitHub", 9)]),
        );
        let resolution = resolve_tag("github", &mut index);
        assert_eq!(resolution.canonical, "GitHub");
        assert_eq!(resolution.record, None);
    }

    #[test]
    fn new_spelling_resolves_to_the_existing_tag_and_is_remembered() {
        let mut index = TagIndex::new(&[], &counts(&[("GitHub", 9)]));
        let resolution = resolve_tag("git-hub", &mut index);
        assert_eq!(resolution.canonical, "GitHub");
        assert_eq!(
            resolution.record,
            Some(NewTagAlias {
                alias_key: "github".into(),
                alias: "git-hub".into(),
                canonical: "GitHub".into(),
            })
        );
        // The second write hits the remembered alias.
        assert_eq!(resolve_tag("GITHUB", &mut index).record, None);
    }

    #[test]
    fn screenshot_list_resolves_to_existing_tags() {
        let mut index = TagIndex::new(
            &[],
            &counts(&[
                ("AI coding agents", 4),
                ("coding agents", 4),
                ("GitHub", 9),
                ("rust", 5),
                ("software engineering", 2),
                ("local ai", 1),
                ("UI/UX", 2),
            ]),
        );
        for (raw, expected) in [
            ("ai coding agents", "AI coding agents"),
            ("coding-agent", "coding agents"),
            ("Coding Agents", "coding agents"),
            ("github", "GitHub"),
            ("Rust", "rust"),
            ("software-engineering", "software engineering"),
            ("local-AI", "local ai"),
            ("ui-ux", "UI/UX"),
            ("ux", "ux"),
        ] {
            assert_eq!(resolve(&mut index, raw), expected, "{raw}");
        }
    }

    #[test]
    fn existing_exact_spelling_is_kept_even_with_a_same_key_duplicate() {
        let mut index = TagIndex::new(&[], &counts(&[("github", 3), ("GitHub", 9)]));
        let resolution = resolve_tag("github", &mut index);
        assert_eq!(resolution.canonical, "github");
        assert_eq!(resolution.record, None);
        // A third spelling picks the display form of the existing ones,
        // without pinning the key before the user reviews the duplicate.
        let resolution = resolve_tag("Github", &mut index);
        assert_eq!(resolution.canonical, "GitHub");
        assert_eq!(resolution.record, None);
        assert_eq!(resolve(&mut index, "github"), "github");
    }

    #[test]
    fn kept_tags_are_compared_before_trimming() {
        let mut index = TagIndex::new(&[alias("rust", "Rust")], &counts(&[(" rust ", 1)]));
        let (tags, records) = resolve_tags(
            &s(&[" rust ", "bad\u{1}tag", "rust"]),
            &s(&[" rust ", "bad\u{1}tag"]),
            &mut index,
        );
        assert_eq!(tags, [" rust ", "bad\u{1}tag", "Rust"]);
        assert!(records.is_empty());
    }

    #[test]
    fn resolve_writes_shares_one_index_across_an_import() {
        let mut index = TagIndex::new(&[], &counts(&[("Rust", 2)]));
        let writes = [
            TagWrite {
                tags: s(&["Zig", "rust"]),
                keep: vec![],
            },
            TagWrite {
                tags: s(&["zig", "rust"]),
                keep: s(&["rust"]),
            },
        ];
        let (resolved, records) = resolve_writes(&writes, &mut index);
        assert_eq!(resolved, vec![s(&["Zig", "Rust"]), s(&["Zig", "rust"])]);
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn merge_members_match_the_stored_spelling_exactly() {
        let mapping = merge_mapping(&[merge(" rust ", &[" rust ", "Rust "])]).unwrap();
        assert_eq!(mapping.len(), 2);
        assert_eq!(mapping[" rust "], "rust");
        assert_eq!(mapping["Rust "], "rust");
        assert!(merge_mapping(&[merge("rust", &[""])]).is_err());
    }

    #[test]
    fn plural_resolves_in_both_directions_only_to_existing_tags() {
        let mut index = TagIndex::new(&[], &counts(&[("library", 2), ("coding agents", 4)]));
        assert_eq!(resolve(&mut index, "libraries"), "library");
        assert_eq!(resolve(&mut index, "coding agent"), "coding agents");
        assert_eq!(resolve(&mut index, "kubernetes"), "kubernetes");
        assert_eq!(resolve(&mut index, "news"), "news");
    }

    #[test]
    fn plural_lookup_also_uses_alias_rows() {
        let mut index = TagIndex::new(&[alias("agent", "Agent")], &[]);
        assert_eq!(resolve(&mut index, "agents"), "Agent");
    }

    #[test]
    fn symbol_tags_stay_distinct() {
        let mut index = TagIndex::new(&[], &counts(&[("c", 1), ("net", 1), ("ux", 1)]));
        assert_eq!(resolve(&mut index, "C#"), "C#");
        assert_eq!(resolve(&mut index, "c++"), "c++");
        assert_eq!(resolve(&mut index, ".NET"), ".NET");
        assert_eq!(resolve(&mut index, "UI/UX"), "UI/UX");
        assert_eq!(resolve(&mut index, "c#"), "C#");
    }

    #[test]
    fn resolve_tags_keeps_current_tags_and_dedupes() {
        let mut index = TagIndex::new(&[alias("github", "GitHub")], &counts(&[("GitHub", 9)]));
        let (tags, records) = resolve_tags(
            &s(&["github", "  ", "GitHub", "Rust", "rust", "bad\u{1}tag"]),
            &[],
            &mut index,
        );
        assert_eq!(tags, ["GitHub", "Rust"]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].canonical, "Rust");

        // On an edit, a tag already on the bookmark is not rewritten.
        let mut index = TagIndex::new(&[alias("github", "GitHub")], &counts(&[("GitHub", 9)]));
        let (tags, _) = resolve_tags(&s(&["github", "git-hub"]), &s(&["github"]), &mut index);
        assert_eq!(tags, ["github", "GitHub"]);
    }

    #[test]
    fn merge_mapping_follows_chains_and_rejects_conflicts_and_cycles() {
        let mapping =
            merge_mapping(&[merge("b", &["a", "b"]), merge("c", &["b", "c", "d"])]).unwrap();
        assert_eq!(mapping["a"], "c");
        assert_eq!(mapping["b"], "c");
        assert_eq!(mapping["d"], "c");

        assert!(merge_mapping(&[merge("b", &["a"]), merge("c", &["a"])]).is_err());
        assert!(merge_mapping(&[merge("b", &["a"]), merge("a", &["b"])]).is_err());
        assert!(merge_mapping(&[merge("a", &["a"])]).is_err());
        assert!(merge_mapping(&[merge(" ", &["a"])]).is_err());
    }

    #[test]
    fn merge_mapping_rejects_one_key_merged_into_two_tags() {
        // `JS` and `js` share the alias key `js`.
        assert!(
            merge_mapping(&[merge("javascript", &["JS"]), merge("java script", &["js"])]).is_err()
        );
        // `Go` leaves the key `go`, which the target `go` keeps.
        assert!(merge_mapping(&[merge("go", &["golang"]), merge("Google", &["Go"])]).is_err());
        // Same key and same target, or a case-only rename, are fine.
        assert_eq!(
            merge_mapping(&[merge("GitHub", &["github", "Github", "gh"])])
                .unwrap()
                .len(),
            3
        );
        assert!(
            merge_mapping(&[merge("javascript", &["JS"]), merge("javascript", &["js"])]).is_ok()
        );
    }

    #[test]
    fn rewrite_replaces_members_and_dedupes_in_order() {
        let mapping = merge_mapping(&[merge("GitHub", &["github", "git hub"])]).unwrap();
        assert_eq!(
            rewrite_tags(&s(&["rust", "github", "GitHub", "git hub"]), &mapping),
            ["rust", "GitHub"]
        );
        assert_eq!(rewrite_tags(&s(&["rust"]), &mapping), ["rust"]);
    }

    #[test]
    fn undo_restores_before_when_untouched_since() {
        let before = s(&["rust", "github"]);
        let after = s(&["rust", "GitHub"]);
        assert_eq!(undo_tags(&after, &before, &after), before);
    }

    #[test]
    fn undo_applies_the_inverse_diff_after_a_later_edit() {
        let before = s(&["rust", "github"]);
        let after = s(&["rust", "GitHub"]);
        let current = s(&["GitHub", "news"]);
        assert_eq!(undo_tags(&current, &before, &after), ["news", "github"]);
        // A canonical that was on the bookmark before the merge stays.
        let before = s(&["GitHub", "github"]);
        let after = s(&["GitHub"]);
        let current = s(&["GitHub", "news"]);
        assert_eq!(
            undo_tags(&current, &before, &after),
            ["GitHub", "news", "github"]
        );
    }

    #[test]
    fn normalization_proposals_cover_only_groups_with_duplicates() {
        let proposals = normalization_proposals(&counts(&[
            ("GitHub", 9),
            ("github", 3),
            ("rust", 5),
            ("coding agents", 4),
            ("coding-agent", 2),
            ("UI/UX", 2),
            ("ux", 1),
        ]));
        assert_eq!(
            proposals,
            vec![
                NewTagMergeProposal {
                    canonical: "GitHub".into(),
                    members: s(&["GitHub", "github"]),
                    confidence: Confidence::High,
                },
                NewTagMergeProposal {
                    canonical: "coding agents".into(),
                    members: s(&["coding agents", "coding-agent"]),
                    confidence: Confidence::High,
                },
            ]
        );
    }

    #[test]
    fn enums_round_trip_through_their_database_text() {
        for kind in [
            ChangeKind::Normalize,
            ChangeKind::LlmMerge,
            ChangeKind::Rename,
            ChangeKind::Merge,
            ChangeKind::Delete,
        ] {
            assert_eq!(ChangeKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert_eq!(ProposalSource::parse("llm").unwrap(), ProposalSource::Llm);
        assert!(ProposalStatus::parse("bogus").is_err());
    }
}
