//! Benchmark for the client-side SSE stream parser (issue #164).
//!
//! Exercises the partial-event accumulation path: many chunks with no
//! delimiter followed by a terminator. The legacy implementation re-scanned
//! the whole buffer per chunk (O(n^2)); the fixed version resumes from the
//! last inspected offset (O(n)) and caps the buffer.

use criterion::{Criterion, criterion_group, criterion_main};
use futures::StreamExt;
use mimir_api_types::StreamItem;
use std::hint::black_box;

fn chunk_bytes(n: usize) -> Vec<bytes::Bytes> {
    let chunk = b"data: a moderately sized payload chunk with no newline ";
    let mut chunks: Vec<bytes::Bytes> = (0..n).map(|_| bytes::Bytes::from_static(chunk)).collect();
    chunks.push(bytes::Bytes::from_static(b"\n\n"));
    chunks
}

async fn drain(
    stream: impl futures::Stream<Item = Result<StreamItem, mimir_client::ClientError>> + Unpin,
) {
    let mut s = stream;
    let mut count = 0usize;
    while let Some(item) = s.next().await {
        if let Ok(StreamItem::Text(ref t)) = item {
            count += 1;
            black_box(t.as_str());
        }
        black_box(&item);
    }
    black_box(count);
}

/// Legacy O(n^2) parser, mirroring the pre-fix implementation, kept here so the
/// benchmark can compare old vs new in a single run.
fn parse_sse_stream_legacy(
    stream: impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>>,
) -> impl futures::Stream<Item = Result<StreamItem, mimir_client::ClientError>> {
    use mimir_client::ClientError;
    let mut buf = Vec::new();
    stream
        .filter_map(move |result| match result {
            Ok(bytes) => {
                buf.extend_from_slice(&bytes);
                let mut items = Vec::new();
                while let Some((pos, delim_len)) = find_double_newline_legacy(&buf) {
                    let event_bytes: Vec<u8> = buf.drain(..pos + delim_len).collect();
                    match String::from_utf8(event_bytes) {
                        Ok(event) => {
                            if let Some(item) = mimir_client::parse_sse_event_pub(&event) {
                                items.push(item);
                            }
                        }
                        Err(_) => {
                            items.push(Err(ClientError::Connection(
                                "invalid UTF-8 in SSE event".to_string(),
                            )));
                        }
                    }
                }
                futures::future::ready(Some(items))
            }
            Err(e) => futures::future::ready(Some(vec![Err(ClientError::Http(e))])),
        })
        .flat_map(futures::stream::iter)
}

fn find_double_newline_legacy(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
    }
    None
}

fn make_result_stream(bytes: &[bytes::Bytes]) -> Vec<Result<bytes::Bytes, reqwest::Error>> {
    bytes.iter().cloned().map(Ok).collect()
}

fn bench_accumulate_partial_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("sse_accumulate");
    for n in [256usize, 1024, 4096] {
        group.throughput(criterion::Throughput::Elements((n + 1) as u64));
        let bytes = chunk_bytes(n);
        let rt = tokio::runtime::Runtime::new().unwrap();
        group.bench_function(format!("legacy_chunks_{n}"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let chunks = make_result_stream(&bytes);
                    drain(parse_sse_stream_legacy(futures::stream::iter(chunks))).await;
                });
            })
        });
        group.bench_function(format!("fixed_chunks_{n}"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let chunks = make_result_stream(&bytes);
                    drain(mimir_client::parse_sse_stream(futures::stream::iter(
                        chunks,
                    )))
                    .await;
                });
            })
        });
    }
    group.finish();
}

criterion_group!(sse_parser, bench_accumulate_partial_event);
criterion_main!(sse_parser);
