use criterion::{Criterion, criterion_group, criterion_main};
use mimir_core::memory::manager::MemoryManager;

fn bench_add_small(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("memory_add_small", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("memory.md");
                std::fs::write(&path, "Initial content\n").unwrap();
                let mgr = rt.block_on(MemoryManager::new(&path, 10_000)).unwrap();
                (mgr, dir)
            },
            |(mut mgr, _dir)| {
                rt.block_on(async {
                    mgr.add("small entry").await.unwrap();
                });
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_add_large(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let large_text = "a".repeat(5000);
    c.bench_function("memory_add_large", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("memory.md");
                std::fs::write(&path, "").unwrap();
                let mgr = rt.block_on(MemoryManager::new(&path, 10_000)).unwrap();
                (mgr, dir)
            },
            |(mut mgr, _dir)| {
                let text = large_text.clone();
                rt.block_on(async move {
                    mgr.add(&text).await.unwrap();
                });
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_replace(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("memory_replace", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("memory.md");
                std::fs::write(&path, "Old text here\n").unwrap();
                let mgr = rt.block_on(MemoryManager::new(&path, 10_000)).unwrap();
                (mgr, dir)
            },
            |(mut mgr, _dir)| {
                rt.block_on(async {
                    mgr.replace("Old text here", "New text here").await.unwrap();
                });
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_usage_calculation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("memory_usage_calculation", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("memory.md");
                let content = "line\n".repeat(1000);
                std::fs::write(&path, &content).unwrap();
                let mgr = rt.block_on(MemoryManager::new(&path, 20_000)).unwrap();
                (mgr, dir)
            },
            |(mgr, _dir)| {
                let _ = std::hint::black_box(mgr.usage_pct());
                let _ = std::hint::black_box(mgr.current_chars());
                let _ = std::hint::black_box(mgr.remaining_chars());
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    memory_benches,
    bench_add_small,
    bench_add_large,
    bench_replace,
    bench_usage_calculation
);
criterion_main!(memory_benches);
