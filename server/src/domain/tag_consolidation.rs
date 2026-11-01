//! Reviewed LLM consolidation, the pure part (tag spec §3 and "Guards for
//! the Haiku default").
//!
//! The LLM sees one [`TagUnit`] per deterministic (L1–L3) group. Candidate
//! merges come from two sources: a whole-list call ([`validate_whole_list`])
//! and free string candidates ([`candidate_reason`]). Every candidate pair is
//! then checked by the model one pair at a time, and [`build_proposals`]
//! unions the accepted pairs into merge groups:
//!
//! - `high` groups (pair check `high`, or a whole-list `high` proposal) are
//!   pre-checked on the review screen;
//! - `medium` pair verdicts form separate rows under "Check these";
//! - a pair the check judged different is a cannot-link: no group may hold
//!   both tags.
//!
//! The result is only ever stored as proposals; nothing here changes tags.

use crate::domain::tag_alias::{Confidence, NewTagMergeProposal};
use crate::domain::tag_normalize::{Level, choose_display, clean_tag, group_tags};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The most tags one run considers, most used first.
pub const MAX_UNITS: usize = 1_000;
/// The most pairs one run checks (20 calls of 40).
pub const MAX_PAIRS: usize = 800;
/// Tags the LLM writes are bounded like every other tag.
pub const MAX_TAG_CHARS: usize = 100;

/// One distinct tag on the user's bookmarks, with a sample title for
/// disambiguation (`go` the language or the game).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSample {
    pub tag: String,
    pub count: i64,
    pub sample_title: Option<String>,
}

/// One tag as the LLM sees it: a deterministic group under its display
/// form, with its summed count and a sample title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagUnit {
    pub tag: String,
    pub count: i64,
    pub sample_title: Option<String>,
}

/// Fold the raw tags into deterministic groups, most used first, capped at
/// [`MAX_UNITS`]. The deterministic proposals merge each group; the LLM
/// works on what is left.
pub fn tag_units(samples: &[TagSample]) -> Vec<TagUnit> {
    let counts: Vec<(String, i64)> = samples.iter().map(|s| (s.tag.clone(), s.count)).collect();
    let titles: HashMap<&str, &str> = samples
        .iter()
        .filter_map(|s| Some((s.tag.as_str(), s.sample_title.as_deref()?)))
        .collect();
    let mut units: Vec<TagUnit> = group_tags(&counts, Level::Plural)
        .into_iter()
        .filter_map(|group| {
            let tag = clean_tag(&group.canonical)?.to_string();
            let sample_title = std::iter::once(&group.canonical)
                .chain(&group.members)
                .find_map(|m| titles.get(m.as_str()))
                .map(|t| t.to_string());
            Some(TagUnit {
                tag,
                count: group.count,
                sample_title,
            })
        })
        .collect();
    units.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    units.truncate(MAX_UNITS);
    units
}

/// Why two tags are worth asking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateReason {
    /// `js` / `javascript`: a short tag whose letters appear in order.
    Abbreviation,
    /// `mcp` / `model context protocol`.
    Initials,
    /// `k8s` / `kubernetes`, `a11y` / `accessibility`.
    Numeronym,
    /// `web dev` / `web development`.
    Substring,
    /// `kubernets` / `kubernetes`: edit distance at most 2.
    Typo,
    /// At least half the words are shared.
    SharedWords,
}

/// Spell out the symbols that carry meaning in tag names, so `c#` is
/// `csharp` and `c++` is `cplusplus` instead of collapsing into `c`.
fn spell_symbols(tag: &str) -> String {
    tag.to_lowercase()
        .replace('#', "sharp")
        .replace('+', "plus")
}

