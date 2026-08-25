//! Episode recording and retrieval on the engine facade.

use crate::engine::MemoryEngine;
use memory_domain::{MemoryId, MemoryResult, MemoryScope};

impl MemoryEngine {
    /// Records an agent experience.
    ///
    /// Requires `with_episodes`; engines without one simply do not
    /// track episodes, which keeps embedded footprints minimal.
    pub async fn record_episode(&self, episode: &memory_episodic::Episode) -> MemoryResult<()> {
        match &self.episodes {
            Some(store) => store.append(episode).await,
            None => Err(memory_domain::MemoryError::Unsupported(
                "no episode store attached (see with_episodes)".into(),
            )),
        }
    }

    /// Lists episodes matching a query, newest first.
    pub async fn recall_episodes(
        &self,
        query: &memory_episodic::EpisodeQuery,
    ) -> MemoryResult<Vec<memory_episodic::Episode>> {
        match &self.episodes {
            Some(store) => store.query(query).await,
            None => Ok(Vec::new()),
        }
    }

    /// Episodes citing a canonical memory as evidence.
    pub async fn episodes_citing(
        &self,
        memory_id: &MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<Vec<memory_episodic::Episode>> {
        self.recall_episodes(&memory_episodic::EpisodeQuery {
            scope: scope.clone(),
            citing_evidence: Some(*memory_id),
            ..Default::default()
        })
        .await
    }
}

#[cfg(test)]
mod episode_tests {
    use super::*;
    use memory_episodic::InMemoryEpisodeStore;

    #[tokio::test]
    async fn episodes_roundtrip_through_the_engine() {
        let engine = MemoryEngine::from_config(Default::default())
            .await
            .unwrap()
            .with_episodes(std::sync::Arc::new(InMemoryEpisodeStore::new()));

        let evidence = {
            let r =
                crate::RememberRequest::new(memory_domain::MemoryType::Episodic, "ran migration")
                    .into_record(Default::default());
            r.id
        };

        let episode = memory_episodic::Episode::builder("migrate", "completed cleanly")
            .citing([evidence])
            .build();
        engine.record_episode(&episode).await.expect("record");

        let cited = engine
            .episodes_citing(&evidence, &Default::default())
            .await
            .expect("citing");
        assert_eq!(cited.len(), 1);
        assert!(cited[0].cites(&evidence));

        // Failure filter works through the same path.
        let failures = engine
            .recall_episodes(&memory_episodic::EpisodeQuery {
                success: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn recall_episodes_without_store_is_empty_not_error() {
        let engine = MemoryEngine::from_config(Default::default()).await.unwrap();
        let hits = engine
            .recall_episodes(&memory_episodic::EpisodeQuery::default())
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
}
