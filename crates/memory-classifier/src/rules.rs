//! Transparent rule-based classification and extraction.
//!
//! Every rule is a readable pattern list. Deterministic, auditable,
//! and instant — the right default when no model is available or
//! wanted.

use crate::classification::{Classification, ClassifierInput, MemoryClassifier};
use crate::extraction::{ExtractedMemory, ExtractionProvider};
use async_trait::async_trait;
use memory_domain::MemoryResult;
use memory_domain::MemoryType;

/// Preference statements.
const PREFERENCE_PATTERNS: [&str; 5] = ["prefers", "prefer", "likes", "dislikes", "favorite"];
/// Commitments and obligations.
const PROSPECTIVE_PATTERNS: [&str; 6] = [
    "will send",
    "needs to",
    "must",
    "deadline",
    "by friday",
    "follow up",
];
/// Skills and how-tos.
const PROCEDURAL_PATTERNS: [&str; 4] = ["how to deploy", "steps to", "procedure for", "runbook"];

/// Rule-based classifier; the engine's zero-LLM default.
#[derive(Clone, Debug, Default)]
pub struct RuleBasedClassifier {
    /// Extra caller-supplied patterns treated as preferences.
    pub preference_terms: Vec<String>,
}

impl RuleBasedClassifier {
    /// Extends the preference vocabulary with domain terms.
    pub fn with_preference_terms(mut self, terms: Vec<String>) -> Self {
        self.preference_terms = terms;
        self
    }

    fn classify_text(&self, text: &str, hint: Option<MemoryType>) -> Classification {
        let lower = text.to_lowercase();

        // Caller hints always win — the classifier exists to help
        // callers who don't know, not to second-guess those who do.
        if let Some(t) = hint {
            return Classification {
                memory_type: t,
                subtype: None,
                confidence: 1.0,
                keywords: keywords(text),
            };
        }

        if PROCEDURAL_PATTERNS.iter().any(|p| lower.contains(p)) {
            return Classification {
                memory_type: MemoryType::Procedural,
                subtype: Some("how-to".into()),
                confidence: 0.7,
                keywords: keywords(text),
            };
        }
        if PROSPECTIVE_PATTERNS.iter().any(|p| lower.contains(p)) {
            return Classification {
                memory_type: MemoryType::Prospective,
                subtype: Some("commitment".into()),
                confidence: 0.65,
                keywords: keywords(text),
            };
        }
        let preference_hit = PREFERENCE_PATTERNS
            .iter()
            .copied()
            .chain(self.preference_terms.iter().map(|s| s.as_str()))
            .any(|p| lower.contains(p));
        if preference_hit {
            return Classification {
                memory_type: MemoryType::Semantic,
                subtype: Some("preference".into()),
                confidence: 0.75,
                keywords: keywords(text),
            };
        }

        Classification {
            memory_type: MemoryType::Semantic,
            subtype: None,
            confidence: 0.4,
            keywords: keywords(text),
        }
    }
}

#[async_trait]
impl MemoryClassifier for RuleBasedClassifier {
    fn name(&self) -> &str {
        "rule-based"
    }

    async fn classify(&self, input: &ClassifierInput) -> MemoryResult<Classification> {
        Ok(self.classify_text(&input.text, input.hinted_type))
    }
}

/// Sentence-splitting rule-based extractor.
///
/// Keeps statements that look like durable facts (preferences,
/// commitments) and drops pure small talk by pattern absence plus
/// length bounds. Deliberately conservative: false negatives are
/// cheaper than junk memories.
pub struct RuleBasedExtractor;

#[async_trait]
impl ExtractionProvider for RuleBasedExtractor {
    fn name(&self) -> &str {
        "rule-based"
    }

    async fn extract(&self, text: &str) -> MemoryResult<Vec<ExtractedMemory>> {
        let mut out = Vec::new();
        for sentence in split_sentences(text) {
            if sentence.len() < 8 || sentence.len() > 300 {
                continue;
            }
            let c = self.classify_defaults(&sentence);
            if let Some(c) = c {
                out.push(ExtractedMemory {
                    text: sentence,
                    memory_type: c.memory_type,
                    subtype: c.subtype,
                    confidence: c.confidence,
                });
            }
        }
        Ok(out)
    }
}

impl RuleBasedExtractor {
    fn classify_defaults(&self, sentence: &str) -> Option<Classification> {
        let classifier = RuleBasedClassifier::default();
        let c = classifier.classify_text(sentence, None);
        match c.memory_type {
            MemoryType::Semantic if c.subtype.is_none() => None, // plain talk
            _ => Some(c),
        }
    }
}

fn split_sentences(text: &str) -> Vec<String> {
    text.split(['.', '\n', '!', '?'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extracts up to 8 lowercase keyword tokens for indexing.
pub(crate) fn keywords(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(str::to_string)
        .take(8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preferences_are_detected_with_subtype() {
        let c = RuleBasedClassifier::default();
        let d = c
            .classify(&ClassifierInput::text("Customer prefers email contact"))
            .await
            .expect("classify");
        assert_eq!(d.memory_type, MemoryType::Semantic);
        assert_eq!(d.subtype.as_deref(), Some("preference"));
        assert!(d.confidence >= 0.7);
    }

    #[tokio::test]
    async fn commitments_route_to_prospective() {
        let c = RuleBasedClassifier::default();
        let d = c
            .classify(&ClassifierInput::text("I will send the report by friday"))
            .await
            .expect("classify");
        assert_eq!(d.memory_type, MemoryType::Prospective);
    }

    #[tokio::test]
    async fn procedures_route_to_procedural() {
        let c = RuleBasedClassifier::default();
        let d = c
            .classify(&ClassifierInput::text("How to deploy the staging cluster"))
            .await
            .expect("classify");
        assert_eq!(d.memory_type, MemoryType::Procedural);
    }

    #[tokio::test]
    async fn caller_hints_are_never_overridden() {
        let c = RuleBasedClassifier::default();
        let input = ClassifierInput {
            hinted_type: Some(MemoryType::Episodic),
            ..ClassifierInput::text("Customer prefers email")
        };
        let d = c.classify(&input).await.expect("classify");
        assert_eq!(d.memory_type, MemoryType::Episodic);
        assert_eq!(d.confidence, 1.0);
    }

    #[tokio::test]
    async fn custom_preference_terms_extend_vocabulary() {
        let c = RuleBasedClassifier::default()
            .with_preference_terms(vec!["loves".into(), "allergic to".into()]);
        let d = c
            .classify(&ClassifierInput::text("User is allergic to peanuts"))
            .await
            .expect("classify");
        assert_eq!(d.subtype.as_deref(), Some("preference"));
    }

    #[tokio::test]
    async fn extractor_pulls_facts_and_skips_small_talk() {
        let x = RuleBasedExtractor;
        let found = x
            .extract("Hi there! The customer prefers email over phone. Nice weather today. She must receive invoices by friday.")
            .await
            .expect("extract");

        let texts: Vec<&str> = found.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("prefers email")),
            "preference must be extracted, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("invoices")),
            "commitment must be extracted"
        );
        assert!(
            !texts
                .iter()
                .any(|t: &&str| t.eq_ignore_ascii_case("nice weather today")),
            "small talk must not become memory"
        );
    }

    #[test]
    fn keywords_cap_at_eight() {
        let kws = keywords("alpha beta gamma delta epsilon zeta eta theta iota kappa");
        assert_eq!(kws.len(), 8);
        assert_eq!(kws.first().unwrap(), "alpha");
    }
}
