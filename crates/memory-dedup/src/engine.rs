//! The decision cascade: signals in, one action out.
//!
//! Order matters and is deliberate:
//! 1. exact hash      -> Ignore (byte-identical)
//! 2. normalized hash -> Ignore (wording differs, statement identical)
//! 3. canonical key + semantic similarity + temporal overlap
//!                    -> Merge (same fact, newer wording)
//! 4. entity overlap high but facts differ -> Link
//! 5. similarity near threshold -> Review
//! 6. otherwise -> Add

use crate::decision::{DedupAction, DedupDecision};
use crate::signals::{exact_hash, normalized_hash};
use chrono::Utc;
use memory_domain::{MemoryRecord, MemorySubject};

/// Similarity above which same-subject records are considered the same
/// fact worth merging.
pub const MERGE_SIMILARITY_THRESHOLD: f32 = 0.90;
/// Band under the merge threshold where decisions become ambiguous.
pub const REVIEW_SIMILARITY_BAND: f32 = 0.10;
/// Subject-key overlap fraction for treating records as related.
pub const ENTITY_OVERLAP_LINK_THRESHOLD: f32 = 0.8;

/// Stateless evaluator; candidates come pre-filtered by scope/type so
/// this stays a pure function of its inputs.
#[derive(Clone, Copy, Debug, Default)]
pub struct DedupEngine {
    /// Optional override for the merge threshold (tests, tuning).
    pub merge_threshold: Option<f32>,
}

impl DedupEngine {
    /// Evaluates an incoming record against existing candidates.
    ///
    /// `similarity_of` supplies semantic similarity lazily so callers
    /// without embeddings can still run the structural cascade.
    pub fn evaluate<S>(
        &self,
        incoming: &MemoryRecord,
        candidates: &[MemoryRecord],
        mut similarity_of: S,
    ) -> DedupDecision
    where
        S: FnMut(&MemoryRecord) -> f32,
    {
        let (incoming_exact, incoming_norm) =
            crate::signals::text_fingerprint(&incoming.content.text);
        let merge_threshold = self.merge_threshold.unwrap_or(MERGE_SIMILARITY_THRESHOLD);

        for candidate in candidates {
            // 1. Byte-identical content on the same subject is a dup.
            if candidate.subject.is_some()
                && subject_keys_equal(candidate.subject.as_ref(), incoming.subject.as_ref())
                && exact_hash(&candidate.content.text) == incoming_exact
            {
                return DedupDecision {
                    action: DedupAction::Ignore,
                    target: Some(candidate.id),
                    reason: "exact hash match".into(),
                };
            }

            // 2. Wording differs, statement identical.
            if normalized_hash(&candidate.content.text) == incoming_norm
                && types_compatible(candidate, incoming)
            {
                return DedupDecision {
                    action: DedupAction::Ignore,
                    target: Some(candidate.id),
                    reason: "normalized text hash match".into(),
                };
            }
        }

        let mut best: Option<(&MemoryRecord, f32)> = None;
        for candidate in candidates {
            let sim = similarity_of(candidate);
            if best.is_none_or(|(_, s)| sim > s) {
                best = Some((candidate, sim));
            }
        }

        if let Some((best_candidate, sim)) = best {
            let same_subject = both_have_subject(best_candidate, incoming)
                && subject_keys_equal(
                    best_candidate.subject.as_ref(),
                    incoming.subject.as_ref(),
                );
            // Merge targets must still be open facts: a closed historical
            // era never absorbs a current statement — history stays put.
            let temporal_compatible =
                !best_candidate.validity().has_ended(Utc::now());

            if same_subject && sim >= merge_threshold && temporal_compatible {
                // 3. Same fact about the same subject, newer wording:
                //    merge as supersession via the update path.
                return DedupDecision {
                    action: DedupAction::Merge,
                    target: Some(best_candidate.id),
                    reason: format!(
                        "same subject with similarity {sim:.2} >= {merge_threshold:.2} and overlapping validity"
                    ),
                };
            }

            if sim < merge_threshold && sim >= merge_threshold - REVIEW_SIMILARITY_BAND {
                // 5. Ambiguous: near-threshold similarity quarantines.
                return DedupDecision {
                    action: DedupAction::Review,
                    target: Some(best_candidate.id),
                    reason: format!("similarity {sim:.2} within review band"),
                };
            }

            if entity_overlap(best_candidate.subject.as_ref(), incoming.subject.as_ref())
                >= ENTITY_OVERLAP_LINK_THRESHOLD
            {
                // 4. Related subjects, distinct statements: keep both.
                return DedupDecision {
                    action: DedupAction::Link,
                    target: Some(best_candidate.id),
                    reason: "high entity overlap with distinct content".into(),
                };
            }
        }

        DedupDecision::add()
    }
}

