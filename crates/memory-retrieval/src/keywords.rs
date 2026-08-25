//! Query keyword extraction shared by planner steps.
//!
//! Extracted from the planner so the heuristics are unit-testable and
//! reusable by the executor's keyword step without re-deriving them.

/// Extracts meaningful keywords from a query text.
///
/// Drops stopwords and short tokens; keeps original casing folded away
/// since canonical stores match case-insensitively.
pub fn extract(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() > 2 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "what", "when",
    "where", "which", "who", "how", "does", "did", "are", "was", "were",
    "has", "have", "had", "you", "your", "our", "their",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stopwords_and_punctuation() {
        let kws = extract("What database does the Atlas project use?");
        assert_eq!(kws, vec!["database", "atlas", "project", "use"]);
    }

    #[test]
    fn ignores_short_tokens() {
        assert!(extract("a an the of to at is").is_empty());
    }

    #[test]
    fn preserves_order_and_folds_case() {
        assert_eq!(
            extract("PostgreSQL Redis PostgreSQL"),
            vec!["postgresql", "redis", "postgresql"]
        );
    }

    #[test]
    fn empty_input_yields_no_keywords() {
        assert!(extract("").is_empty());
        assert!(extract("   ...  ").is_empty());
    }
}
