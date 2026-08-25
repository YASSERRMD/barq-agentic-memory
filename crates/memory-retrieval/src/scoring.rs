//! Candidate scoring: one composite, explainable formula.
//!
//! Weights are fixed and public rather than tuned per-request so
//! rankings stay predictable and debuggable. Every component is
//! normalized to [0, 1] before weighting.

use memory_domain::{MemoryRecord, SourceKind};
use serde::{Deserialize, Serialize};

/// Component weights for the composite score.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreWeights {
    /// Similarity (semantic or keyword overlap), [0, 1].
    pub similarity: f32,
    /// Recency of the record's creation.
    pub recency: f32,
    /// Caller-assigned importance.
    pub importance: f32,
    /// Caller-assigned confidence.
    pub confidence: f32,
    /// Source authority ordering from provenance.
    pub authority: f32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            similarity: 0.55,
            recency: 0.15,
            importance: 0.12,
            confidence: 0.10,
            authority: 0.08,
        }
    }
}

/// Context needed to score a candidate fairly against its peers.
#[derive(Clone, Copy, Debug)]
pub struct ScoreContext<'a> {
    /// Similarity the retrieval step already assigned, in [0, 1].
    pub similarity: f32,
    /// Reference instant for recency decay.
    pub now: chrono::DateTime<chrono::Utc>,
    /// Half-life for exponential recency decay; longer = flatter.
    ///
    /// Tied to the reference so scoring is stable across calls: a
    /// memory's relative position shifts smoothly as `now` advances.
    pub half_life: &'a chrono::Duration,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> ScoreContext<'a> {
    pub fn new(
        similarity: f32,
        now: chrono::DateTime<chrono::Utc>,
        half_life: &'a chrono::Duration,
    ) -> Self {
        Self {
            similarity: similarity.clamp(0.0, 1.0),
            now,
            half_life,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Computes the composite score for one candidate.
pub fn score(record: &MemoryRecord, ctx: &ScoreContext<'_>, weights: &ScoreWeights) -> f32 {
    let recency = recency_score(record.created_at, ctx.now, ctx.half_life);
    let importance = record.importance.clamp(0.0, 1.0);
    let confidence = record.confidence.clamp(0.0, 1.0);
    let authority = authority_score(record.provenance.source.clone());

    weights.similarity * ctx.similarity
        + weights.recency * recency
        + weights.importance * importance
        + weights.confidence * confidence
        + weights.authority * authority
}

/// Exponential decay with configurable half-life.
fn recency_score(
    created: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    half_life: &chrono::Duration,
) -> f32 {
    let age = (now - created).num_seconds().max(0) as f64;
    let half = half_life.num_seconds().max(1) as f64;
    let s = (-age / half).exp();
    s as f32
}

/// Authority normalized into [0, 1] using the domain's fixed ordering
/// (user > external > tool > system > agent).
fn authority_score(source: SourceKind) -> f32 {
    source.default_authority() / 1.0
}

/// Temporal relevance: facts whose validity window is currently open
/// score full; historical windows decay by distance from the snapshot.
pub fn temporal_relevance(record: &MemoryRecord, at: chrono::DateTime<chrono::Utc>) -> f32 {
    if record.is_valid_at(at) {
        return 1.0;
    }
    // Retired-but-included candidates degrade instead of vanishing.
    // Fractional days keep sub-day staleness distinguishable.
    match (record.valid_from, record.valid_to) {
        (_, Some(to)) if to <= at => {
            let days_past = ((at - to).num_seconds().max(0) as f32) / 86_400.0;
            (1.0 / (1.0 + days_past / 30.0)).clamp(0.05, 1.0)
        }
        (Some(from), _) if from > at => {
            let days_ahead = ((from - at).num_seconds().max(0) as f32) / 86_400.0;
            (1.0 / (1.0 + days_ahead / 30.0)).clamp(0.05, 1.0)
        }
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use memory_domain::{MemoryContent, MemoryType, Provenance, RetentionPolicy};

    fn record_with(source: SourceKind, age_days: i64, importance: f32) -> MemoryRecord {
        let mut r = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("candidate"));
        r.created_at = Utc::now() - Duration::days(age_days);
        r.importance = importance;
        r.provenance = Provenance::now(source.clone());
        r.retention = RetentionPolicy::standard();
        r
    }

    const HL: chrono::Duration = Duration::days(30);

    #[test]
    fn identical_context_identical_scores() {
        let w = ScoreWeights::default();
        let now = Utc::now();
        let a = record_with(SourceKind::User, 3, 0.8);
        let b = record_with(SourceKind::User, 3, 0.8);
        let sa = score(&a, &ScoreContext::new(0.9, now, &HL), &w);
        let sb = score(&b, &ScoreContext::new(0.9, now, &HL), &w);
        assert_eq!(sa, sb);
    }

    #[test]
    fn fresher_records_outscore_stale_ones() {
        let w = ScoreWeights::default();
        let now = Utc::now();
        let fresh = record_with(SourceKind::Agent, 1, 0.5);
        let stale = record_with(SourceKind::Agent, 400, 0.5);
        let sf = score(&fresh, &ScoreContext::new(0.7, now, &HL), &w);
        let ss = score(&stale, &ScoreContext::new(0.7, now, &HL), &w);
        assert!(sf > ss);
    }

    #[test]
    fn authority_breaks_similarity_ties_toward_users() {
        let w = ScoreWeights::default();
        let now = Utc::now();
        let user_fact = record_with(SourceKind::User, 10, 0.5);
        let agent_guess = record_with(SourceKind::Agent, 10, 0.5);
        let su = score(&user_fact, &ScoreContext::new(0.6, now, &HL), &w);
        let sg = score(&agent_guess, &ScoreContext::new(0.6, now, &HL), &w);
        assert!(su > sg);
    }

    #[test]
    fn currently_valid_facts_beat_expired_ones() {
        let mut current = record_with(SourceKind::Agent, 1, 0.5);
        let mut expired = record_with(SourceKind::Agent, 100, 0.5);
        expired.valid_to = Some(Utc::now() - Duration::hours(1));
        current.valid_from = Some(Utc::now() - Duration::hours(2));

        assert_eq!(temporal_relevance(&current, Utc::now()), 1.0);
        assert!(temporal_relevance(&expired, Utc::now()) < 1.0);
    }

    #[test]
    fn weights_sum_near_one_by_construction() {
        let w = ScoreWeights::default();
        let total = w.similarity + w.recency + w.importance + w.confidence + w.authority;
        assert!((total - 1.0).abs() < 1e-4, "weights must sum to ~1.0");
    }
}
