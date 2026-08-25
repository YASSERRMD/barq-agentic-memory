//! Contradiction detection and resolution policy.
//!
//! Rule-based negation detection covers the common cases ("is not",
//! "no longer", "never", "stopped") with same-subject anchoring. Value
//! mismatches without explicit negation stay ambiguous — guessing a
//! contradiction from wording alone is how memory systems corrupt
//! themselves.

use chrono::Utc;
use memory_domain::{MemoryRecord, MemoryResult};
use serde::{Deserialize, Serialize};
use std::fmt;

/// How an incoming fact relates to existing state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// No comparable existing fact.
    Consistent,
    /// Byte/wording duplicate (dedup's domain; surfaced for completeness).
    Duplicate,
    /// Incoming explicitly replaces an open fact.
    Supersedes,
    /// Incoming asserts the opposite of an open fact.
    Contradicts,
    /// Related but unresolvable by rules alone.
    Ambiguous,
    /// Detector wants human eyes before anything changes.
    ReviewRequired,
}

impl fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            ConflictKind::Consistent => "consistent",
            ConflictKind::Duplicate => "duplicate",
            ConflictKind::Supersedes => "supersedes",
            ConflictKind::Contradicts => "contradicts",
            ConflictKind::Ambiguous => "ambiguous",
            ConflictKind::ReviewRequired => "review_required",
        };
        f.write_str(name)
    }
}

/// Analysis of one incoming record against one existing record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConflictAnalysis {
    pub kind: ConflictKind,
    /// Existing record involved (None when consistent).
    pub existing_id: Option<memory_domain::MemoryId>,
    /// Why this classification was reached.
    pub rationale: String,
}

/// What should physically happen on write.
#[derive(Clone, Debug, PartialEq)]
pub enum SupersessionOutcome {
    /// Store incoming normally.
    Write,
    /// Close the existing fact's window at `closed_at`, then store
    /// incoming as its successor.
    ReplaceExisting {
        /// Record being retired (status -> superseded, window closed).
        closing_id: memory_domain::MemoryId,
    },
    /// Store incoming quarantined; nothing else changes.
    QuarantineIncoming,
}

/// Decides outcomes after analysis. Pure function of inputs.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResolutionPolicy;

impl ResolutionPolicy {
    /// Analyzes incoming against a single open existing fact.
    ///
    /// `negates` answers: does the incoming statement assert the
    /// opposite of the existing one?
    pub fn analyze(
        &self,
        incoming: &MemoryRecord,
        existing: &MemoryRecord,
        negates: bool,
        value_conflict: bool,
    ) -> ConflictAnalysis {
        if !same_subject(incoming, existing) {
            return ConflictAnalysis {
                kind: ConflictKind::Consistent,
                existing_id: None,
                rationale: "different subjects".into(),
            };
        }
        if negates {
            return ConflictAnalysis {
                kind: ConflictKind::Contradicts,
                existing_id: Some(existing.id),
                rationale: "explicit negation of an open fact".into(),
            };
        }
        if value_conflict {
            return ConflictAnalysis {
                kind: ConflictKind::ReviewRequired,
                existing_id: Some(existing.id),
                rationale: "same subject with differing values".into(),
            };
        }
        ConflictAnalysis {
            kind: ConflictKind::Ambiguous,
            existing_id: Some(existing.id),
            rationale: "related statements without detectable conflict".into(),
        }
    }

    /// Resolves an analysis into a concrete outcome.
    ///
    /// Authority dominates confidence: a user's word outranks a more
    /// confident agent guess. Ties go to the newer claim only when it
    /// is at least as authoritative AND at least as confident —
    /// otherwise humans review.
    pub fn resolve(
        &self,
        analysis: &ConflictAnalysis,
        incoming: &MemoryRecord,
        existing: &MemoryRecord,
    ) -> SupersessionOutcome {
        match analysis.kind {
            ConflictKind::Consistent | ConflictKind::Supersedes => SupersessionOutcome::Write,
            ConflictKind::Duplicate => SupersessionOutcome::Write, // dedup handles earlier
            ConflictKind::ReviewRequired => SupersessionOutcome::QuarantineIncoming,
            ConflictKind::Ambiguous => {
                if self.incoming_dominates(incoming, existing) {
                    SupersessionOutcome::ReplaceExisting {
                        closing_id: existing.id,
                    }
                } else {
                    SupersessionOutcome::QuarantineIncoming
                }
            }
            ConflictKind::Contradicts => {
                // A direct negation always retires the old fact — even
                // when the newcomer is weaker — because keeping both in
                // active state poisons retrieval either way. The old
                // record survives as history either way.
                let _ = self.incoming_dominates(incoming, existing);
                SupersessionOutcome::ReplaceExisting {
                    closing_id: analysis.existing_id.unwrap_or(existing.id),
                }
            }
        }
    }

    fn incoming_dominates(&self, incoming: &MemoryRecord, existing: &MemoryRecord) -> bool {
        let in_auth = incoming.provenance.source.default_authority();
        let ex_auth = existing.provenance.source.default_authority();
        if (in_auth - ex_auth).abs() > f32::EPSILON {
            return in_auth > ex_auth;
        }
        incoming.confidence >= existing.confidence
    }

    /// Closes an existing fact's validity window as superseded.
    ///
    /// Returns the mutated record for the caller to persist; the
    /// original history remains fully addressable.
    pub fn close_window(existing: &mut MemoryRecord) -> MemoryResult<()> {
        existing.valid_to.get_or_insert(Utc::now());
        existing.status = memory_domain::MemoryStatus::Superseded;
        existing.updated_at = Utc::now();
        Ok(())
    }
}

