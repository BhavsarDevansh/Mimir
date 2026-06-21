//! Micro-benchmarks for non-hotpath pure helpers in mimir-core.
//!
//! These exercise small, deterministic functions that are easy to skip when
//! focusing on the hotpath: FTS5 escaping, daily-schedule arithmetic, tool
//! output rendering, and config TOML parsing.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use mimir_core::{
    config::Config,
    fts5::escape_fts5,
    job_queue::{DailySchedule, JobPriority, JobRunStatus},
    tools::{ToolOutput, output_to_llm_text},
};

fn bench_escape_fts5(c: &mut Criterion) {
    let inputs = [
        "hello world",
        "foo OR bar AND NOT baz",
        "a\"b\"c*d*e",
        "(parentheses) and -dashes-",
        "🎉 unicode mixed with ASCII keywords",
    ];
    c.bench_function("fts5_escape_mixed_inputs", |b| {
        b.iter(|| {
            for input in inputs {
                black_box(escape_fts5(input));
            }
        })
    });
}

fn bench_daily_schedule_next_after(c: &mut Criterion) {
    let schedule = DailySchedule::parse("03:30").unwrap();
    let now = chrono::Utc::now();
    c.bench_function("daily_schedule_next_after", |b| {
        b.iter(|| black_box(schedule.next_after(now)))
    });
}

fn bench_daily_schedule_parse(c: &mut Criterion) {
    let inputs = ["02:00", "23:59", "00:00", "12:30", "06:15"];
    c.bench_function("daily_schedule_parse", |b| {
        b.iter(|| {
            for input in inputs {
                let parsed = DailySchedule::parse(input).unwrap();
                black_box(parsed);
            }
        })
    });
}

fn bench_job_queue_serde(c: &mut Criterion) {
    // Exercise the public serde roundtrip pathway for queue enums.
    let statuses = [
        JobRunStatus::Running,
        JobRunStatus::Succeeded,
        JobRunStatus::Failed,
        JobRunStatus::TimedOut,
        JobRunStatus::Cancelled,
    ];
    c.bench_function("job_run_status_serde_roundtrip", |b| {
        b.iter(|| {
            for s in statuses {
                let json = serde_json::to_string(&s).unwrap();
                let back: JobRunStatus = serde_json::from_str(&json).unwrap();
                black_box(back);
            }
        })
    });
    let priorities = [
        JobPriority::System,
        JobPriority::Maintenance,
        JobPriority::User,
    ];
    c.bench_function("job_priority_serde_roundtrip", |b| {
        b.iter(|| {
            for p in priorities {
                let json = serde_json::to_string(&p).unwrap();
                let back: JobPriority = serde_json::from_str(&json).unwrap();
                black_box(back);
            }
        })
    });
}

fn bench_tool_output_rendering(c: &mut Criterion) {
    let out = ToolOutput {
        result: Some(serde_json::json!({"answer": 42, "items": [1, 2, 3]})),
        error: None,
        stdout: Some("line one\nline two\nline three\n".to_string()),
        stderr: Some("warning: deprecation\n".to_string()),
        exit_code: Some(0),
    };
    c.bench_function("tool_output_to_llm_text", |b| {
        b.iter(|| black_box(out.to_llm_text()))
    });
    c.bench_function("tool_output_to_display_text", |b| {
        b.iter(|| black_box(out.to_display_text()))
    });
    c.bench_function("output_to_llm_text_helper", |b| {
        b.iter(|| {
            black_box(output_to_llm_text(
                out.result.as_ref(),
                out.error.as_ref(),
                out.stdout.as_ref(),
                out.stderr.as_ref(),
                out.exit_code,
            ))
        })
    });
}

fn bench_config_toml_parse(c: &mut Criterion) {
    let toml = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-test"
model = "gpt-4o"

[agent]
max_tool_rounds = 8

[memory]
char_limit = 2000

[server]
bind_addr = "127.0.0.1:8080"

[personality]
preset = "default"

[scheduler]
debounce_seconds = 5
cooldown_seconds = 30
"#;
    c.bench_function("config_toml_parse", |b| {
        b.iter(|| black_box(toml::from_str::<Config>(black_box(toml)).unwrap()))
    });
}

criterion_group!(
    pure_helpers,
    bench_escape_fts5,
    bench_daily_schedule_next_after,
    bench_daily_schedule_parse,
    bench_job_queue_serde,
    bench_tool_output_rendering,
    bench_config_toml_parse,
);
criterion_main!(pure_helpers);