fn letters(tag: &str) -> Vec<char> {
    spell_symbols(tag)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn words(tag: &str) -> Vec<String> {
    spell_symbols(tag)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_subsequence(short: &[char], long: &[char]) -> bool {
    let mut rest = long.iter();
    short.iter().all(|c| rest.any(|l| l == c))
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j + 1] + 1)
                    .min(cur[j] + 1)
                    .min(prev[j] + usize::from(ca != cb)),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

fn contains(long: &[char], short: &[char]) -> bool {
    long.windows(short.len()).any(|w| w == short)
}

/// The string-candidate pass (`scripts/tag-lab/verify.py`), symbol-aware.
/// Free and deterministic; the pair check decides.
pub fn candidate_reason(a: &str, b: &str) -> Option<CandidateReason> {
    let (la, lb) = (letters(a), letters(b));
    if la.is_empty() || lb.is_empty() {
        return None;
    }
    let (short, long, long_tag) = if la.len() <= lb.len() {
        (&la, &lb, b)
    } else {
        (&lb, &la, a)
    };
    let long_words = words(long_tag);
    // One letter is not an abbreviation: `c` would match every `c...` tag.
    if (2..=5).contains(&short.len()) && short[0] == long[0] && is_subsequence(short, long) {
        return Some(CandidateReason::Abbreviation);
    }
    if short.len() <= 5
        && long_words.len() > 1
        && *short
            == long_words
                .iter()
                .filter_map(|w| w.chars().next())
                .collect::<Vec<_>>()
    {
        return Some(CandidateReason::Initials);
    }
    if short.len() >= 3
        && short[1..short.len() - 1].iter().all(char::is_ascii_digit)
        && short[0] == long[0]
        && short.last() == long.last()
        && short[1..short.len() - 1]
            .iter()
            .collect::<String>()
            .parse::<usize>()
            .is_ok_and(|n| n + 2 == long.len())
    {
        return Some(CandidateReason::Numeronym);
    }
    if short.len() >= 2 && contains(long, short) {
        return Some(CandidateReason::Substring);
    }
    if short.len() >= 5 && edit_distance(&la, &lb) <= 2 {
        return Some(CandidateReason::Typo);
    }
    let (wa, wb): (HashSet<String>, HashSet<String>) = (
        words(a).into_iter().collect(),
        words(b).into_iter().collect(),
    );
    let shared = wa.intersection(&wb).count();
    let all = wa.union(&wb).count();
    (all > 0 && shared * 2 >= all && shared > 0).then_some(CandidateReason::SharedWords)
}

/// A whole-list proposal that survived validation, as unit indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedGroup {
    pub canonical: usize,
    pub members: Vec<usize>,
    pub confidence: Confidence,
}

/// One merge group as the model returned it, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawGroup {
    pub canonical: String,
    pub members: Vec<String>,
    pub confidence: Confidence,
}

/// Validate the whole-list answer before anything is shown (spec §3):
/// unknown tags and tags an earlier group claimed are dropped, then groups
/// whose canonical is not a member or that have fewer than two members are
/// dropped. Tags match exactly, else case-insensitively when that is
/// unambiguous. Invalid entries are dropped, never guessed.
pub fn validate_whole_list(groups: &[RawGroup], units: &[TagUnit]) -> Vec<ProposedGroup> {
    let exact: HashMap<&str, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.tag.as_str(), i))
        .collect();
    let mut folded: HashMap<String, Option<usize>> = HashMap::new();
    for (i, unit) in units.iter().enumerate() {
        folded
            .entry(unit.tag.to_lowercase())
            .and_modify(|seen| *seen = None)
            .or_insert(Some(i));
    }
    let lookup = |tag: &str| -> Option<usize> {
        if tag.chars().count() > MAX_TAG_CHARS || tag.chars().any(char::is_control) {
            return None;
        }
        let tag = tag.trim();
        exact
            .get(tag)
            .copied()
            .or_else(|| folded.get(&tag.to_lowercase()).copied().flatten())
    };

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();
    for group in groups {
        let mut members: Vec<usize> = Vec::new();
        for member in &group.members {
            if let Some(i) = lookup(member)
                && !claimed.contains(&i)
                && !members.contains(&i)
            {
                members.push(i);
            }
        }
        let Some(canonical) = lookup(&group.canonical).filter(|c| members.contains(c)) else {
            continue;
        };
        if members.len() < 2 {
            continue;
        }
        claimed.extend(&members);
        out.push(ProposedGroup {
            canonical,
            members,
            confidence: group.confidence,
        });
    }
    out
}