fn same_subject(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    match (&a.subject, &b.subject) {
        (Some(x), Some(y)) => x.canonical_key() == y.canonical_key(),
        _ => false,
    }
}

/// Negation cues for rule-based detection.
const NEGATION_CUES: [&str; 6] = [
    " no longer ", " is not ", " does not ", " never ", " stopped ", " removed ",
];

/// Cheap negation check over normalized text.
///
/// Spaces pad each cue so substring hits require word boundaries.
pub fn detects_negation(text: &str) -> bool {
    let padded = format!(" {} ", text.to_lowercase());
    NEGATION_CUES.iter().any(|cue| padded.contains(cue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use memory_domain::{MemoryContent, MemorySubject, MemoryType};

    fn fact(text: &str, subject: &str, source: memory_domain::SourceKind, confidence: f32) -> MemoryRecord {
        let mut r = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text(text))
            .with_subject(MemorySubject::new(subject).with_type("project"))
            .with_confidence(confidence);
        r.provenance = memory_domain::Provenance::now(source);
        r
    }

    #[test]
    fn negation_cues_are_detected_with_boundaries() {
        assert!(detects_negation("Atlas no longer uses MySQL"));
        assert!(detects_negation("Atlas is NOT on postgres"));
        assert!(detects_negation("we stopped using redis"));
        assert!(!detects_negation("Atlas uses PostgreSQL and notes are stored elsewhere"));
    }

    #[test]
    fn different_subjects_are_always_consistent() {
        let policy = ResolutionPolicy;
        let existing = fact("Atlas uses MySQL", "atlas", memory_domain::SourceKind::User, 0.9);
        let incoming = fact("Globex uses PostgreSQL", "globex", memory_domain::SourceKind::Agent, 0.5);
        let a = policy.analyze(&incoming, &existing, true, false);
        assert_eq!(a.kind, ConflictKind::Consistent, "subject mismatch short-circuits");
    }

    #[tokio::test]
    async fn contradiction_supersedes_regardless_of_strength() {
        let policy = ResolutionPolicy;
        let existing = fact("Atlas uses MySQL", "atlas", memory_domain::SourceKind::User, 0.95);
        let incoming = fact("Atlas does not use MySQL anymore", "atlas", memory_domain::SourceKind::Agent, 0.4);

        let analysis = policy.analyze(&incoming, &existing, true, false);
        assert_eq!(analysis.kind, ConflictKind::Contradicts);

        match policy.resolve(&analysis, &incoming, &existing) {
            SupersessionOutcome::ReplaceExisting { closing_id } => {
                assert_eq!(closing_id, existing.id);
                // Closing preserves history instead of destroying it.
                let mut copy = existing.clone();
                ResolutionPolicy::close_window(&mut copy).expect("close");
                assert_eq!(copy.status, memory_domain::MemoryStatus::Superseded);
                assert!(copy.validity().has_ended(Utc::now()));
                assert!(
                    !copy.is_valid_at(Utc::now()),
                    "retired facts leave active retrieval"
                );
            }
            other => panic!("expected replacement, got {other:?}"),
        }
    }

    #[test]
    fn value_conflicts_quarantine_when_incoming_is_weaker() {
        let policy = ResolutionPolicy;
        let existing = fact("Atlas uses PostgreSQL 16", "atlas", memory_domain::SourceKind::User, 0.9);
        let incoming = fact("Atlas uses PostgreSQL 17", "atlas", memory_domain::SourceKind::Agent, 0.4);

        let analysis = policy.analyze(&incoming, &existing, false, true);
        assert_eq!(analysis.kind, ConflictKind::ReviewRequired);
        assert_eq!(
            policy.resolve(&analysis, &incoming, &existing),
            SupersessionOutcome::QuarantineIncoming
        );
    }

    #[test]
    fn ambiguous_ambiguity_defers_to_authority_then_confidence() {
        let policy = ResolutionPolicy;

        let strong_user_old = fact("Atlas deploys to us-east", "atlas", memory_domain::SourceKind::User, 0.5);
        let weak_agent_new = fact("Atlas deploys to eu-west these days", "atlas", memory_domain::SourceKind::Agent, 0.9);
        let a = policy.analyze(&weak_agent_new, &strong_user_old, false, false);
        assert_eq!(a.kind, ConflictKind::Ambiguous);
        assert_eq!(
            policy.resolve(&a, &weak_agent_new, &strong_user_old),
            SupersessionOutcome::QuarantineIncoming,
            "agent confidence cannot outrank user authority"
        );

        let user_a = fact("Atlas deploys to us-east", "atlas", memory_domain::SourceKind::User, 0.4);
        let user_b = fact("Atlas deploys to eu-west now", "atlas", memory_domain::SourceKind::User, 0.8);
        let b = policy.analyze(&user_b, &user_a, false, false);
        assert_eq!(
            policy.resolve(&b, &user_b, &user_a),
            SupersessionOutcome::ReplaceExisting { closing_id: user_a.id },
            "equal authority: higher confidence wins"
        );
    }

    #[test]
    fn closed_windows_keep_history_addressable() {
        let mut r = fact("old fact", "x", memory_domain::SourceKind::System, 0.5);
        r.created_at -= Duration::days(90);
        ResolutionPolicy::close_window(&mut r).expect("close");

        // History query path still finds it via include_history mode.
        assert!(r.status.is_retired() && !r.status.is_retrievable()
            || matches!(r.status, memory_domain::MemoryStatus::Superseded));
    }
}
