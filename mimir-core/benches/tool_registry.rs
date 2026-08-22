use criterion::{Criterion, criterion_group, criterion_main};
use mimir_core::llm::backend::{LlmBackend, LlmStream};
use mimir_core::llm::types::{LlmError, Message, Usage};
use mimir_core::tools::{ToolContext, ToolRegistry};
use std::sync::Arc;

/// Minimal LLM backend for the registry bench, which never calls the LLM.
#[derive(Debug)]
struct DummyLlm;

#[async_trait::async_trait]
impl LlmBackend for DummyLlm {
    async fn chat_message(
        &self,
        _messages: Vec<Message>,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        unimplemented!("registry bench never calls the LLM")
    }

    async fn chat_stream_with_usage(
        &self,
        _messages: Vec<Message>,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmStream, LlmError> {
        unimplemented!("registry bench never calls the LLM")
    }

    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
        unimplemented!("registry bench never calls the LLM")
    }
}

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
    let ctx = ToolContext {
        llm: Arc::new(DummyLlm),
        allow_write_tools: true,
    };
    c.bench_function("tool_registry_execute_echo", |b| {
        b.to_async(&rt).iter(|| async {
            let result = registry
                .execute("echo", serde_json::json!({"message": "hello"}), &ctx)
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
