//! The fact-extraction pipeline: LLM call, schema validation, fact
//! conversion, insertion, and inference trigger.

use std::sync::Arc;

use mimir_core::conversation::ConversationMessage;
use mimir_core::llm::backend::LlmBackend;
use mimir_core::llm::types::Message;

use crate::extract::parse::{parse_extracted_fact, parse_remember_output, split_list_objects};
use crate::extract::prompt::{build_base_prompt, build_extraction_prompt};
use crate::extract::schema::{ExtractedFact, RememberOutput};
use crate::extract::tool::remember_tool_schema;
use crate::models::source::ExtractionMethod;
use crate::normalize::{ExtractionOutcome, NormalizedFact, Provenance, normalize_and_insert};
use crate::{KnowledgeError, KnowledgeGraph};

// ---------------------------------------------------------------------------

/// Run the fact extraction pipeline on a single user message.
///
/// 1. Calls the LLM via the `remember` tool.
/// 2. Validates schema, resolves entities, checks dedup.
/// 3. Assigns confidence based on classification.
/// 4. Handles corrections (temporal or retrospective).
/// 5. Flags sensitive facts for confirmation.
/// 6. Inserts facts, attaches sources, triggers inference.
pub async fn extract_facts(
    kg: &KnowledgeGraph,
    llm: &Arc<dyn LlmBackend>,
    user_message: &str,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let prompt = build_base_prompt(kg).await?;
    let messages = vec![Message::system(prompt), Message::user(user_message)];

    let (assistant_msg, _usage) = llm
        .chat_message(messages, Some(vec![remember_tool_schema().clone()]))
        .await
        .map_err(|e| KnowledgeError::Validation(format!("LLM call failed: {}", e)))?;

    let extracted = parse_remember_output(assistant_msg)?;
    process_remember_output(kg, extracted).await
}

/// Run the fact extraction pipeline over a labelled conversation transcript
/// with the condensed core-facts block injected into the prompt.
///
/// The transcript is supplied as a slice of [`ConversationMessage`]s so the
/// caller controls how much context is sent (last user + assistant pair today,
/// expandable in future). Identity is read by the LLM from the core-facts
/// block, not passed as a parameter.
pub async fn extract_facts_with_context(
    kg: &KnowledgeGraph,
    llm: &Arc<dyn LlmBackend>,
    messages: &[ConversationMessage],
    condensed_memory: Option<&str>,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let prompt = build_extraction_prompt(kg, condensed_memory, messages).await?;
    // The transcript is embedded in the system prompt above; the user turn is
    // just the action instruction so the LLM is not handed the conversation
    // twice.
    let llm_messages = vec![
        Message::system(prompt),
        Message::user(
            "Analyse the labelled Recent conversation above and emit any new \
             facts about the user via the 'remember' tool, following the rules, \
             source-discipline, and novelty-check in this system prompt.",
        ),
    ];
    let (assistant_msg, _usage) = llm
        .chat_message(llm_messages, Some(vec![remember_tool_schema().clone()]))
        .await
        .map_err(|e| KnowledgeError::Validation(format!("LLM call failed: {}", e)))?;

    let extracted = parse_remember_output(assistant_msg)?;
    process_remember_output(kg, extracted).await
}

pub async fn process_remember_output(
    kg: &KnowledgeGraph,
    output: RememberOutput,
) -> Result<ExtractionOutcome, KnowledgeError> {
    // Conversational learning always comes through the LLM `remember` tool.
    let provenance = Provenance::chat(ExtractionMethod::LlmExtraction);
    let (normalized, build_errors) = extracted_to_normalized(kg, output.facts).await;

    let mut outcome = normalize_and_insert(kg, normalized, provenance).await?;
    // Prepend any predicate-canonicalisation / parse errors so callers see the
    // full picture (these never abort the batch).
    let mut errors = build_errors;
    errors.append(&mut outcome.errors);
    outcome.errors = errors;
    Ok(outcome)
}

/// Adapt LLM-emitted [`ExtractedFact`]s onto the shared
/// [`crate::normalize::NormalizedFact`] shape.
///
/// This is the conversational-only normalisation the shared boundary cannot do:
/// predicate canonicalisation (so list-splitting sees canonical names), list
/// splitting (the LLM may cram a list into one fact), and parsing the LLM's
/// string-typed entity/temporal/recurrence/category fields into the typed
/// `NormalizedFact`. Per-fact canonicalisation/parse errors are collected and
/// returned alongside the successfully-built facts so one bad fact never aborts
/// the batch - mirroring the old `process_fact_batch` tolerance.
async fn extracted_to_normalized(
    kg: &KnowledgeGraph,
    facts: Vec<ExtractedFact>,
) -> (Vec<NormalizedFact>, Vec<KnowledgeError>) {
    let mut normalized = Vec::new();
    let mut errors = Vec::new();

    for mut fact in facts {
        // Canonicalise the predicate once: `resolve_canonical_relationship_type`
        // enforces the Rust-side allow-list (issue #401) — seeded predicates
        // and their aliases resolve to the canonical id, the prompt-instructed
        // `favourite_*` family is accepted, and any other predicate is rejected
        // with a clear error instead of auto-creating a `relationship_types`
        // row. The canonical name drives list-splitting below.
        // `normalize_and_insert` re-resolves the id (idempotently) through the
        // permissive shared boundary, so the strict check above is not
        // repeated downstream.
        let relationship_type_id = match kg
            .resolve_canonical_relationship_type(&fact.relationship_type)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let canonical_name = kg.relationship_type_name(relationship_type_id).await;
        fact.relationship_type = canonical_name
            .unwrap_or_else(|| crate::normalize_alias(&fact.relationship_type).unwrap_or_default());

        for fact in split_list_objects(&fact) {
            match parse_extracted_fact(&fact) {
                Ok(nf) => normalized.push(nf),
                Err(error) => errors.push(error),
            }
        }
    }

    (normalized, errors)
}
