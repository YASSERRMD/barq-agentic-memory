//! Stable identifiers for memory entities.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique identifier of a canonical memory record.
///
/// Backed by a UUIDv7 so identifier order approximates creation order,
/// which keeps local indexes and range scans cheap without a separate
/// sequence column.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(Uuid);

impl MemoryId {
    /// Generates a new time-ordered identifier.
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wraps an existing UUID.
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parses the canonical hyphenated string form.
    pub fn parse(input: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(input).map(Self)
    }

    /// Inner UUID reference.
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Canonical hyphenated string form.
    pub fn hyphenated(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.hyphenated().to_string())
    }
}

impl fmt::Debug for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemoryId({})", self.0.hyphenated())
    }
}

impl From<Uuid> for MemoryId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<MemoryId> for Uuid {
    fn from(value: MemoryId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique_and_parse_back() {
        let a = MemoryId::generate();
        let b = MemoryId::generate();
        assert_ne!(a, b);

        let reparsed = MemoryId::parse(&a.hyphenated()).expect("roundtrip");
        assert_eq!(a, reparsed);
    }

    #[test]
    fn serializes_as_plain_string() {
        let id = MemoryId::generate();
        let json = serde_json::to_string(&id).expect("serialize");
        assert!(!json.contains('{'), "expected transparent string, got {json}");

        let back: MemoryId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn ordering_matches_uuid_ordering() {
        let earlier = MemoryId::from_uuid(Uuid::nil());
        let later = MemoryId::generate();
        assert!(earlier < later);
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(MemoryId::parse("not-a-uuid").is_err());
    }
}