/// A pair to check, as unit indices with what put it on the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePair {
    pub a: usize,
    pub b: usize,
    /// Set when the string pass found it.
    pub reason: Option<CandidateReason>,
    /// The whole-list proposal's confidence, when that call proposed it.
    pub proposed: Option<Confidence>,
}

/// Every pair from either source, once, capped at [`MAX_PAIRS`]. Whole-list
/// pairs come first: string candidates cannot find pure synonyms
/// (`wfh` / `remote work`).
pub fn candidate_pairs(units: &[TagUnit], groups: &[ProposedGroup]) -> Vec<CandidatePair> {
    let mut pairs: Vec<CandidatePair> = Vec::new();
    let mut index: HashMap<(usize, usize), usize> = HashMap::new();
    let key = |a: usize, b: usize| (a.min(b), a.max(b));
    for group in groups {
        for &member in group.members.iter().filter(|&&m| m != group.canonical) {
            index.insert(key(member, group.canonical), pairs.len());
            pairs.push(CandidatePair {
                a: member,
                b: group.canonical,
                reason: candidate_reason(&units[member].tag, &units[group.canonical].tag),
                proposed: Some(group.confidence),
            });
        }
    }
    'outer: for a in 0..units.len() {
        for b in a + 1..units.len() {
            if pairs.len() >= MAX_PAIRS {
                break 'outer;
            }
            if index.contains_key(&key(a, b)) {
                continue;
            }
            if let Some(reason) = candidate_reason(&units[a].tag, &units[b].tag) {
                pairs.push(CandidatePair {
                    a,
                    b,
                    reason: Some(reason),
                    proposed: None,
                });
            }
        }
    }
    pairs.truncate(MAX_PAIRS);
    pairs
}

/// The pair check's answer for one pair whose echo matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairVerdict {
    /// Index into the checked pair list.
    pub pair: usize,
    pub same: bool,
    pub confidence: Confidence,
}

/// Union-find over units where a union is refused when it would put two
/// tags of a cannot-link pair into one set.
struct Groups {
    parent: Vec<usize>,
    members: Vec<Vec<usize>>,
    cannot: HashSet<(usize, usize)>,
}

