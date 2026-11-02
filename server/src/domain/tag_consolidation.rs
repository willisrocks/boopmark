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

use crate::domain::error::DomainError;
use crate::domain::tag_alias::{
    ChangeKind, Confidence, NewTagMergeProposal, ProposalSource, ProposalStatus, TagAlias,
    TagMerge, TagMergeProposal, string_enum,
};
use crate::domain::tag_normalize::{
    Level, bounded_tag, choose_display, clean_tag, compact_key, group_tags,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

/// The most tags one run considers, most used first.
pub const MAX_UNITS: usize = 1_000;
/// The most pairs one run checks (20 calls of 40).
pub const MAX_PAIRS: usize = 800;
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

/// What the candidate pass compares, computed once per tag.
struct CandidateKey {
    letters: Vec<char>,
    words: HashSet<String>,
    initials: Vec<char>,
}

impl CandidateKey {
    fn new(tag: &str) -> Self {
        let spelled = spell_symbols(tag);
        let words: Vec<&str> = spelled
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        Self {
            letters: spelled.chars().filter(|c| c.is_alphanumeric()).collect(),
            initials: words.iter().filter_map(|w| w.chars().next()).collect(),
            words: words.into_iter().map(str::to_string).collect(),
        }
    }
}

/// The string-candidate pass (`scripts/tag-lab/verify.py`), symbol-aware.
/// Free and deterministic; the pair check decides.
#[cfg(test)]
fn candidate_reason(a: &str, b: &str) -> Option<CandidateReason> {
    key_reason(&CandidateKey::new(a), &CandidateKey::new(b))
}

fn key_reason(a: &CandidateKey, b: &CandidateKey) -> Option<CandidateReason> {
    let (la, lb) = (&a.letters, &b.letters);
    if la.is_empty() || lb.is_empty() {
        return None;
    }
    let (short, long, long_key) = if la.len() <= lb.len() {
        (la, lb, b)
    } else {
        (lb, la, a)
    };
    // One letter is not an abbreviation: `c` would match every `c...` tag.
    if (2..=5).contains(&short.len()) && short[0] == long[0] && is_subsequence(short, long) {
        return Some(CandidateReason::Abbreviation);
    }
    if short.len() <= 5 && long_key.initials.len() > 1 && *short == long_key.initials {
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
    if short.len() >= 5 && long.len() - short.len() <= 2 && edit_distance(la, lb) <= 2 {
        return Some(CandidateReason::Typo);
    }
    let shared = a.words.intersection(&b.words).count();
    let all = a.words.union(&b.words).count();
    (shared > 0 && shared * 2 >= all).then_some(CandidateReason::SharedWords)
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
        let tag = bounded_tag(tag)?;
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

/// A pair of units to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidatePair {
    pub a: usize,
    pub b: usize,
}

/// Every pair from either source, once, capped at [`MAX_PAIRS`]. Whole-list
/// pairs come first: string candidates cannot find pure synonyms
/// (`wfh` / `remote work`).
pub fn candidate_pairs(units: &[TagUnit], groups: &[ProposedGroup]) -> Vec<CandidatePair> {
    let mut pairs: Vec<CandidatePair> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let key = |a: usize, b: usize| (a.min(b), a.max(b));
    for group in groups {
        for &member in group.members.iter().filter(|&&m| m != group.canonical) {
            seen.insert(key(member, group.canonical));
            pairs.push(CandidatePair {
                a: member,
                b: group.canonical,
            });
        }
    }
    let keys: Vec<CandidateKey> = units.iter().map(|u| CandidateKey::new(&u.tag)).collect();
    'outer: for a in 0..units.len() {
        for b in a + 1..units.len() {
            if pairs.len() >= MAX_PAIRS {
                break 'outer;
            }
            if !seen.contains(&key(a, b)) && key_reason(&keys[a], &keys[b]).is_some() {
                pairs.push(CandidatePair { a, b });
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

/// True when `short` is an abbreviation, initials or numeronym of `long`
/// (`js` of `javascript`, `k8s` of `kubernetes`).
fn is_short_form(short: &str, long: &str) -> bool {
    let (short, long) = (CandidateKey::new(short), CandidateKey::new(long));
    short.letters.len() < long.letters.len()
        && matches!(
            key_reason(&short, &long),
            Some(
                CandidateReason::Abbreviation
                    | CandidateReason::Initials
                    | CandidateReason::Numeronym
            )
        )
}

/// The canonical of a group: the whole-list call's canonical when the
/// group holds one (a `high` proposal first). Otherwise the display rule,
/// over the names that are not a short form of another name in the group,
/// so `javascript` wins over a more used `js`.
fn pick_canonical(names: &[(String, i64)], groups: &[ProposedGroup], units: &[TagUnit]) -> String {
    let mut proposed: Vec<&ProposedGroup> = groups.iter().collect();
    proposed.sort_by_key(|g| g.confidence != Confidence::High);
    proposed
        .iter()
        .map(|g| &units[g.canonical].tag)
        .find(|tag| names.iter().any(|(name, _)| name == *tag))
        .cloned()
        .unwrap_or_else(|| {
            let full: Vec<(String, i64)> = names
                .iter()
                .filter(|(name, _)| !names.iter().any(|(other, _)| is_short_form(name, other)))
                .cloned()
                .collect();
            choose_display(if full.is_empty() { names } else { &full })
        })
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

string_enum!(
    /// Where the latest consolidation run of a user stands.
    JobStatus {
        Running => "running",
        Succeeded => "succeeded",
        Failed => "failed",
    }
);

/// A run still marked running this long after it started died with its
/// process; a new run may replace it. This is longer than the slowest
/// possible run (42 calls at the 60 s vendor timeout), and a replaced run
/// can no longer store its proposals.
pub const STALE_RUN_MINUTES: i64 = 60;

/// The latest consolidation run of one user.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidationJob {
    pub run_id: Uuid,
    pub status: JobStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// User-facing; never a key, prompt or model answer.
    pub error: Option<String>,
    pub proposals: i32,
    pub pairs_checked: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl ConsolidationJob {
    /// Report a run that outlived [`STALE_RUN_MINUTES`] as failed.
    pub fn settle(mut self, now: DateTime<Utc>) -> Self {
        if self.status == JobStatus::Running
            && now - self.started_at > Duration::minutes(STALE_RUN_MINUTES)
        {
            self.status = JobStatus::Failed;
            self.error = Some("The run stopped before it finished. Start it again.".into());
        }
        self
    }
}

/// How a run ended, as recorded on its job row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutcome {
    pub status: JobStatus,
    pub error: Option<String>,
    pub proposals: i32,
    pub pairs_checked: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// One spelling on a review row, with its bookmark count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewChip {
    pub tag: String,
    pub count: i64,
}

/// One pending proposal as the review screen shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRow {
    pub id: Uuid,
    pub source: ProposalSource,
    pub confidence: Confidence,
    pub canonical: String,
    /// Every spelling the row merges, including the spelling variants of
    /// its members that applying it also merges.
    pub chips: Vec<ReviewChip>,
    /// Uses of the spellings that change: at most this many bookmarks.
    pub bookmarks: i64,
    pub sample_title: Option<String>,
}

/// The pending deterministic proposals an LLM proposal covers: those whose
/// canonical is one of its members. Applying the LLM row applies them in the
/// same batch, so every spelling of a member merges and chains resolve.
fn covered<'a>(
    proposal: &TagMergeProposal,
    pending: &'a [TagMergeProposal],
) -> impl Iterator<Item = &'a TagMergeProposal> {
    let members: HashSet<&str> = if proposal.source == ProposalSource::Llm {
        proposal.members.iter().map(String::as_str).collect()
    } else {
        HashSet::new()
    };
    pending.iter().filter(move |p| {
        p.source == ProposalSource::Deterministic
            && p.status == ProposalStatus::Pending
            && members.contains(p.canonical.as_str())
    })
}

/// The proposals to apply for a selection: the selected pending proposals
/// plus the deterministic ones their LLM rows cover, each once.
fn expand_selection<'a>(
    selected: &[Uuid],
    pending: &'a [TagMergeProposal],
) -> Vec<&'a TagMergeProposal> {
    let mut out: Vec<&TagMergeProposal> = Vec::new();
    for proposal in pending.iter().filter(|p| selected.contains(&p.id)) {
        for p in std::iter::once(proposal).chain(covered(proposal, pending)) {
            if !out.iter().any(|o| o.id == p.id) {
                out.push(p);
            }
        }
    }
    out
}

