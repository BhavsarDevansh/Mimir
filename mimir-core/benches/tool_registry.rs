use criterion::{Criterion, criterion_group, criterion_main};
use mimir_core::tools::ToolRegistry;

fn bench_register(c: &mut Criterion) {
    c.bench_function("tool_registry_register", |b| {
        b.iter_batched(
            ToolRegistry::new,
            |registry| {
                // Register the two built-in tools repeatedly to measure overhead.
                registry.register_builtins();
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_export_schema(c: &mut Criterion) {
    let registry = ToolRegistry::with_builtins();
    c.bench_function("tool_registry_export_schema", |b| {
        b.iter(|| {
            let schema = registry.export_openai_tools();
            std::hint::black_box(schema);
        })
    });
}

fn bench_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = ToolRegistry::with_builtins();
    c.bench_function("tool_registry_execute_echo", |b| {
        b.to_async(&rt).iter(|| async {
            let result = registry
                .execute("echo", serde_json::json!({"message": "hello"}))
                .await
                .unwrap();
            std::hint::black_box(result);
        })
    });
}

criterion_group!(
    tool_benches,
    bench_register,
    bench_export_schema,
    bench_execute
);
criterion_main!(tool_benches);
