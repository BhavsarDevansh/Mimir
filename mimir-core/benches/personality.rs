use criterion::{Criterion, criterion_group, criterion_main};
use mimir_core::personality::Personality;

fn bench_system_prompt_empty(c: &mut Criterion) {
    let personality = Personality::from_path(std::path::Path::new("/nonexistent"), "transparent");
    c.bench_function("personality_system_prompt_empty", |b| {
        b.iter(|| {
            let prompt = personality.system_prompt("");
            std::hint::black_box(prompt);
        })
    });
}

fn bench_system_prompt_large_memory(c: &mut Criterion) {
    let personality = Personality::from_path(std::path::Path::new("/nonexistent"), "transparent");
    let memory = "a".repeat(10_000);
    c.bench_function("personality_system_prompt_large_memory", |b| {
        b.iter(|| {
            let prompt = personality.system_prompt(&memory);
            std::hint::black_box(prompt);
        })
    });
}

criterion_group!(
    personality_benches,
    bench_system_prompt_empty,
    bench_system_prompt_large_memory
);
criterion_main!(personality_benches);
