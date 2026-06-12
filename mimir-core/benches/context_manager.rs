use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_core::context::ContextManager;

fn bench_create_session(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("context_create_session", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("ctx.db");
                let mgr = rt.block_on(ContextManager::new(&db)).unwrap();
                (mgr, dir)
            },
            |(mgr, _dir)| {
                rt.block_on(async {
                    let sid = mgr.create_session("system prompt").await.unwrap();
                    std::hint::black_box(sid);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_add_messages(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("context_add_messages", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("ctx.db");
                let mgr = rt.block_on(ContextManager::new(&db)).unwrap();
                let sid = rt.block_on(mgr.create_session("sys")).unwrap();
                (mgr, sid, dir)
            },
            |(mgr, sid, _dir)| {
                rt.block_on(async {
                    std::hint::black_box(mgr.add_user_message(sid, "hello world").await).unwrap();
                    std::hint::black_box(mgr.add_assistant_message(sid, "hi there").await).unwrap();
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_export_messages(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("context_export_messages", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("ctx.db");
                let mgr = rt.block_on(ContextManager::new(&db)).unwrap();
                let sid = rt.block_on(mgr.create_session("sys")).unwrap();
                for i in 0..20 {
                    rt.block_on(mgr.add_user_message(sid, &format!("msg {}", i)))
                        .unwrap();
                    rt.block_on(mgr.add_assistant_message(sid, &format!("resp {}", i)))
                        .unwrap();
                }
                (mgr, sid, dir)
            },
            |(mgr, sid, _dir)| {
                let msgs = rt.block_on(mgr.export_messages(sid)).unwrap();
                std::hint::black_box(msgs);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_trim_to_budget(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("context_trim_to_budget", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("ctx.db");
                let mgr = rt.block_on(ContextManager::new(&db)).unwrap();
                let sid = rt.block_on(mgr.create_session("sys")).unwrap();
                for i in 0..50 {
                    rt.block_on(mgr.add_user_message(sid, &format!("user message number {}", i)))
                        .unwrap();
                    rt.block_on(
                        mgr.add_assistant_message(sid, &format!("assistant response number {}", i)),
                    )
                    .unwrap();
                }
                (mgr, sid, dir)
            },
            |(mgr, sid, _dir)| {
                rt.block_on(async {
                    std::hint::black_box(mgr.trim_to_budget(sid, Some(1000), 100).await).unwrap();
                });
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    context_benches,
    bench_create_session,
    bench_add_messages,
    bench_export_messages,
    bench_trim_to_budget
);
criterion_main!(context_benches);
