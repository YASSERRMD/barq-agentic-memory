//! Engine-level benchmarks (release builds only; see scripts/bench.sh).
//!
//! Each benchmark runs against a fresh in-memory engine with the
//! hashing embedder so numbers reflect engine overhead, not network or
//! model latency.

use criterion::{Criterion, criterion_group, criterion_main};
use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};
use memory_domain::{MemoryScope, MemoryType};
use std::hint::black_box;
use std::time::Instant;

fn engine() -> MemoryEngine {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
            ..EngineConfig::default()
        })
        .await
        .expect("engine")
    })
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn bench_write_latency(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    let mut n = 0u64;

    c.bench_function("write/remember_latency", |b| {
        b.iter(|| {
            n += 1;
            runtime.block_on(engine.remember(RememberRequest::new(
                MemoryType::Semantic,
                format!("fact number {n} about project atlas"),
            )))
        })
    });
}

fn bench_exact_read(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    let saved = runtime
        .block_on(engine.remember(RememberRequest::new(
            MemoryType::Semantic,
            "lookup target fact",
        )))
        .expect("seed");
    let scope = MemoryScope::default();

    c.bench_function("read/exact_get_latency", |b| {
        b.iter(|| runtime.block_on(engine.recall_exact(saved.id, &scope)))
    });
}

fn bench_keyword_search(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    for i in 0..100 {
        let _ = runtime.block_on(engine.remember(RememberRequest::new(
            MemoryType::Semantic,
            format!("fact {i}: project atlas uses postgresql"),
        )));
    }

    let mut group = c.benchmark_group("read/keyword_search");
    group.bench_function("corpus_100", |b| {
        b.iter(|| {
            runtime.block_on(
                engine.search(
                    memory_domain::MemoryQuery::default()
                        .with_text("atlas postgresql")
                        .with_limit(10),
                ),
            )
        })
    });
    group.finish();
}

fn bench_vector_recall(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    for i in 0..50 {
        let _ = runtime.block_on(engine.remember(RememberRequest::new(
            MemoryType::Semantic,
            format!("document {i} describing deployment runbooks and postgresql"),
        )));
    }

    c.bench_function("recall/semantic_recall_50_docs", |b| {
        b.iter(|| {
            runtime.block_on(engine.recall_semantic(
                "deployment runbook postgres",
                5,
                &MemoryScope::default(),
            ))
        })
    });
}

fn bench_hybrid_recall(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    for i in 0..50 {
        let _ = runtime.block_on(
            engine.remember(
                RememberRequest::new(
                    MemoryType::Semantic,
                    format!("hybrid corpus fact {i} about vector databases"),
                )
                .with_subject(memory_domain::MemorySubject::new(format!("subject-{i}"))),
            ),
        );
    }

    c.bench_function("recall/hybrid_recall_50_docs", |b| {
        b.iter(|| {
            runtime.block_on(engine.recall(
                &memory_retrieval::RecallRequest::new("vector database facts").with_budget(5),
            ))
        })
    });
}

fn bench_update_supersession(c: &mut Criterion) {
    let engine = engine();
    let runtime = rt();
    c.bench_function("write/update_supersession", |b| {
        b.iter_batched(
            || {
                let saved = runtime
                    .block_on(engine.remember(RememberRequest::new(
                        MemoryType::Semantic,
                        "seed fact for supersession benchmark",
                    )))
                    .expect("seed");
                saved.id
            },
            |old_id| {
                runtime.block_on(engine.update(UpdateRequest::content(
                    old_id,
                    MemoryScope::default(),
                    "revised content for supersession",
                )))
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_startup(c: &mut Criterion) {
    c.bench_function("startup/embedded_engine_assembly", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let t0 = Instant::now();
                let runtime = rt();
                let _ = black_box(runtime.block_on(MemoryEngine::from_config(EngineConfig {
                    vector: Some(VectorStoreConfig::InMemory),
                    embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
                    ..EngineConfig::default()
                })));
                total += t0.elapsed();
            }
            total
        })
    });
}

criterion_group!(
    benches,
    bench_write_latency,
    bench_exact_read,
    bench_keyword_search,
    bench_vector_recall,
    bench_hybrid_recall,
    bench_update_supersession,
    bench_startup,
);
criterion_main!(benches);