/// Every spelling that applying `proposal` merges: its members, then the
/// members of the spelling-variant rows it covers, each once.
pub fn row_spellings(proposal: &TagMergeProposal, pending: &[TagMergeProposal]) -> Vec<String> {
    unique(
        proposal
            .members
            .iter()
            .chain(covered(proposal, pending).flat_map(|p| &p.members)),
    )
}

/// `tags` in order, each once.
fn unique<'a>(tags: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        if !out.contains(tag) {
            out.push(tag.clone());
        }
    }
    out
}

/// Pending proposals as they apply now. A member or target that is on no
/// bookmark any more, and whose alias points at another tag, was merged
/// since the proposal was made (maybe under an edited name); it is replaced
/// by that tag. So `js -> javascript` still merges `JS` after `JS, js` was
/// merged into `JS`. Proposals left with nothing to merge drop out.
pub fn resolve_through_aliases(
    pending: &[TagMergeProposal],
    aliases: &[TagAlias],
    on_bookmarks: &HashSet<String>,
) -> Vec<TagMergeProposal> {
    let by_key: HashMap<&str, &str> = aliases
        .iter()
        .map(|a| (a.alias_key.as_str(), a.canonical.as_str()))
        .collect();
    let mut padded: HashMap<&str, Vec<&String>> = HashMap::new();
    for tag in on_bookmarks.iter().filter(|t| t.trim() != t.as_str()) {
        padded.entry(tag.trim()).or_default().push(tag);
    }
    for variants in padded.values_mut() {
        variants.sort();
    }
    let current = |tag: &String| -> String {
        if on_bookmarks.contains(tag) {
            return tag.clone();
        }
        by_key
            .get(compact_key(tag).as_str())
            .map_or_else(|| tag.clone(), |c| c.to_string())
    };
    pending
        .iter()
        .filter_map(|proposal| {
            let canonical = current(&proposal.canonical);
            // A spelling on bookmarks that only differs by surrounding
            // whitespace (" js ") merges with its trimmed member.
            let mut members: Vec<String> = Vec::new();
            for member in proposal.members.iter().map(current) {
                let variants = padded.get(member.as_str()).into_iter().flatten();
                for tag in std::iter::once(&member).chain(variants.copied()) {
                    if !members.contains(tag) {
                        members.push(tag.clone());
                    }
                }
            }
            members
                .iter()
                .any(|m| *m != canonical)
                .then(|| TagMergeProposal {
                    canonical,
                    members,
                    ..proposal.clone()
                })
        })
        .collect()
}

