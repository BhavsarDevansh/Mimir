//! Benchmarks for per-test database setup costs in mimir-core.
//!
//! Every test that constructs a `ContextManager` or `JobQueue` pays the
//! schema-initialisation cost below: fresh SQLite file, `PRAGMA` setup, DDL
//! (and for the context DB the FTS5 triggers), and index creation. These
//! benchmarks capture the baseline so the test-suite setup cost can be
//! tracked and reduced.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_core::context::ContextManager;
use mimir_core::job_queue::JobQueue;

fn bench_context_schema_init(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("context_schema_init", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                rt.block_on(async {
                    let mgr = ContextManager::new(dir.path().join("ctx.db"))
                        .await
                        .unwrap();
                    std::hint::black_box(&mgr);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_job_queue_schema_init(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("job_queue_schema_init", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                rt.block_on(async {
                    let jq = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();
                    std::hint::black_box(&jq);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    db_init_benches,
    bench_context_schema_init,
    bench_job_queue_schema_init
);
criterion_main!(db_init_benches);
