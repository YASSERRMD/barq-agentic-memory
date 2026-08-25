//! Shared predicate applying a [`MemoryQuery`] to canonical records.
//!
//! Both embedded stores filter identically so behavior cannot drift
//! between the volatile and persistent backends.

use memory_domain::{MemoryQuery, MemoryRecord, MemoryScope};

/// True when `record` satisfies every constraint in `query`.
pub fn matches_query(record: &MemoryRecord, query: &MemoryQuery) -> bool {
    if !query.scope.contains(&record.scope) {
        return false;
    }
    if !query.accepts_type(record.memory_type) {
        return false;
    }
    if !query.accepts_status(record.status) {
        return false;
    }
    if !subject_matches(record, query) {
        return false;
    }
    if !text_matches(record, query) {
        return false;
    }
    record.validity().contains(query.effective_valid_at())
}

fn subject_matches(record: &MemoryRecord, query: &MemoryQuery) -> bool {
    match (&query.subject, &record.subject) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(want), Some(have)) => want.canonical_key() == have.canonical_key(),
    }
}

fn text_matches(record: &MemoryRecord, query: &MemoryQuery) -> bool {
    match query.text.as_deref() {
        None => true,
        Some(needle) => {
            let needle = needle.trim().to_lowercase();
            record.content.text.to_lowercase().contains(&needle)
        }
    }
}

/// Convenience: scope wildcard used by tests and examples.
pub fn global_scope() -> MemoryScope {
    MemoryScope::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use memory_domain::{MemoryContent, MemorySubject, MemoryType, RetentionPolicy};

    fn record() -> MemoryRecord {
        let mut r = MemoryRecord::new(
            MemoryType::Semantic,
            MemoryContent::from_text("Customer prefers email contact"),
        );
        r.subject = Some(MemorySubject::new("cust-7"));
        r
    }

    #[test]
    fn default_query_accepts_active_records() {
        assert!(matches_query(&record(), &MemoryQuery::default()));
    }

    #[test]
    fn type_filter_is_exact() {
        let q = MemoryQuery::default().of_type(MemoryType::Episodic);
        assert!(!matches_query(&record(), &q));
        let q = MemoryQuery::default().of_type(MemoryType::Semantic);
        assert!(matches_query(&record(), &q));
    }

    #[test]
    fn text_match_ignores_case() {
        let q = MemoryQuery::default().with_text("PREFERS EMAIL");
        assert!(matches_query(&record(), &q));
        let q = MemoryQuery::default().with_text("slack");
        assert!(!matches_query(&record(), &q));
    }

    #[test]
    fn subject_key_must_equal() {
        let q = MemoryQuery::default().with_subject(MemorySubject::new("cust-7"));
        assert!(matches_query(&record(), &q));
        let q = MemoryQuery::default().with_subject(MemorySubject::new("cust-8"));
        assert!(!matches_query(&record(), &q));
    }

    #[test]
    fn expired_records_fail_validity_snapshot() {
        let mut r = record();
        r.valid_to = Some(Utc::now() - Duration::hours(1));
        assert!(!matches_query(&r, &MemoryQuery::default()));
    }

    #[test]
    fn retention_expiry_is_not_a_query_concern() {
        // Lifecycle sweeps own expiry; queries still surface lapsed
        // records until a sweep retires them.
        let mut r = record();
        r.retention = RetentionPolicy::expiring_at(Utc::now() - Duration::hours(1));
        assert!(matches_query(&r, &MemoryQuery::default()));
    }
}
