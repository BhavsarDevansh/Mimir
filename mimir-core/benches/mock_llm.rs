//! Benchmarks for the `MockLlmClient` test double.
//!
//! The mock is the LLM backend for most of the test suite (unit tests,
//! server integration tests, connector tests, and E2E daemons), so its
//! per-call lock traffic and record-cloning cost is paid thousands of times
//! per suite run.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_core::llm::MockLlmClient;
use mimir_core::llm::backend::LlmBackend;
use mimir_core::llm::types::{Message, Usage};

fn bench_mock_chat_call(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("mock_llm_chat_call", |b| {
        b.iter_batched(
            || {
                let mut mock = MockLlmClient::builder();
                for i in 0..100 {
                    mock = mock.push_chat(format!("response {i}"), Usage::default());
                }
                mock.build()
            },
            |mock| {
                rt.block_on(async {
                    let (msg, usage) = mock
                        .chat_message(vec![Message::user("hello")], None)
                        .await
                        .expect("queued response");
                    std::hint::black_box((msg, usage));
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_mock_records_clone(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("mock_llm_records_clone_100", |b| {
        b.iter_batched(
            || {
                let mut mock = MockLlmClient::builder();
                for i in 0..100 {
                    mock = mock.push_chat(format!("response {i}"), Usage::default());
                }
                let mock = mock.build();
                for i in 0..100 {
                    rt.block_on(async {
                        mock.chat_message(vec![Message::user(format!("message {i}"))], None)
                            .await
                            .expect("queued response");
                    });
                }
                mock
            },
            |mock| {
                let calls = mock.chat_calls();
                std::hint::black_box(calls.len());
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    mock_llm_benches,
    bench_mock_chat_call,
    bench_mock_records_clone
);
criterion_main!(mock_llm_benches);
