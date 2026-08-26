//! Benchmark for the full daemon `AppState` build.
//!
//! `AppState::from_config_with_llm` is the startup path every daemon boot
//! pays — and every connector/CLI E2E test pays it once per `TestDaemon`.
//! It composes the context manager, tool registry, knowledge graph (58
//! migrations), job queue, hooks engine, scheduler, and connector
//! framework. The benchmark builds and shuts down a fresh state per sample.
//!
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_core::config::{Config, ReloadableConfig};
use mimir_core::llm::{LlmBackend, MockLlmClient};
use mimir_server::state::AppState;

fn build_config(dir: &tempfile::TempDir) -> (Arc<ReloadableConfig>, Arc<dyn LlmBackend>) {
    let mut config = Config::default();
    config.llm.endpoint = "http://127.0.0.1:1".to_string();
    config.llm.api_key = "test".to_string();
    config.llm.model = "gpt-4o".to_string();
    config.context.db_path = Some(dir.path().join("context.db"));
    config.knowledge.db_path = Some(dir.path().join("knowledge.db"));
    config.scheduler.db_path = Some(dir.path().join("jobs.db"));
    // No geocoder and no identity entity: keeps the build to the
    // deterministic core paths, matching the E2E test config.
    config.geocoder.enabled = false;
    config.identity.name = String::new();
    let config = Arc::new(ReloadableConfig::new(
        config,
        dir.path().join("dummy_config.toml"),
    ));
    let mock: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
    (config, mock)
}

fn bench_app_state_build_only(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("app_state_from_config_with_llm_build", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (config, mock) = build_config(&dir);
                (dir, config, mock)
            },
            |(dir, config, mock)| {
                rt.block_on(async {
                    let (state, _sched_rx, _hook_rx) =
                        AppState::from_config_with_llm(config, mock, Arc::from("bench-token"))
                            .await
                            .unwrap();
                    std::hint::black_box(&state);
                    // The state is dropped without `shutdown()`: the daemon
                    // test harness does the same when a test ends without
                    // stopping the server. The 5s hook-exit timeout is the
                    // teardown cost captured by the *_shutdown variant.
                });
                drop(dir);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_app_state_build_and_shutdown(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("app_state_from_config_with_llm_build_shutdown", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (config, mock) = build_config(&dir);
                (dir, config, mock)
            },
            |(dir, config, mock)| {
                rt.block_on(async {
                    let (state, _sched_rx, _hook_rx) =
                        AppState::from_config_with_llm(config, mock, Arc::from("bench-token"))
                            .await
                            .unwrap();
                    state.shutdown().await;
                    std::hint::black_box(&state);
                });
                drop(dir);
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group! {
    name = state_benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_app_state_build_only, bench_app_state_build_and_shutdown
}
criterion_main!(state_benches);
