use bytes::Bytes;
use futures::{Stream, StreamExt};

use mimir_api_types::{StreamItem, ToolCallInfo, ToolCallStartInfo, Usage};

use crate::error::ClientError;

/// Maximum number of buffered bytes for a single SSE event before the parser
/// emits an error. Caps unbounded memory growth when a malformed stream never
/// emits a double-newline delimiter (issue #164).
const MAX_SSE_EVENT_SIZE: usize = 1024 * 1024; // 1 MiB

/// Parse a byte stream into SSE events.
///
/// Buffers raw bytes and only decodes complete events (delimited by `\n\n`)
/// so that multi-byte UTF-8 sequences split across TCP/HTTP chunk boundaries
/// are preserved.
///
/// The buffer is capped at [`MAX_SSE_EVENT_SIZE`] to prevent unbounded memory
/// growth, and the delimiter scan resumes from the last inspected offset so
/// the cost is linear rather than quadratic in the accumulated event size
/// (issue #164). Exposed publicly (`#[doc(hidden)]`) so benchmarks can drive
/// the parser directly.
#[doc(hidden)]
pub fn parse_sse_stream(
    stream: impl Stream<Item = Result<Bytes, ClientError>>,
) -> impl Stream<Item = Result<StreamItem, ClientError>> {
    let mut buf: Vec<u8> = Vec::new();
    // Index up to which `buf` has been confirmed to contain no delimiter.
    // Scanning resumes just before this point so delimiters straddling a chunk
    // boundary are still found.
    let mut scan_from: usize = 0;
    stream
        .filter_map(move |result| {
            let mut items = Vec::new();
            match result {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    loop {
                        match find_double_newline_from(&buf, scan_from) {
                            Some((pos, delim_len)) => {
                                let event_bytes: Vec<u8> = buf.drain(..pos + delim_len).collect();
                                // The remaining buffer is the tail after the event;
                                // rescan it from the start.
                                scan_from = 0;
                                match String::from_utf8(event_bytes) {
                                    Ok(event) => {
                                        if let Some(item) = parse_sse_event(&event) {
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
                            None => {
                                if buf.len() > MAX_SSE_EVENT_SIZE {
                                    items.push(Err(ClientError::Connection(
                                        "SSE event exceeded max size".to_string(),
                                    )));
                                    buf.clear();
                                    scan_from = 0;
                                } else {
                                    // Remember how far we've scanned so the next chunk
                                    // only inspects the newly appended tail plus a small
                                    // overlap for boundary-spanning delimiters. The
                                    // longest delimiter is 4 bytes, so overlap by 3.
                                    scan_from = buf.len().saturating_sub(3);
                                }
                                break;
                            }
                        }
                    }
                    futures::future::ready(Some(items))
                }
                Err(e) => futures::future::ready(Some(vec![Err(e)])),
            }
        })
        .flat_map(futures::stream::iter)
}

/// Return the index and delimiter length of the first `\n\n` or `\r\n\r\n`
/// in `buf`, starting the search at `start`.
fn find_double_newline_from(buf: &[u8], start: usize) -> Option<(usize, usize)> {
    let haystack = buf.get(start..)?;
    let lf = memchr::memmem::find(haystack, b"\n\n").map(|p| (start + p, 2));
    let crlf = memchr::memmem::find(haystack, b"\r\n\r\n").map(|p| (start + p, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Return the index and delimiter length of the first `\n\n` or `\r\n\r\n` in `buf`.
#[cfg(test)]
fn find_double_newline(buf: &[u8]) -> Option<(usize, usize)> {
    find_double_newline_from(buf, 0)
}

/// Parse a single SSE event block into a [`StreamItem`] or an error.
fn parse_sse_event(event: &str) -> Option<Result<StreamItem, ClientError>> {
    let mut event_type = "";
    let mut data = String::new();
    for line in event.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            // Per SSE spec: strip exactly one leading space after "data:", not all whitespace.
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            data.push_str(value);
        }
    }
    match event_type {
        "usage" => match serde_json::from_str::<Usage>(&data) {
            Ok(u) => Some(Ok(StreamItem::Usage(u))),
            Err(e) => Some(Err(ClientError::Serialization(e))),
        },
        "tool_call" => match serde_json::from_str::<ToolCallInfo>(&data) {
            Ok(info) => Some(Ok(StreamItem::ToolCall(info))),
            Err(e) => Some(Err(ClientError::Serialization(e))),
        },
        "tool_call_start" => match serde_json::from_str::<ToolCallStartInfo>(&data) {
            Ok(info) => Some(Ok(StreamItem::ToolCallStart(info))),
            Err(e) => Some(Err(ClientError::Serialization(e))),
        },
        "session_id" => match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(v) => v
                .get("session_id")
                .and_then(|s| s.as_i64())
                .map(|id| Ok(StreamItem::SessionId(id.to_string()))),
            Err(_) => None,
        },
        "error" => Some(Err(ClientError::Server {
            status: 500,
            message: data,
        })),
        // default / no event type → text
        _ => {
            if data.is_empty() {
                None
            } else {
                Some(Ok(StreamItem::Text(data)))
            }
        }
    }
}

/// Re-export of the internal SSE event parser for benchmarks.
#[doc(hidden)]
pub fn parse_sse_event_pub(event: &str) -> Option<Result<StreamItem, ClientError>> {
    parse_sse_event(event)
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_tool_call_event() {
        let event = "event: tool_call\ndata: {\"name\":\"get_current_time\",\"display_name\":\"Get Current Time\",\"result\":\"2025-05-30T12:00:00Z\"}\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::ToolCall(info)) => {
                assert_eq!(info.name, "get_current_time");
                assert_eq!(info.display_name, "Get Current Time");
                assert_eq!(info.result, "2025-05-30T12:00:00Z");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_default_event_is_text() {
        let event = "data: Hello world\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, "Hello world"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_with_leading_space_preserved() {
        // When the LLM streams a token like " on", the SSE data line becomes
        // "data:  on" (two spaces after colon). Per SSE spec only the first
        // space after "data:" is stripped; the second is part of the content.
        let event = "data:  on\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, " on"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_session_id_event() {
        let event = "event: session_id\ndata: {\"session_id\":123}\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::SessionId(s)) => assert_eq!(s, "123"),
            other => panic!("expected SessionId, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_no_space_after_colon() {
        // "data:" with no space is valid SSE; content starts immediately.
        let event = "data:hello\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    // ---- pure SSE-parser unit tests ---------------------------------------
    #[test]
    fn find_double_newline_lf() {
        assert_eq!(find_double_newline(b"a\n\nb"), Some((1, 2)));
    }

    #[test]
    fn find_double_newline_crlf() {
        assert_eq!(find_double_newline(b"a\r\n\r\nb"), Some((1, 4)));
    }

    #[test]
    fn find_double_newline_none() {
        assert_eq!(find_double_newline(b"no delimiter here"), None);
    }

    #[test]
    fn find_double_newline_first_occurrence_wins() {
        // The first \n\n must be reported, not a later one.
        assert_eq!(find_double_newline(b"x\n\ny\n\nz"), Some((1, 2)));
    }

    #[test]
    fn find_double_newline_empty_buffer() {
        assert_eq!(find_double_newline(b""), None);
    }

    #[test]
    fn parse_sse_event_text_default() {
        let item = parse_sse_event("data: hello world\n").unwrap().unwrap();
        assert_eq!(item, StreamItem::Text("hello world".to_string()));
    }

    #[test]
    fn parse_sse_event_text_multiline_data_concatenated() {
        // Multiple `data:` lines are joined with `\n` per the SSE spec.
        let item = parse_sse_event("data: line1\ndata: line2\n")
            .unwrap()
            .unwrap();
        assert_eq!(item, StreamItem::Text("line1\nline2".to_string()));
    }

    #[test]
    fn parse_sse_event_text_no_leading_space() {
        // Per SSE spec exactly one leading space is stripped; no space is kept.
        let item = parse_sse_event("data:nospace\n").unwrap().unwrap();
        assert_eq!(item, StreamItem::Text("nospace".to_string()));
    }

    #[test]
    fn parse_sse_event_usage() {
        let item =
            parse_sse_event("event: usage\ndata: {\"prompt_tokens\":4,\"completion_tokens\":5,\"total_tokens\":9}\n")
                .unwrap()
                .unwrap();
        assert_eq!(
            item,
            StreamItem::Usage(Usage {
                prompt_tokens: 4,
                completion_tokens: 5,
                total_tokens: 9,
            })
        );
    }

    #[test]
    fn parse_sse_event_usage_invalid_json_returns_error() {
        let item = parse_sse_event("event: usage\ndata: {bad json\n").unwrap();
        assert!(matches!(item, Err(ClientError::Serialization(_))));
    }

    #[test]
    fn parse_sse_event_tool_call() {
        let item = parse_sse_event(
            "event: tool_call\ndata: {\"name\":\"echo\",\"display_name\":\"Echo\",\"result\":\"hi\"}\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            item,
            StreamItem::ToolCall(ToolCallInfo {
                name: "echo".to_string(),
                display_name: "Echo".to_string(),
                result: "hi".to_string(),
            })
        );
    }

    #[test]
    fn parse_sse_event_tool_call_start() {
        let item = parse_sse_event(
            "event: tool_call_start\ndata: {\"name\":\"echo\",\"display_name\":\"Echo\"}\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            item,
            StreamItem::ToolCallStart(ToolCallStartInfo {
                name: "echo".to_string(),
                display_name: "Echo".to_string(),
            })
        );
    }

    #[test]
    fn parse_sse_event_session_id() {
        let item = parse_sse_event("event: session_id\ndata: {\"session_id\":12345}\n")
            .unwrap()
            .unwrap();
        assert_eq!(item, StreamItem::SessionId("12345".to_string()));
    }

    #[test]
    fn parse_sse_event_session_id_missing_field_is_none() {
        // No `session_id` key → no item emitted.
        let item = parse_sse_event("event: session_id\ndata: {}\n");
        assert!(item.is_none());
    }

    #[test]
    fn parse_sse_event_error() {
        let item = parse_sse_event("event: error\ndata: boom\n").unwrap();
        assert!(
            matches!(item, Err(ClientError::Server { status: 500, message }) if message == "boom")
        );
    }

    #[test]
    fn parse_sse_event_empty_data_returns_none() {
        // Default event with no data yields no item.
        assert!(parse_sse_event("event: message\n").is_none());
        assert!(parse_sse_event("").is_none());
    }

    #[test]
    fn find_double_newline_from_resumes_after_cursor() {
        // The cursor scan must still find a delimiter that appears after the
        // already-scanned prefix.
        assert_eq!(
            find_double_newline_from(b"prefix no delim\n\n", 5),
            Some((15, 2))
        );
        assert_eq!(find_double_newline_from(b"prefix\r\n\r\n", 3), Some((6, 4)));
        // Start beyond buffer length → None.
        assert_eq!(find_double_newline_from(b"abc\n\n", 99), None);
    }

    #[tokio::test]
    async fn parse_sse_stream_caps_unbounded_buffer() {
        // Issue #164: a stream that never emits a double-newline delimiter
        // must not grow the buffer without bound — it should produce an error.
        let big = bytes::Bytes::from(vec![b'a'; MAX_SSE_EVENT_SIZE + 1]);
        let chunks: Vec<Result<bytes::Bytes, ClientError>> = vec![Ok(big)];
        let mut stream = parse_sse_stream(futures::stream::iter(chunks));
        use futures::StreamExt;
        let item = stream.next().await.unwrap();
        assert!(
            matches!(item, Err(ClientError::Connection(ref m)) if m.contains("exceeded max size")),
            "unexpected item: {item:?}"
        );
    }

    #[tokio::test]
    async fn parse_sse_stream_handles_boundary_spanning_delimiter() {
        // Split a `\r\n\r\n` delimiter across two chunks so the first byte of
        // the delimiter is the last byte of chunk 1. The overlap scan must
        // still find it.
        let chunk1 = bytes::Bytes::from_static(b"data: hello\r");
        let chunk2 = bytes::Bytes::from_static(b"\n\r\ndata: world\n\n");
        let chunks: Vec<Result<bytes::Bytes, ClientError>> = vec![Ok(chunk1), Ok(chunk2)];
        let mut stream = parse_sse_stream(futures::stream::iter(chunks));
        use futures::StreamExt;
        let mut texts = Vec::new();
        while let Some(Ok(StreamItem::Text(t))) = stream.next().await {
            texts.push(t);
        }
        assert_eq!(texts, vec!["hello".to_string(), "world".to_string()]);
    }
}
