//! Rule-based relation extraction from subject-anchored statements.
//!
//! Patterns are deliberately narrow: "Atlas uses PostgreSQL" yields
//! USES; everything ambiguous is skipped. Wrong edges poison graph
//! traversal, so precision beats recall here.

use crate::graph::{EntityKey, Relation};
use async_trait::async_trait;
use memory_domain::{MemoryId, MemoryResult, MemorySubject};

/// Extracts relations from memory content.
#[async_trait]
pub trait RelationExtractor: Send + Sync {
    fn name(&self) -> &str;

    /// Extracts candidate relations justified by one canonical memory.
    async fn extract(
        &self,
        evidence: MemoryId,
        subject: &MemorySubject,
        text: &str,
    ) -> MemoryResult<Vec<Relation>>;
}

/// Cue words mapping to relation types.
const RELATION_CUES: [(&str, &str); 5] = [
    (" uses ", "USES"),
    (" runs on ", "RUNS_ON"),
    (" owned by ", "OWNED_BY"),
    (" deployed to ", "DEPLOYED_TO"),
    (" hosted in ", "HOSTED_IN"),
];

/// Pattern-based extractor: the sentence must name the subject and a
/// cue verb phrase, with the object following the cue.
pub struct RuleBasedRelationExtractor;

#[async_trait]
impl RelationExtractor for RuleBasedRelationExtractor {
    fn name(&self) -> &str {
        "rule-based"
    }

    async fn extract(
        &self,
        evidence: MemoryId,
        subject: &MemorySubject,
        text: &str,
    ) -> MemoryResult<Vec<Relation>> {
        let lower = format!(" {} ", text.to_lowercase());
        let mut out = Vec::new();

        for (cue, relation_type) in RELATION_CUES {
            if let Some(after) = lower.split(cue).nth(1) {
                let object_words: Vec<&str> =
                    after.split_whitespace().take(3).collect();
                if object_words.is_empty() {
                    continue;
                }
                // Object = first word (multi-word names need an NLP
                // extractor; rules stay conservative).
                let object = object_words[0]
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if object.is_empty() || object.len() < 2 {
                    continue;
                }
                let from = EntityKey::from_subject(subject);
                let to = EntityKey::new(None, &object);
                if let Ok(relation) = Relation::new(evidence, from, relation_type, to) {
                    out.push(relation);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uses_pattern_yields_uses_edge() {
        let x = RuleBasedRelationExtractor;
        let subject = MemorySubject::new("atlas").with_type("project");
        let rels = x
            .extract(MemoryId::generate(), &subject, "Project Atlas uses PostgreSQL")
            .await
            .expect("extract");

        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].relation_type, "USES");
        assert_eq!(rels[0].from.0, "project:atlas");
        assert_eq!(rels[0].to.0, "postgresql");
    }

    #[tokio::test]
    async fn no_cue_means_no_edge() {
        let x = RuleBasedRelationExtractor;
        let subject = MemorySubject::new("atlas");
        let rels = x
            .extract(MemoryId::generate(), &subject, "Atlas had a busy week")
            .await
            .expect("extract");
        assert!(rels.is_empty());
    }

    #[tokio::test]
    async fn owned_by_points_at_the_owner() {
        let x = RuleBasedRelationExtractor;
        let subject = MemorySubject::new("atlas").with_type("project");
        let rels = x
            .extract(MemoryId::generate(), &subject, "Atlas is owned by platform")
            .await
            .expect("extract");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].relation_type, "OWNED_BY");
        assert_eq!(rels[0].to.0, "platform");
    }
}
