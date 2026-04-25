use std::collections::HashMap;

/// Compute the new tag list for a bookmark.
///
/// For each current tag:
/// - Look up its mapping. If absent or empty, treat as identity (`[tag]`).
/// - Collect every value from every mapping output for the bookmark's tags.
/// - Lowercase, dedupe (case-insensitive), and sort lexicographically.
pub fn compute_new_tags(
    current: &[String],
    mapping: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut acc: BTreeSet<String> = BTreeSet::new();
    for tag in current {
        let outputs = match mapping.get(tag) {
            Some(values) if !values.is_empty() => values.clone(),
            _ => vec![tag.clone()],
        };
        for out in outputs {
            let normalized = out.trim().to_lowercase();
            if !normalized.is_empty() {
                acc.insert(normalized);
            }
        }
    }
    acc.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, vs)| ((*k).to_string(), vs.iter().map(|v| (*v).to_string()).collect()))
            .collect()
    }

    #[test]
    fn merges_variants_into_one_canonical() {
        let mapping = map(&[
            ("js", &["javascript"]),
            ("javascript", &["javascript"]),
            ("JavaScript", &["javascript"]),
        ]);
        let result =
            compute_new_tags(&["js".into(), "JavaScript".into()], &mapping);
        assert_eq!(result, vec!["javascript".to_string()]);
    }

    #[test]
    fn adds_parent_tag_alongside_narrow_tag() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn omitted_tag_is_treated_as_identity() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into(), "rust".into()], &mapping);
        assert_eq!(
            result,
            vec!["frontend".to_string(), "react".to_string(), "rust".to_string()]
        );
    }

    #[test]
    fn empty_mapping_value_is_treated_as_identity() {
        let mapping = map(&[("react", &[])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["react".to_string()]);
    }

    #[test]
    fn outputs_are_lowercased() {
        let mapping = map(&[("react", &["React", "FRONTEND"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn deduplicates_case_insensitively() {
        let mapping = map(&[
            ("react", &["react", "frontend"]),
            ("vue", &["vue", "Frontend"]),
        ]);
        let result = compute_new_tags(&["react".into(), "vue".into()], &mapping);
        assert_eq!(
            result,
            vec!["frontend".to_string(), "react".to_string(), "vue".to_string()]
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        let mapping = map(&[("react", &["react"])]);
        let result = compute_new_tags(&[], &mapping);
        assert!(result.is_empty());
    }
}
