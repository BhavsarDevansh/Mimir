//! Benchmarks for mimir-api-types wire (de)serialisation and truncation.
//!
//! Exercises the full serde roundtrip pathway for representative request and
//! response payloads, plus the ToolCallInfo result-truncation helper.

use criterion::{Criterion, criterion_group, criterion_main};
use mimir_api_types::{
    AuditQueryRequest, BrowseRequest, ChatRequest, ChatResponse, FactDetailResponse,
    FactQueryParams, FactRow, ForgetRequest, PendingFactRow, PendingListResponse, StatusResponse,
    ToolCallInfo, Usage,
};
use std::hint::black_box;

fn sample_fact_row() -> FactRow {
    FactRow {
        id: 7,
        subject: "Alice".to_string(),
        predicate: "lives_in".to_string(),
        object: Some("London".to_string()),
        confidence: 0.9,
        status: "active".to_string(),
        valid_from: Some("2020-01-01T00:00:00Z".to_string()),
        valid_until: None,
        inferred: false,
    }
}

fn bench_truncate_result(c: &mut Criterion) {
    let short = "ok";
    let long = "x".repeat(200);
    let multiline = "line1\nline2\nline3\nline4";
    let emoji = "🎉".repeat(120);
    c.bench_function("tool_call_info_truncate_short", |b| {
        b.iter(|| black_box(ToolCallInfo::truncate_result(black_box(short))))
    });
    c.bench_function("tool_call_info_truncate_long", |b| {
        b.iter(|| black_box(ToolCallInfo::truncate_result(black_box(&long))))
    });
    c.bench_function("tool_call_info_truncate_multiline", |b| {
        b.iter(|| black_box(ToolCallInfo::truncate_result(black_box(multiline))))
    });
    c.bench_function("tool_call_info_truncate_emoji", |b| {
        b.iter(|| black_box(ToolCallInfo::truncate_result(black_box(&emoji))))
    });
}

fn bench_serde_roundtrip(c: &mut Criterion) {
    let chat_req = ChatRequest {
        session_id: Some(123),
        message: "hello world".to_string(),
        model: Some("gpt-4o".to_string()),
        personality_preset: Some("concise".to_string()),
        incognito: Some(false),
    };
    let chat_resp = ChatResponse {
        session_id: 123,
        response: "hi there".to_string(),
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        },
        tool_calls: vec![ToolCallInfo {
            name: "echo".to_string(),
            display_name: "Echo".to_string(),
            result: "hello".to_string(),
        }],
    };
    let status = StatusResponse {
        version: "0.54.3".to_string(),
        uptime_seconds: 9999,
        queue_depth_user: 3,
        queue_depth_system: 1,
        hook_queue_depth: 2,
        worker_threads: 4,
        endpoint: "http://127.0.0.1:8080".to_string(),
        model: "gpt-4o".to_string(),
        config_path: Some("/cfg/config.toml".to_string()),
        config_exists: true,
        llm_reachable: true,
        context_window: Some(128_000),
        memory_exists: true,
        memory_chars: 1234,
        memory_limit: 10_000,
        memory_usage_pct: 12.34,
    };
    let fact_detail = FactDetailResponse {
        fact: sample_fact_row(),
        sources: vec![],
        dependencies: vec![],
        audit_log: vec![],
    };
    let forget = ForgetRequest {
        fact_id: Some(42),
        predicate: Some("lives_in".to_string()),
        subject: Some("Alice".to_string()),
        entity: Some("Alice".to_string()),
        source: Some("chat".to_string()),
        from: Some("2020-01-01T00:00:00Z".to_string()),
        to: Some("2021-01-01T00:00:00Z".to_string()),
        all: false,
        yes: true,
        confirm_sensitive: true,
        confirmation_phrase: Some("I am sure".to_string()),
        archive: true,
    };
    let audit = AuditQueryRequest {
        entity: Some("Alice".to_string()),
        predicate: Some("lives_in".to_string()),
        from: Some("2020-01-01T00:00:00Z".to_string()),
        to: Some("2021-01-01T00:00:00Z".to_string()),
        change_type: Some("status_change".to_string()),
        offset: Some(0),
        limit: Some(50),
    };
    let browse = BrowseRequest {
        entity: "Alice".to_string(),
        depth: 3,
        offset: Some(0),
        limit: Some(25),
    };
    let query = FactQueryParams {
        entity: "Alice".to_string(),
        predicate: Some("lives_in".to_string()),
        min_confidence: Some(0.5),
        offset: Some(0),
        limit: Some(10),
    };
    let pending = PendingListResponse {
        total: 5,
        facts: vec![
            PendingFactRow {
                fact_id: 1,
                subject: "Alice".to_string(),
                predicate: "ssn".to_string(),
                object: None,
                created_at: "2020-01-01T00:00:00Z".to_string(),
            },
            PendingFactRow {
                fact_id: 2,
                subject: "Bob".to_string(),
                predicate: "ssn".to_string(),
                object: None,
                created_at: "2020-01-01T00:00:00Z".to_string(),
            },
        ],
    };

    c.bench_function("serde_chat_request_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&chat_req).unwrap();
            let back: ChatRequest = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_chat_response_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&chat_resp).unwrap();
            let back: ChatResponse = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_status_response_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&status).unwrap();
            let back: StatusResponse = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_fact_detail_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&fact_detail).unwrap();
            let back: FactDetailResponse = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_forget_request_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&forget).unwrap();
            let back: ForgetRequest = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_audit_query_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&audit).unwrap();
            let back: AuditQueryRequest = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_browse_request_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&browse).unwrap();
            let back: BrowseRequest = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_fact_query_params_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&query).unwrap();
            let back: FactQueryParams = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
    c.bench_function("serde_pending_list_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&pending).unwrap();
            let back: PendingListResponse = serde_json::from_str(&json).unwrap();
            black_box(back);
        })
    });
}

criterion_group!(wire_types, bench_truncate_result, bench_serde_roundtrip);
criterion_main!(wire_types);