fn keywords(record_subject: Option<&MemorySubject>) -> Vec<String> {
    record_subject.map(|s| s.canonical_key()).into_iter().collect()
}

fn subject_keys_equal(a: Option<&MemorySubject>, b: Option<&MemorySubject>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.canonical_key() == y.canonical_key(),
        _ => false,
    }
}

fn both_have_subject(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    a.subject.is_some() && b.subject.is_some()
}

fn types_compatible(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    a.memory_type == b.memory_type
}

/// Overlap between the keyword sets of two records' subjects and tags;
/// 0.0 when either side has no signal.
///
/// Uses canonical subject keys plus content keywords — cheap structure,
/// not embeddings.
fn entity_overlap(a: Option<&MemorySubject>, b: Option<&MemorySubject>) -> f32 {
    let ka = keywords(a);
    let kb = keywords(b);
    if ka.is_empty() || kb.is_empty() {
        return 0.0;
    }
    let shared = ka.iter().filter(|k| kb.contains(k)).count();
    shared as f32 / ka.len().max(kb.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use memory_domain::{MemoryContent, MemoryType};

    fn record(text: &str, subject_id: &str) -> MemoryRecord {
        MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text(text))
            .with_subject(MemorySubject::new(subject_id).with_type("project"))
    }

    #[test]
    fn no_candidates_means_add() {
        let engine = DedupEngine::default();
        let d = engine.evaluate(&record("brand new", "x"), &[], |_| 0.0);
        assert_eq!(d.action, DedupAction::Add);
    }

    #[test]
    fn byte_identical_content_ignores() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas uses PostgreSQL", "atlas")];
        let d = engine.evaluate(&record("Atlas uses PostgreSQL", "atlas"), &existing, |_| 1.0);
        assert_eq!(d.action, DedupAction::Ignore);
        assert!(d.reason.contains("exact"));
    }

    #[test]
    fn reworded_identical_statement_ignores() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas uses PostgreSQL", "atlas")];
        let incoming = record("atlas uses POSTGRESQL", "atlas");
        let d = engine.evaluate(&incoming, &existing, |_| 1.0);
        assert_eq!(d.action, DedupAction::Ignore);
        assert!(d.reason.contains("normalized"));
    }

    #[test]
    fn same_subject_high_similarity_overlapping_validity_merges() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas runs its workloads on PostgreSQL", "atlas")];
        let incoming = record("Project Atlas does its database work in PostgreSQL", "atlas");

        // Hashes differ; similarity must carry the decision.
        let d = engine.evaluate(&incoming, &existing, |_| 0.93);
        assert_eq!(d.action, DedupAction::Merge);
        assert_eq!(d.target, Some(existing[0].id));
    }

    #[test]
    fn similarity_alone_without_same_subject_never_merges() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas runs its workloads on PostgreSQL", "other-project")];
        let incoming = record("Project Atlas does its database work in PostgreSQL", "atlas");
        let d = engine.evaluate(&incoming, &existing, |_| 0.99);
        assert_ne!(d.action, DedupAction::Merge, "similarity alone must not merge");
    }

    #[test]
    fn near_threshold_similarity_requires_review() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas uses PostgreSQL 16", "atlas")];
        let incoming = record("Atlas uses PostgreSQL 17 maybe", "atlas");
        let d = engine.evaluate(&incoming, &existing, |_| MERGE_SIMILARITY_THRESHOLD - 0.05);
        assert_eq!(d.action, DedupAction::Review);
    }

    #[test]
    fn clearly_different_content_links_or_adds() {
        let engine = DedupEngine::default();
        let existing = vec![record("Atlas uses PostgreSQL", "atlas")];
        let incoming = record("Atlas is owned by the platform team", "atlas");
        let d = engine.evaluate(&incoming, &existing, |_| 0.3);
        assert!(
            matches!(d.action, DedupAction::Link | DedupAction::Add),
            "distinct statements stay distinct, got {d:?}"
        );
    }

    #[test]
    fn non_overlapping_temporal_windows_do_not_merge() {
        let engine = DedupEngine::default();
        let mut historical = record("Atlas used MySQL back then", "atlas");
        historical.valid_to = Some(Utc::now() - chrono::Duration::days(30));
        let incoming = record("Atlas now standardizes on MySQL everywhere", "atlas");
        let d = engine.evaluate(&incoming, &[historical], |_| 0.95);
        assert_ne!(d.action, DedupAction::Merge, "past-era facts must not absorb current ones");
    }
}
