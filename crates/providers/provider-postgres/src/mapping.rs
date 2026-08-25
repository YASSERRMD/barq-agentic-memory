//! Enum/string mapping helpers.
//!
//! Statuses and types are stored as their serialized snake_case names;
//! these helpers are the single translation point so column values stay
//! stable even if serde attributes change.

use memory_domain::{MemoryError, MemoryResult, MemoryStatus, MemoryType};

/// Column value for a [`MemoryType`].
pub fn type_to_db(t: MemoryType) -> &'static str {
    t.as_str()
}

/// Parses a [`MemoryType`] column value.
pub fn type_from_db(raw: &str) -> MemoryResult<MemoryType> {
    match raw {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "prospective" => Ok(MemoryType::Prospective),
        other => Err(MemoryError::storage(
            "postgres",
            format!("unknown memory_type '{other}'"),
        )),
    }
}

/// Column value for a [`MemoryStatus`].
pub fn status_to_db(s: MemoryStatus) -> &'static str {
    match s {
        MemoryStatus::Active => "active",
        MemoryStatus::Superseded => "superseded",
        MemoryStatus::Quarantined => "quarantined",
        MemoryStatus::Archived => "archived",
        MemoryStatus::Deleted => "deleted",
    }
}

/// Parses a [`MemoryStatus`] column value.
pub fn status_from_db(raw: &str) -> MemoryResult<MemoryStatus> {
    match raw {
        "active" => Ok(MemoryStatus::Active),
        "superseded" => Ok(MemoryStatus::Superseded),
        "quarantined" => Ok(MemoryStatus::Quarantined),
        "archived" => Ok(MemoryStatus::Archived),
        "deleted" => Ok(MemoryStatus::Deleted),
        other => Err(MemoryError::storage(
            "postgres",
            format!("unknown status '{other}'"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_roundtrips_through_column_values() {
        for t in MemoryType::ALL {
            assert_eq!(type_from_db(type_to_db(t)).unwrap(), t);
        }
    }

    #[test]
    fn every_status_roundtrips_through_column_values() {
        for s in MemoryStatus::ALL_STATUSES {
            assert_eq!(status_from_db(status_to_db(s)).unwrap(), s);
        }
    }

    #[test]
    fn unknown_values_are_storage_errors_not_panics() {
        assert!(type_from_db("quantum").is_err());
        assert!(status_from_db("vibes").is_err());
    }
}
