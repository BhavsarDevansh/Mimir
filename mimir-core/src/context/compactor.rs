//! LLM session compaction (issue #279): summarises the oldest complete turns
//! beyond the compaction window, persists the summary on the session, and
//! deletes the summarised messages so trimming never silently discards
//! context.
//!
//! The pipeline is deterministic: [`SessionCompactor`] snapshots the turns
//! via [`ContextManager::compaction_candidates`], asks the LLM for a concise
//! summary (folding in any previous summary), and commits through
//! [`ContextManager::apply_compaction`]. If the LLM call fails, the compacted
//! transcript itself is kept (character-capped) so no context is lost — the
//! same fallback philosophy as memory condensation.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use crate::context::{CompactionCandidates, ContextError, ContextManager, ContextMessage};
use crate::llm::LlmBackend;
use crate::llm::types::{LlmError, Message};

/// Upper bound on the stored summary length, bounding the summary's share of
/// every future prompt and of the resume flow.
pub const MAX_COMPACTION_SUMMARY_CHARS: usize = 2000;

/// Outcome of one successful compaction run.
#[derive(Debug, Clone)]
pub struct CompactionOutcome {
    /// Number of complete turns summarised and deleted.
    pub compacted_turns: usize,
    /// The stored summary.
    pub summary: String,
    /// True when the LLM call failed and the transcript was kept verbatim
    /// (character-capped) instead of an LLM summary.
    pub deterministic_fallback: bool,
    /// Timestamp of the last summarised message; the retained window starts
    /// after it.
    pub compacted_at: DateTime<Utc>,
}

/// A summarisation attempt that failed or produced no text.
#[derive(Debug)]
enum SummariseError {
    Llm(LlmError),
    Empty,
}

/// Orchestrates one session-compaction run: DB snapshot → LLM summary →
/// commit.
pub struct SessionCompactor {
    context: Arc<ContextManager>,
    llm: Arc<dyn LlmBackend>,
    max_turns: u16,
}

impl SessionCompactor {
    /// Create a compactor for sessions that keep at most `max_turns`
    /// complete turns; older complete turns are summarised and removed.
    pub fn new(context: Arc<ContextManager>, llm: Arc<dyn LlmBackend>, max_turns: u16) -> Self {
        Self {
            context,
            llm,
            max_turns,
        }
    }

    /// Compact one session, returning `None` when nothing is beyond the
    /// window.
    pub async fn compact_session(
        &self,
        session_id: i64,
    ) -> Result<Option<CompactionOutcome>, ContextError> {
        let Some(candidates) = self
            .context
            .compaction_candidates(session_id, self.max_turns)
            .await?
        else {
            debug!(session_id, "session.compaction: nothing beyond the window");
            return Ok(None);
        };

        let transcript = render_transcript(&candidates.turn_messages);
        if transcript.trim().is_empty() {
            debug!(session_id, "session.compaction: transcript empty; skipping");
            return Ok(None);
        }

        let (summary, deterministic_fallback) = match self.summarise(&candidates, &transcript).await
        {
            Ok(text) => (truncate(&text, MAX_COMPACTION_SUMMARY_CHARS), false),
            Err(error) => {
                let detail = match &error {
                    SummariseError::Llm(llm_error) => llm_error.to_string(),
                    SummariseError::Empty => "empty response".to_string(),
                };
                warn!(
                    session_id,
                    "session.compaction: LLM summarisation failed ({detail}); keeping the transcript verbatim"
                );
                (truncate(&transcript, MAX_COMPACTION_SUMMARY_CHARS), true)
            }
        };

        let compacted_at = candidates
            .turn_messages
            .last()
            .expect("candidates are non-empty")
            .created_at;
        self.context
            .apply_compaction(session_id, &summary, compacted_at, &candidates.delete_ids)
            .await?;

        info!(
            session_id,
            compacted_turns = candidates.compacted_turns,
            deterministic_fallback,
            "session.compaction: summarised and removed old turns"
        );
        Ok(Some(CompactionOutcome {
            compacted_turns: candidates.compacted_turns,
            summary,
            deterministic_fallback,
            compacted_at,
        }))
    }

    async fn summarise(
        &self,
        candidates: &CompactionCandidates,
        transcript: &str,
    ) -> Result<String, SummariseError> {
        let system = Message::system(
            "You summarise conversation transcripts for a personal assistant. You preserve concrete facts, decisions, names, dates, and unresolved items. You never invent anything not present in the transcript or the previous summary. You output plain prose only, without headings or commentary.",
        );

        let mut prompt = String::new();
        if let Some(previous) = &candidates.existing_summary {
            prompt.push_str(
                "Previous summary (already compacted earlier):
",
            );
            prompt.push_str(previous);
            prompt.push('\n');
        }
        prompt.push_str(
            "Transcript of the older turns:
",
        );
        prompt.push_str(transcript);
        prompt.push_str(&format!(
            "
Write one concise summary of the transcript that also carries forward anything still relevant from the previous summary. Max {} characters.",
            MAX_COMPACTION_SUMMARY_CHARS
        ));

        let (response, _) = self
            .llm
            .chat(vec![system, Message::user(prompt)], None)
            .await
            .map_err(SummariseError::Llm)?;
        if response.trim().is_empty() {
            return Err(SummariseError::Empty);
        }
        Ok(response)
    }
}