/// Shown when a checked row no longer matches what the page showed.
pub const LIST_CHANGED: &str =
    "These suggestions changed since the page loaded. Reload the page and check them again.";

/// One row the user checked, as the page showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedRow {
    pub id: Uuid,
    /// The spellings the row showed (its chips).
    pub shown: Vec<String>,
    /// The merge target as submitted, maybe edited.
    pub canonical: Option<String>,
}

/// What applying the checked rows does.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewedApply {
    /// The checked rows plus the spelling-variant rows they cover.
    pub ids: Vec<Uuid>,
    pub merges: Vec<TagMerge>,
    /// Edited targets to save on their proposals.
    pub edited: Vec<(Uuid, String)>,
    /// `llm_merge` when an AI proposal is among them, else `normalize`.
    pub kind: ChangeKind,
}

/// Plan applying the checked rows against the proposals pending now
/// (already passed through [`resolve_through_aliases`]). Every checked row
/// must still be pending and merge exactly the spellings it showed, so a
/// spelling that appeared after the page loaded is never merged. Edited
/// targets must be valid tags.
pub fn plan_reviewed_apply(
    rows: &[ReviewedRow],
    pending: &[TagMergeProposal],
) -> Result<ReviewedApply, DomainError> {
    if rows.is_empty() {
        return Err(DomainError::InvalidInput(
            "Check at least one merge.".into(),
        ));
    }
    let changed = || DomainError::InvalidInput(LIST_CHANGED.into());
    let mut targets: HashMap<Uuid, String> = HashMap::new();
    let mut edited = Vec::new();
    for row in rows {
        let proposal = pending
            .iter()
            .find(|p| p.id == row.id && p.status == ProposalStatus::Pending)
            .ok_or_else(changed)?;
        let shown: HashSet<&String> = row.shown.iter().collect();
        let now = row_spellings(proposal, pending);
        if now.len() != shown.len() || !now.iter().all(|s| shown.contains(s)) {
            return Err(changed());
        }
        let target = match &row.canonical {
            Some(submitted) => bounded_tag(submitted)
                .ok_or_else(|| {
                    DomainError::InvalidInput(format!(
                        "\"{}\" is not a valid tag name.",
                        submitted.trim()
                    ))
                })?
                .to_string(),
            None => proposal.canonical.clone(),
        };
        if target != proposal.canonical {
            edited.push((proposal.id, target.clone()));
        }
        targets.insert(proposal.id, target);
    }
    let selected: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let applied = expand_selection(&selected, pending);
    // One merge per checked row, of every spelling it showed. A row that a
    // checked AI row covers is applied through that row, so each spelling
    // has one target, even when the AI target was edited to a covered
    // spelling.
    let covering: Vec<&TagMergeProposal> = pending
        .iter()
        .filter(|p| selected.contains(&p.id))
        .collect();
    let merging: Vec<&TagMergeProposal> = covering
        .iter()
        .copied()
        .filter(|p| {
            !covering
                .iter()
                .any(|c| c.id != p.id && covered(c, pending).any(|v| v.id == p.id))
        })
        .collect();
    // Two checked AI rows that share a member both cover that member's
    // spelling row. Its spellings go with one of them only (the highest
    // confidence, then the first), so an edited target cannot give them two
    // targets; the other row's shared member still chains to its target.
    let mut by_rank = merging.clone();
    by_rank.sort_by_key(|p| p.confidence.rank());
    let mut owner: HashMap<Uuid, Uuid> = HashMap::new();
    for p in &by_rank {
        for v in covered(p, pending) {
            owner.entry(v.id).or_insert(p.id);
        }
    }
    let merges = merging
        .iter()
        .map(|p| TagMerge {
            canonical: targets[&p.id].clone(),
            members: unique(
                p.members.iter().chain(
                    covered(p, pending)
                        .filter(|v| owner[&v.id] == p.id)
                        .flat_map(|v| &v.members),
                ),
            ),
        })
        .collect();
    Ok(ReviewedApply {
        merges,
        kind: if applied.iter().any(|p| p.source == ProposalSource::Llm) {
            ChangeKind::LlmMerge
        } else {
            ChangeKind::Normalize
        },
        ids: applied.iter().map(|p| p.id).collect(),
        edited,
    })
}