impl Groups {
    fn new(len: usize, cannot: HashSet<(usize, usize)>) -> Self {
        Self {
            parent: (0..len).collect(),
            members: (0..len).map(|i| vec![i]).collect(),
            cannot,
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let blocked = self.members[ra].iter().any(|&x| {
            self.members[rb]
                .iter()
                .any(|&y| self.cannot.contains(&(x.min(y), x.max(y))))
        });
        if blocked {
            return;
        }
        let moved = std::mem::take(&mut self.members[ra]);
        self.members[rb].extend(moved);
        self.parent[ra] = rb;
    }
}

/// The canonical of a group: the whole-list call's canonical when the
/// group holds one (a `high` proposal first), otherwise the display rule.
fn pick_canonical(names: &[(String, i64)], groups: &[ProposedGroup], units: &[TagUnit]) -> String {
    let mut proposed: Vec<&ProposedGroup> = groups.iter().collect();
    proposed.sort_by_key(|g| g.confidence != Confidence::High);
    proposed
        .iter()
        .map(|g| &units[g.canonical].tag)
        .find(|tag| names.iter().any(|(name, _)| name == *tag))
        .cloned()
        .unwrap_or_else(|| choose_display(names))
}

/// Union the accepted pairs into merge proposals (spec "Guards"):
///
/// 1. `high` edges (a pair-check `high` "same", or a pair from a whole-list
///    `high` proposal) make `high` groups, which are pre-checked.
/// 2. `medium` "same" verdicts then join those groups or single tags; each
///    such join is one `medium` row whose members are the joined groups'
///    canonicals, so applying it after the `high` rows chains cleanly.
/// 3. Every pair the check answered "not same" is a cannot-link in both
///    passes. `low` "same" verdicts are not proposed.
pub fn build_proposals(
    units: &[TagUnit],
    groups: &[ProposedGroup],
    pairs: &[CandidatePair],
    verdicts: &[PairVerdict],
) -> Vec<NewTagMergeProposal> {
    let key = |p: &CandidatePair| (p.a.min(p.b), p.a.max(p.b));
    let cannot: HashSet<(usize, usize)> = verdicts
        .iter()
        .filter(|v| !v.same)
        .map(|v| key(&pairs[v.pair]))
        .collect();
    let edges = |level: Confidence| -> Vec<(usize, usize)> {
        verdicts
            .iter()
            .filter(|v| v.same && v.confidence == level)
            .map(|v| (pairs[v.pair].a, pairs[v.pair].b))
            .collect()
    };
    let mut high = edges(Confidence::High);
    for group in groups.iter().filter(|g| g.confidence == Confidence::High) {
        high.extend(
            group
                .members
                .iter()
                .filter(|&&m| m != group.canonical)
                .map(|&m| (m, group.canonical)),
        );
    }

    let mut uf = Groups::new(units.len(), cannot);
    for (a, b) in high {
        uf.union(a, b);
    }
    let first: Vec<usize> = (0..units.len()).map(|i| uf.find(i)).collect();
    for (a, b) in edges(Confidence::Medium) {
        uf.union(a, b);
    }

    let names = |members: &[usize]| -> Vec<(String, i64)> {
        members
            .iter()
            .map(|&i| (units[i].tag.clone(), units[i].count))
            .collect()
    };
    let mut first_groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &root) in first.iter().enumerate() {
        first_groups.entry(root).or_default().push(i);
    }
    // Each first-pass group as one name: its canonical and summed count.
    let mut first_names: HashMap<usize, (String, i64)> = HashMap::new();
    let mut out: Vec<(i64, NewTagMergeProposal)> = Vec::new();
    for (&root, members) in &first_groups {
        let named = names(members);
        let total: i64 = named.iter().map(|(_, c)| c).sum();
        let canonical = pick_canonical(&named, groups, units);
        first_names.insert(root, (canonical.clone(), total));
        if members.len() > 1 {
            out.push((
                total,
                NewTagMergeProposal {
                    canonical,
                    members: named.into_iter().map(|(t, _)| t).collect(),
                    confidence: Confidence::High,
                },
            ));
        }
    }

    let mut final_groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &root in first_groups.keys() {
        final_groups.entry(uf.find(root)).or_default().push(root);
    }
    for roots in final_groups.values().filter(|roots| roots.len() > 1) {
        let named: Vec<(String, i64)> = roots.iter().map(|r| first_names[r].clone()).collect();
        let total: i64 = named.iter().map(|(_, c)| c).sum();
        out.push((
            total,
            NewTagMergeProposal {
                canonical: pick_canonical(&named, groups, units),
                members: named.into_iter().map(|(t, _)| t).collect(),
                confidence: Confidence::Medium,
            },
        ));
    }

