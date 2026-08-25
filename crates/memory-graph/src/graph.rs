//! Entities and relations.

use memory_domain::MemoryId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable key for an entity: `type:id`, mirroring MemorySubject's
/// canonical keys so subjects resolve to entities without a mapping
/// table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityKey(pub String);

impl EntityKey {
    /// Builds a key from parts.
    ///
    /// The type lowercases; the id is trimmed but case-preserved to
    /// stay byte-identical with [`memory_domain::MemorySubject::
    /// canonical_key`], so subjects resolve to entities without a
    /// mapping table.
    pub fn new(entity_type: Option<&str>, id: &str) -> Self {
        match entity_type {
            Some(t) => Self(format!("{}:{}", t.trim().to_lowercase(), id.trim())),
            None => Self(id.trim().to_string()),
        }
    }

    /// Key from a canonical subject.
    pub fn from_subject(subject: &memory_domain::MemorySubject) -> Self {
        Self::new(subject.entity_type.as_deref(), &subject.entity_id)
    }
}

impl fmt::Display for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A node in the entity graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub display_name: String,
}

/// A directed, typed edge between entities.
///
/// `evidence` points at the canonical memory that justifies the edge;
/// edges without evidence are unverifiable and rejected at write time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Canonical memory this edge is backed by.
    pub evidence: MemoryId,
    pub from: EntityKey,
    pub relation_type: String,
    pub to: EntityKey,
}

impl Relation {
    /// Creates a validated relation ("Atlas" -USES-> "PostgreSQL").
    pub fn new(
        evidence: MemoryId,
        from: EntityKey,
        relation_type: impl Into<String>,
        to: EntityKey,
    ) -> Result<Self, memory_domain::MemoryError> {
        let relation_type = relation_type.into();
        if from.0.is_empty() || to.0.is_empty() || relation_type.trim().is_empty() {
            return Err(memory_domain::MemoryError::validation(
                "relation",
                "entities and relation type must not be empty",
            ));
        }
        if from == to {
            return Err(memory_domain::MemoryError::validation(
                "relation",
                "self-relations carry no information",
            ));
        }
        Ok(Self {
            evidence,
            from,
            relation_type: relation_type.to_uppercase(),
            to,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_keys_normalize_like_subjects() {
        let subject = memory_domain::MemorySubject::new("Atlas").with_type("Project");
        assert_eq!(EntityKey::from_subject(&subject).0, "project:Atlas",
            "must equal MemorySubject::canonical_key exactly");
        assert_eq!(
            EntityKey::from_subject(&subject).0,
            subject.canonical_key()
        );
        assert_eq!(EntityKey::new(None, " postgres ").0, "postgres");
    }

    #[test]
    fn relations_uppercase_types_and_reject_degenerate_cases() {
        let e = MemoryId::generate();
        let r = Relation::new(e, EntityKey::new(None, "a"), "uses", EntityKey::new(None, "b"))
            .expect("valid");
        assert_eq!(r.relation_type, "USES");

        assert!(Relation::new(e, EntityKey::new(None, "x"), "", EntityKey::new(None, "b")).is_err());
        assert!(Relation::new(e, EntityKey::new(None, "x"), "uses", EntityKey::new(None, "x")).is_err());
    }
}
