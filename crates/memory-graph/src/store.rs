//! Graph storage contract and the embedded implementation.

use crate::graph::{Entity, EntityKey, Relation};
use async_trait::async_trait;
use memory_domain::{MemoryId, MemoryResult, MemoryScope};
use std::collections::HashMap;
use std::sync::RwLock;

/// Entity-relation graph over canonical memories.
#[async_trait]
pub trait GraphProvider: Send + Sync {
    /// Provider name for logs.
    fn name(&self) -> &str;

    /// Inserts or updates an entity node.
    async fn upsert_entity(&self, entity: &Entity) -> MemoryResult<()>;

    /// Adds a relation edge; must reference existing evidence memory.
    async fn add_relation(&self, relation: &Relation) -> MemoryResult<()>;

    /// Outgoing relations from an entity.
    async fn relations_from(&self, key: &EntityKey) -> MemoryResult<Vec<Relation>>;

    /// Incoming relations to an entity.
    async fn relations_to(&self, key: &EntityKey) -> MemoryResult<Vec<Relation>>;

    /// Removes all edges justified by one canonical memory (used when
    /// that memory is forgotten or purged).
    async fn remove_evidence(&self, evidence: &MemoryId) -> MemoryResult<()>;
}

/// Thread-safe adjacency-list graph.
#[derive(Default)]
pub struct InMemoryGraphStore {
    entities: RwLock<HashMap<EntityKey, Entity>>,
    edges: RwLock<Vec<Relation>>,
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.read().expect("poisoned").len()
    }
}

#[async_trait]
impl GraphProvider for InMemoryGraphStore {
    fn name(&self) -> &str {
        "in-memory"
    }

    async fn upsert_entity(&self, entity: &Entity) -> MemoryResult<()> {
        if entity.display_name.trim().is_empty() {
            return Err(memory_domain::MemoryError::validation(
                "display_name",
                "must not be empty",
            ));
        }
        self.entities
            .write()
            .expect("poisoned")
            .insert(entity.key.clone(), entity.clone());
        Ok(())
    }

    async fn add_relation(&self, relation: &Relation) -> MemoryResult<()> {
        let mut edges = self.edges.write().expect("poisoned");
        if edges.iter().any(|e| e == relation) {
            return Ok(()); // idempotent
        }
        // Nodes materialize implicitly so edges never dangle on missing
        // entities; display names arrive with explicit upserts.
        edges.push(relation.clone());
        Ok(())
    }

    async fn relations_from(&self, key: &EntityKey) -> MemoryResult<Vec<Relation>> {
        Ok(self
            .edges
            .read()
            .expect("poisoned")
            .iter()
            .filter(|e| e.from == *key)
            .cloned()
            .collect())
    }

    async fn relations_to(&self, key: &EntityKey) -> MemoryResult<Vec<Relation>> {
        Ok(self
            .edges
            .read()
            .expect("poisoned")
            .iter()
            .filter(|e| e.to == *key)
            .cloned()
            .collect())
    }

    async fn remove_evidence(&self, evidence: &MemoryId) -> MemoryResult<()> {
        self.edges
            .write()
            .expect("poisoned")
            .retain(|e| e.evidence != *evidence);
        Ok(())
    }
}

/// Convenience: two-hop neighborhood query composed from the trait.
pub async fn neighbors(
    graph: &dyn GraphProvider,
    key: &EntityKey,
) -> MemoryResult<Vec<(Relation, Direction)>> {
    let mut out = Vec::new();
    for r in graph.relations_from(key).await? {
        out.push((r, Direction::Outgoing));
    }
    for r in graph.relations_to(key).await? {
        out.push((r, Direction::Incoming));
    }
    Ok(out)
}

/// Edge direction relative to the queried entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

// Scope rides on canonical records; graphs are per-engine instances,
// so a scope parameter is unnecessary at this layer. Kept referenced
// for the future multi-tenant graph backend contract.
#[allow(unused)]
fn _scope_note(_s: &MemoryScope) {}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::MemorySubject;

    #[tokio::test]
    async fn relations_roundtrip_and_index_both_directions() {
        let g = InMemoryGraphStore::new();
        let atlas = EntityKey::from_subject(
            &MemorySubject::new("atlas").with_type("project"),
        );
        let postgres = EntityKey::from_subject(
            &MemorySubject::new("postgres").with_type("database"),
        );
        let evidence = MemoryId::generate();

        g.upsert_entity(&Entity {
            key: atlas.clone(),
            display_name: "Project Atlas".into(),
        })
        .await
        .expect("upsert");

        g.add_relation(&Relation::new(evidence, atlas.clone(), "uses", postgres.clone()).expect("rel"))
            .await
            .expect("add");
        g.add_relation(&Relation::new(evidence, atlas.clone(), "uses", postgres.clone()).expect("rel"))
            .await
            .expect("duplicate is idempotent");
        assert_eq!(g.edge_count(), 1);

        let from = g.relations_from(&atlas).await.expect("from");
        assert_eq!(from.len(), 1);
        assert_eq!(from[0].to, postgres);

        let to = g.relations_to(&postgres).await.expect("to");
        assert_eq!(to.len(), 1);

        g.remove_evidence(&evidence).await.expect("remove");
        assert_eq!(g.edge_count(), 0);
    }

    #[tokio::test]
    async fn neighbor_composition_reports_direction() {
        let g = InMemoryGraphStore::new();
        let a = EntityKey::new(None, "a");
        let b = EntityKey::new(None, "b");
        let c = EntityKey::new(None, "c");
        let e = MemoryId::generate();

        g.add_relation(&Relation::new(e, b.clone(), "uses", a.clone()).expect("rel"))
            .await
            .expect("add");
        g.add_relation(&Relation::new(MemoryId::generate(), a.clone(), "owns", c.clone()).expect("rel"))
            .await
            .expect("add");

        let hood = neighbors(&g, &a).await.expect("hood");
        assert_eq!(hood.len(), 2);
        assert!(hood.contains(&(Relation::new(e, b.clone(), "uses", a.clone()).unwrap(), Direction::Incoming)));
        assert!(hood
            .iter()
            .any(|(r, d)| r.from == a && *d == Direction::Outgoing));
    }
}