    out.sort_by(|(ca, a), (cb, b)| {
        (a.confidence != Confidence::High)
            .cmp(&(b.confidence != Confidence::High))
            .then_with(|| cb.cmp(ca))
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    out.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(tags: &[(&str, i64)]) -> Vec<TagUnit> {
        tags.iter()
            .map(|(t, c)| TagUnit {
                tag: t.to_string(),
                count: *c,
                sample_title: None,
            })
            .collect()
    }

    fn raw(canonical: &str, members: &[&str], confidence: Confidence) -> RawGroup {
        RawGroup {
            canonical: canonical.to_string(),
            members: members.iter().map(|m| m.to_string()).collect(),
            confidence,
        }
    }

    fn pair(a: usize, b: usize) -> CandidatePair {
        CandidatePair {
            a,
            b,
            reason: None,
            proposed: None,
        }
    }

    fn verdict(pair: usize, same: bool, confidence: Confidence) -> PairVerdict {
        PairVerdict {
            pair,
            same,
            confidence,
        }
    }

    fn summary(proposals: &[NewTagMergeProposal]) -> Vec<(String, Vec<String>, Confidence)> {
        proposals
            .iter()
            .map(|p| (p.canonical.clone(), p.members.clone(), p.confidence))
            .collect()
    }

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn units_fold_deterministic_groups_and_keep_a_sample_title() {
        let samples = vec![
            TagSample {
                tag: "GitHub".into(),
                count: 3,
                sample_title: None,
            },
            TagSample {
                tag: "github".into(),
                count: 5,
                sample_title: Some("Actions".into()),
            },
            TagSample {
                tag: "rust".into(),
                count: 9,
                sample_title: None,
            },
        ];
        assert_eq!(
            tag_units(&samples),
            vec![
                TagUnit {
                    tag: "rust".into(),
                    count: 9,
                    sample_title: None
                },
                TagUnit {
                    tag: "github".into(),
                    count: 8,
                    sample_title: Some("Actions".into())
                },
            ]
        );
    }

    #[test]
    fn candidates_find_abbreviations_numeronyms_and_typos() {
        use CandidateReason::*;
        assert_eq!(candidate_reason("js", "javascript"), Some(Abbreviation));
        assert_eq!(
            candidate_reason("mcp", "model context protocol"),
            Some(Abbreviation)
        );
        assert_eq!(candidate_reason("k8s", "kubernetes"), Some(Numeronym));
        assert_eq!(
            candidate_reason("web dev", "web development"),
            Some(Substring)
        );
        assert_eq!(candidate_reason("kubernets", "kubernetes"), Some(Typo));
        assert_eq!(candidate_reason("javscript", "javascript"), Some(Typo));
        assert_eq!(
            candidate_reason("machine learning", "learning theory"),
            None
        );
        assert_eq!(candidate_reason("rust", "python"), None);
    }

    #[test]
    fn candidates_respect_symbols() {
        // `c#` is not `c`: it never matched `continuous integration` again.
        assert_eq!(candidate_reason("c#", "continuous integration"), None);
        assert_eq!(candidate_reason("c#", "c"), None);
        assert_eq!(candidate_reason("c#", "cpp"), None);
        assert!(candidate_reason("c++", "cpp").is_some());
        assert!(candidate_reason("c#", "csharp").is_some());
    }

    #[test]
    fn whole_list_validation_drops_unknown_claimed_and_invalid_groups() {
        let units = units(&[
            ("javascript", 9),
            ("js", 4),
            ("typescript", 3),
            ("ts", 2),
            ("Rust", 1),
        ]);
        let groups = validate_whole_list(
            &[
                raw(
                    "javascript",
                    &["javascript", "js", "ecmascript"],
                    Confidence::High,
                ),
                // `js` is already claimed, so one member is left: dropped.
                raw("js", &["js", "jscript"], Confidence::High),
                // The canonical is not a member: dropped.
                raw("typescript", &["ts", "tsx"], Confidence::Medium),
                // Case-insensitive match maps back to the listed spelling.
                raw("RUST", &["RUST", "rust-lang", "ts"], Confidence::Low),
            ],
            &units,
        );
        assert_eq!(
            groups,
            vec![
                ProposedGroup {
                    canonical: 0,
                    members: vec![0, 1],
                    confidence: Confidence::High
                },
                ProposedGroup {
                    canonical: 4,
                    members: vec![4, 3],
                    confidence: Confidence::Low
                },
            ]
        );
    }

    #[test]
    fn whole_list_pairs_come_first_and_are_not_repeated_by_candidates() {
        let units = units(&[("javascript", 9), ("js", 4), ("wfh", 2), ("remote work", 2)]);
        let groups = vec![ProposedGroup {
            canonical: 3,
            members: vec![3, 2],
            confidence: Confidence::Medium,
        }];
        let pairs = candidate_pairs(&units, &groups);
        assert_eq!(
            pairs
                .iter()
                .map(|p| (p.a, p.b, p.proposed))
                .collect::<Vec<_>>(),
            vec![(2, 3, Some(Confidence::Medium)), (0, 1, None)]
        );
    }

    #[test]
    fn high_pairs_and_high_whole_list_groups_are_high_and_medium_is_separate() {
        let units = units(&[
            ("javascript", 9),
            ("js", 4),
            ("ecmascript", 1),
            ("remote work", 3),
            ("wfh", 2),
        ]);
        let groups = vec![ProposedGroup {
            canonical: 0,
            members: vec![0, 2],
            confidence: Confidence::High,
        }];
        let pairs = vec![pair(1, 0), pair(4, 3), pair(2, 3)];
        let verdicts = vec![
            verdict(0, true, Confidence::High),
            verdict(1, true, Confidence::Medium),
            verdict(2, true, Confidence::Low),
        ];
        assert_eq!(
            summary(&build_proposals(&units, &groups, &pairs, &verdicts)),
            vec![
                (
                    "javascript".to_string(),
                    s(&["javascript", "js", "ecmascript"]),
                    Confidence::High
                ),
                (
                    "remote work".to_string(),
                    s(&["remote work", "wfh"]),
                    Confidence::Medium
                ),
            ]
        );
    }

    #[test]
    fn medium_rows_join_high_groups_by_their_canonicals() {
        let units = units(&[("kubernetes", 9), ("k8s", 3), ("kube", 2)]);
        let pairs = vec![pair(1, 0), pair(2, 0)];
        let verdicts = vec![
            verdict(0, true, Confidence::High),
            verdict(1, true, Confidence::Medium),
        ];
        assert_eq!(
            summary(&build_proposals(&units, &[], &pairs, &verdicts)),
            vec![
                (
                    "kubernetes".to_string(),
                    s(&["kubernetes", "k8s"]),
                    Confidence::High
                ),
                (
                    "kubernetes".to_string(),
                    s(&["kubernetes", "kube"]),
                    Confidence::Medium
                ),
            ]
        );
    }

    #[test]
    fn cannot_link_stops_chains_and_vetoes_whole_list_pairs() {
        // seo ~ selfhosted (wrong) and selfhosted ~ homelab would chain seo
        // into homelab; the check said seo is not homelab.
        let units = units(&[("homelab", 5), ("selfhosted", 4), ("seo", 3)]);
        let pairs = vec![pair(1, 0), pair(2, 1), pair(2, 0)];
        let verdicts = vec![
            verdict(0, true, Confidence::High),
            verdict(1, true, Confidence::High),
            verdict(2, false, Confidence::High),
        ];
        let proposals = build_proposals(&units, &[], &pairs, &verdicts);
        assert_eq!(
            summary(&proposals),
            vec![(
                "homelab".to_string(),
                s(&["homelab", "selfhosted"]),
                Confidence::High
            )]
        );

        // A whole-list `high` pair the check rejected is not merged.
        let groups = vec![ProposedGroup {
            canonical: 0,
            members: vec![0, 2],
            confidence: Confidence::High,
        }];
        let pairs = vec![pair(2, 0)];
        let verdicts = vec![verdict(0, false, Confidence::Medium)];
        assert!(build_proposals(&units, &groups, &pairs, &verdicts).is_empty());
    }

    #[test]
    fn group_canonical_prefers_the_whole_list_choice_then_the_display_rule() {
        let units = units(&[("js", 9), ("javascript", 4), ("Golang", 3), ("go lang", 1)]);
        let groups = vec![ProposedGroup {
            canonical: 1,
            members: vec![1, 0],
            confidence: Confidence::Medium,
        }];
        let pairs = vec![pair(0, 1), pair(2, 3)];
        let verdicts = vec![
            verdict(0, true, Confidence::High),
            verdict(1, true, Confidence::High),
        ];
        let canonicals: Vec<String> = build_proposals(&units, &groups, &pairs, &verdicts)
            .into_iter()
            .map(|p| p.canonical)
            .collect();
        assert_eq!(canonicals, ["javascript", "go lang"]);
    }
}