/// The review screen's rows: "Check these" (`medium`, then `low`) first,
/// unchecked; then the `high` rows, checked. Within a section, the rows
/// that change the most bookmarks come first.
pub fn review_rows(pending: &[TagMergeProposal], samples: &[TagSample]) -> Vec<ReviewRow> {
    let by_tag: HashMap<&str, &TagSample> = samples.iter().map(|s| (s.tag.as_str(), s)).collect();
    let mut rows: Vec<ReviewRow> = pending
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending)
        .map(|proposal| {
            let mut spellings = row_spellings(proposal, pending);
            // The canonical first, then the most used.
            spellings.sort_by_key(|tag| {
                let count = by_tag.get(tag.as_str()).map_or(0, |s| s.count);
                (*tag != proposal.canonical, -count)
            });
            let chips: Vec<ReviewChip> = spellings
                .iter()
                .map(|tag| ReviewChip {
                    tag: tag.clone(),
                    count: by_tag.get(tag.as_str()).map_or(0, |s| s.count),
                })
                .collect();
            ReviewRow {
                id: proposal.id,
                source: proposal.source,
                confidence: proposal.confidence,
                canonical: proposal.canonical.clone(),
                bookmarks: chips
                    .iter()
                    .filter(|c| c.tag != proposal.canonical)
                    .map(|c| c.count)
                    .sum(),
                sample_title: spellings
                    .iter()
                    .find_map(|tag| by_tag.get(tag.as_str())?.sample_title.clone()),
                chips,
            }
        })
        .collect();
    let section = |c: Confidence| match c {
        Confidence::Medium => 0,
        Confidence::Low => 1,
        Confidence::High => 2,
    };
    rows.sort_by(|a, b| {
        section(a.confidence)
            .cmp(&section(b.confidence))
            .then_with(|| b.bookmarks.cmp(&a.bookmarks))
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    rows
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
        CandidatePair { a, b }
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
            pairs.iter().map(|p| (p.a, p.b)).collect::<Vec<_>>(),
            vec![(2, 3), (0, 1)]
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

    #[test]
    fn without_a_whole_list_choice_the_full_name_beats_a_more_used_short_form() {
        let units = units(&[("js", 9), ("javascript", 4), ("k8s", 3), ("kubernetes", 1)]);
        let pairs = vec![pair(0, 1), pair(2, 3)];
        let verdicts = vec![
            verdict(0, true, Confidence::High),
            verdict(1, true, Confidence::Medium),
        ];
        let canonicals: Vec<String> = build_proposals(&units, &[], &pairs, &verdicts)
            .into_iter()
            .map(|p| p.canonical)
            .collect();
        assert_eq!(canonicals, ["javascript", "kubernetes"]);
    }

    fn proposal(
        canonical: &str,
        members: &[&str],
        source: ProposalSource,
        confidence: Confidence,
    ) -> TagMergeProposal {
        TagMergeProposal {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            canonical: canonical.to_string(),
            members: s(members),
            source,
            confidence,
            status: ProposalStatus::Pending,
            batch_id: None,
            created_at: Utc::now(),
            decided_at: None,
        }
    }

    fn sample(tag: &str, count: i64, title: Option<&str>) -> TagSample {
        TagSample {
            tag: tag.to_string(),
            count,
            sample_title: title.map(str::to_string),
        }
    }

    #[test]
    fn review_lists_check_these_first_then_high() {
        use ProposalSource::*;
        let pending = vec![
            proposal(
                "github",
                &["GitHub", "github"],
                Deterministic,
                Confidence::High,
            ),
            proposal("javascript", &["javascript", "js"], Llm, Confidence::High),
            proposal(
                "remote work",
                &["remote work", "wfh"],
                Llm,
                Confidence::Medium,
            ),
            proposal("vim", &["neovim", "vim"], Llm, Confidence::Low),
        ];
        let samples = vec![
            sample("GitHub", 2, None),
            sample("github", 5, Some("Actions")),
            sample("javascript", 9, None),
            sample("js", 4, Some("Closures")),
            sample("remote work", 3, None),
            sample("wfh", 1, None),
            sample("neovim", 2, None),
            sample("vim", 6, None),
        ];
        let rows = review_rows(&pending, &samples);
        let summary: Vec<(&str, Confidence, i64)> = rows
            .iter()
            .map(|r| (r.canonical.as_str(), r.confidence, r.bookmarks))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("remote work", Confidence::Medium, 1),
                ("vim", Confidence::Low, 2),
                ("javascript", Confidence::High, 4),
                ("github", Confidence::High, 2),
            ]
        );
        assert_eq!(rows[2].sample_title.as_deref(), Some("Closures"));
        assert_eq!(
            rows[3].chips,
            vec![
                ReviewChip {
                    tag: "github".into(),
                    count: 5
                },
                ReviewChip {
                    tag: "GitHub".into(),
                    count: 2
                },
            ]
        );
    }

    fn ids(proposals: Vec<&TagMergeProposal>) -> Vec<Uuid> {
        proposals.iter().map(|p| p.id).collect()
    }

    #[test]
    fn an_llm_row_shows_and_applies_the_spelling_variants_of_its_members() {
        use ProposalSource::*;
        let variants = proposal(
            "github",
            &["GitHub", "github"],
            Deterministic,
            Confidence::High,
        );
        let llm = proposal("github", &["github", "gh"], Llm, Confidence::Medium);
        let other = proposal("rust", &["Rust", "rust"], Deterministic, Confidence::High);
        let pending = vec![variants.clone(), llm.clone(), other.clone()];

        let rows = review_rows(
            &pending,
            &[sample("GitHub", 2, None), sample("gh", 1, None)],
        );
        let llm_row = rows.iter().find(|r| r.id == llm.id).unwrap();
        let tags: Vec<&str> = llm_row.chips.iter().map(|c| c.tag.as_str()).collect();
        assert_eq!(tags, ["github", "GitHub", "gh"]);
        assert_eq!(llm_row.bookmarks, 3);

        assert_eq!(
            ids(expand_selection(&[llm.id], &pending)),
            vec![llm.id, variants.id]
        );
        assert_eq!(
            ids(expand_selection(&[variants.id, llm.id, other.id], &pending)),
            vec![variants.id, llm.id, other.id]
        );
        // Unknown ids are not invented.
        assert!(expand_selection(&[Uuid::new_v4()], &pending).is_empty());
    }

    #[test]
    fn a_run_left_running_too_long_reports_failed() {
        let started = Utc::now() - Duration::minutes(STALE_RUN_MINUTES + 1);
        let job = ConsolidationJob {
            run_id: Uuid::nil(),
            status: JobStatus::Running,
            started_at: started,
            finished_at: None,
            error: None,
            proposals: 0,
            pairs_checked: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        let settled = job.clone().settle(Utc::now());
        assert_eq!(settled.status, JobStatus::Failed);
        assert!(settled.error.is_some());
        let fresh = ConsolidationJob {
            started_at: Utc::now(),
            ..job
        };
        assert_eq!(fresh.settle(Utc::now()).status, JobStatus::Running);
    }

    fn shown(proposal: &TagMergeProposal, pending: &[TagMergeProposal]) -> ReviewedRow {
        ReviewedRow {
            id: proposal.id,
            shown: row_spellings(proposal, pending),
            canonical: None,
        }
    }

    #[test]
    fn apply_merges_exactly_what_was_shown_with_edited_targets() {
        use ProposalSource::*;
        let variants = proposal("js", &["JS", "js"], Deterministic, Confidence::High);
        let llm = proposal("javascript", &["javascript", "js"], Llm, Confidence::High);
        let pending = vec![variants.clone(), llm.clone()];
        let mut row = shown(&llm, &pending);
        assert_eq!(row.shown, s(&["javascript", "js", "JS"]));
        row.canonical = Some(" JavaScript ".into());

        let plan = plan_reviewed_apply(&[row.clone()], &pending).unwrap();
        assert_eq!(plan.ids, vec![llm.id, variants.id]);
        assert_eq!(plan.kind, ChangeKind::LlmMerge);
        assert_eq!(plan.edited, vec![(llm.id, "JavaScript".to_string())]);
        // One merge of every spelling the row showed.
        assert_eq!(
            plan.merges,
            vec![TagMerge {
                canonical: "JavaScript".into(),
                members: s(&["javascript", "js", "JS"])
            }]
        );

        // The target may be one of the covered spellings: no false cycle.
        let mut to_covered = shown(&llm, &pending);
        to_covered.canonical = Some("JS".into());
        let plan = plan_reviewed_apply(&[to_covered], &pending).unwrap();
        let mapping = crate::domain::tag_alias::merge_mapping(&plan.merges).unwrap();
        assert_eq!(mapping["javascript"], "JS");
        assert_eq!(mapping["js"], "JS");

        // Checking the covered row too changes nothing.
        let both = [shown(&llm, &pending), shown(&variants, &pending)];
        let plan = plan_reviewed_apply(&both, &pending).unwrap();
        assert_eq!(plan.merges.len(), 1);
        assert_eq!(plan.ids.len(), 2);

        // An invalid edited target is refused.
        row.canonical = Some("bad\ntag".into());
        assert!(plan_reviewed_apply(&[row], &pending).is_err());
    }

    #[test]
    fn a_spelling_row_covered_by_two_checked_rows_gets_one_target() {
        use ProposalSource::*;
        // A medium row joined the high group by its canonical, so both AI
        // rows have `javascript` and both cover its spelling row.
        let variants = proposal(
            "javascript",
            &["JavaScript", "javascript"],
            Deterministic,
            Confidence::High,
        );
        let medium = proposal(
            "javascript",
            &["ecmascript", "javascript"],
            Llm,
            Confidence::Medium,
        );
        let high = proposal("javascript", &["javascript", "js"], Llm, Confidence::High);
        let pending = vec![variants.clone(), medium.clone(), high.clone()];
        let mut edited = shown(&medium, &pending);
        assert_eq!(edited.shown, s(&["ecmascript", "javascript", "JavaScript"]));
        edited.canonical = Some("ECMAScript".into());
        let rows = [edited, shown(&high, &pending)];

        let plan = plan_reviewed_apply(&rows, &pending).unwrap();
        // The spelling row goes with the high row only, listed first in
        // `pending` or not.
        assert_eq!(
            plan.merges,
            vec![
                TagMerge {
                    canonical: "ECMAScript".into(),
                    members: s(&["ecmascript", "javascript"]),
                },
                TagMerge {
                    canonical: "javascript".into(),
                    members: s(&["javascript", "js", "JavaScript"]),
                },
            ]
        );
        assert_eq!(plan.ids.len(), 3);
        // Every spelling has one target: the edited one, through the chain.
        let mapping = crate::domain::tag_alias::merge_mapping(&plan.merges).unwrap();
        for tag in ["JavaScript", "javascript", "js", "ecmascript"] {
            assert_eq!(mapping[tag], "ECMAScript", "{tag}");
        }

        // With equal confidence, the first checked row takes it.
        let other = proposal(
            "javascript",
            &["ecmascript", "javascript"],
            Llm,
            Confidence::High,
        );
        let pending = vec![variants.clone(), other.clone(), high.clone()];
        let rows = [shown(&other, &pending), shown(&high, &pending)];
        let plan = plan_reviewed_apply(&rows, &pending).unwrap();
        assert_eq!(
            plan.merges[0].members,
            s(&["ecmascript", "javascript", "JavaScript"])
        );
        assert_eq!(plan.merges[1].members, s(&["javascript", "js"]));
    }

    #[test]
    fn apply_refuses_a_row_that_changed_since_the_page_loaded() {
        use ProposalSource::*;
        let llm = proposal("javascript", &["javascript", "js"], Llm, Confidence::High);
        let row = shown(&llm, std::slice::from_ref(&llm));
        assert_eq!(row.shown, s(&["javascript", "js"]));

        // A spelling-variant row for `js` appeared after the page loaded:
        // applying the old row would merge `JS`, which the user never saw.
        let variants = proposal("js", &["JS", "js"], Deterministic, Confidence::High);
        let now = vec![llm.clone(), variants];
        let error = plan_reviewed_apply(std::slice::from_ref(&row), &now).unwrap_err();
        assert!(matches!(error, DomainError::InvalidInput(m) if m == LIST_CHANGED));

        // A checked row that is no longer pending is refused before any
        // expansion, even next to a row that is still fine.
        let other = proposal("rust", &["Rust", "rust"], Deterministic, Confidence::High);
        let fine = shown(&other, std::slice::from_ref(&other));
        assert!(plan_reviewed_apply(&[fine.clone(), row], std::slice::from_ref(&other)).is_err());
        assert!(plan_reviewed_apply(&[fine], &[other]).is_ok());
    }

    #[test]
    fn proposals_follow_merges_made_since_but_keep_spellings_still_in_use() {
        use ProposalSource::*;
        let alias = |spelling: &str, canonical: &str| TagAlias {
            alias_key: compact_key(spelling),
            alias: spelling.to_string(),
            canonical: canonical.to_string(),
            source: crate::domain::tag_alias::AliasSource::Manual,
            batch_id: None,
        };
        // `JS, js` was merged into `JS` (an edited target); `js` is gone.
        let llm = proposal("javascript", &["javascript", "js"], Llm, Confidence::High);
        let on_bookmarks: HashSet<String> = ["JS", "javascript"].map(String::from).into();
        let resolved = resolve_through_aliases(
            std::slice::from_ref(&llm),
            &[alias("js", "JS")],
            &on_bookmarks,
        );
        assert_eq!(resolved[0].members, s(&["javascript", "JS"]));
        assert_eq!(resolved[0].id, llm.id);

        // A spelling still on bookmarks keeps its proposal, even when an
        // older alias row points its key elsewhere.
        let variants = proposal(
            "github",
            &["GitHub", "github"],
            Deterministic,
            Confidence::High,
        );
        let on_bookmarks: HashSet<String> = ["GitHub", "github"].map(String::from).into();
        let resolved = resolve_through_aliases(
            std::slice::from_ref(&variants),
            &[alias("github", "github")],
            &on_bookmarks,
        );
        assert_eq!(resolved, vec![variants]);

        // A whitespace-only variant on bookmarks merges with its member.
        let on_bookmarks: HashSet<String> = ["javascript", " js "].map(String::from).into();
        let resolved = resolve_through_aliases(std::slice::from_ref(&llm), &[], &on_bookmarks);
        assert_eq!(resolved[0].members, s(&["javascript", "js", " js "]));
        let row = shown(&resolved[0], &resolved);
        let plan = plan_reviewed_apply(&[row], &resolved).unwrap();
        assert_eq!(plan.merges[0].members, s(&["javascript", "js", " js "]));

        // Nothing left to merge: the proposal drops out.
        let gone = proposal("rust", &["Rust", "rust"], Deterministic, Confidence::High);
        let on_bookmarks: HashSet<String> = ["rust"].map(String::from).into();
        assert!(
            resolve_through_aliases(&[gone], &[alias("Rust", "rust")], &on_bookmarks).is_empty()
        );
    }
}