/// Render the compacted messages as a labelled transcript for the LLM.
/// Assistant tool-call JSON is machine-oriented and skipped; tool results
/// are kept with their role label.
fn render_transcript(messages: &[ContextMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{}: {}
",
            message.role, content
        ));
    }
    out
}

/// Truncate `text` to at most `max_chars` characters, appending an ellipsis
/// when cut.
fn truncate(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextManager;
    use crate::llm::{LlmError, MockLlmClient, Usage};
    use std::sync::Arc;

    async fn seed(turns: u32) -> (Arc<ContextManager>, tempfile::TempDir, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let mgr = Arc::new(ContextManager::new(&db).await.unwrap());
        let sid = mgr.create_session("sys").await.unwrap();
        for i in 0..turns {
            mgr.add_user_message(sid, format!("u{i}")).await.unwrap();
            mgr.add_assistant_message(sid, format!("a{i}"))
                .await
                .unwrap();
        }
        (mgr, dir, sid)
    }

    #[tokio::test]
    async fn compact_session_writes_llm_summary_and_removes_old_turns() {
        let (mgr, _dir, sid) = seed(25).await;
        let llm: Arc<dyn crate::llm::LlmBackend> = Arc::new(
            MockLlmClient::builder()
                .push_chat("Earlier: holiday plans in Rome", Usage::default())
                .build(),
        );
        let compactor = SessionCompactor::new(Arc::clone(&mgr), llm, 15);

        let outcome = compactor
            .compact_session(sid)
            .await
            .unwrap()
            .expect("25 turns with a 15-turn window must compact");
        assert_eq!(outcome.compacted_turns, 10);
        assert_eq!(outcome.summary, "Earlier: holiday plans in Rome");
        assert!(!outcome.deterministic_fallback);

        let session = mgr.load_session(sid).await.unwrap();
        assert_eq!(
            session.summary.as_deref(),
            Some("Earlier: holiday plans in Rome")
        );
        assert!(session.compacted_at.is_some());

        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs.len(), 32, "system + summary + 15 retained turns");
        assert_eq!(msgs[2].content, "u10");
    }

    #[tokio::test]
    async fn compact_session_nothing_to_do_below_window() {
        let (mgr, _dir, sid) = seed(10).await;
        let mock = Arc::new(MockLlmClient::builder().build());
        let llm: Arc<dyn crate::llm::LlmBackend> = mock.clone();
        let compactor = SessionCompactor::new(Arc::clone(&mgr), llm, 15);

        let outcome = compactor.compact_session(sid).await.unwrap();
        assert!(outcome.is_none(), "below-window sessions must not compact");
        assert!(mock.chat_calls().is_empty(), "no LLM call when idle");
        let session = mgr.load_session(sid).await.unwrap();
        assert!(session.summary.is_none());
        assert!(session.compacted_at.is_none());
    }

    #[tokio::test]
    async fn compact_session_falls_back_deterministically_on_llm_error() {
        let (mgr, _dir, sid) = seed(25).await;
        let llm: Arc<dyn crate::llm::LlmBackend> = Arc::new(
            MockLlmClient::builder()
                .push_chat_error(LlmError::QueueFull)
                .build(),
        );
        let compactor = SessionCompactor::new(Arc::clone(&mgr), llm, 15);

        let outcome = compactor
            .compact_session(sid)
            .await
            .unwrap()
            .expect("compaction must still progress when the LLM fails");
        assert!(outcome.deterministic_fallback);
        assert!(
            outcome.summary.contains("u0"),
            "fallback keeps the transcript"
        );
        assert!(outcome.summary.contains("a0"));

        let session = mgr.load_session(sid).await.unwrap();
        assert!(session.summary.is_some());
        assert!(session.compacted_at.is_some());
        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs.len(), 32);
    }

    #[tokio::test]
    async fn compact_session_accumulates_previous_summary_into_prompt() {
        let (mgr, _dir, sid) = seed(25).await;
        let llm = Arc::new(
            MockLlmClient::builder()
                .push_chat("First summary", Usage::default())
                .push_chat("Second summary", Usage::default())
                .build(),
        );
        let llm_backend: Arc<dyn crate::llm::LlmBackend> = llm.clone();
        let compactor = SessionCompactor::new(Arc::clone(&mgr), llm_backend, 15);

        compactor.compact_session(sid).await.unwrap().unwrap();
        for i in 25..28 {
            mgr.add_user_message(sid, format!("u{i}")).await.unwrap();
            mgr.add_assistant_message(sid, format!("a{i}"))
                .await
                .unwrap();
        }
        compactor.compact_session(sid).await.unwrap().unwrap();

        let calls = llm.chat_calls();
        assert_eq!(calls.len(), 2);
        let second_prompt = calls[1][1].content.clone();
        assert!(
            second_prompt.contains("First summary"),
            "the second summarisation must fold in the previous summary"
        );
        assert!(
            second_prompt.contains("u10"),
            "the second batch compacts the oldest retained turns"
        );
    }
}
