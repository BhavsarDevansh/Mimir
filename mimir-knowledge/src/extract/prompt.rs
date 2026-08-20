//! Conversation-aware extraction prompts (the Librarian).

use mimir_core::conversation::ConversationMessage;
use mimir_core::personality::Personality;

use crate::{KnowledgeError, KnowledgeGraph};

/// predicate standards, list splitting, within-output deduplication, and the
/// output contract.
///
/// Shared by the simple [`extract_facts`] path (no contextual inputs) and the
/// rich [`build_extraction_prompt`] (which layers the core-facts block and
/// recent conversation on top).
pub(super) async fn build_base_prompt(kg: &KnowledgeGraph) -> Result<String, KnowledgeError> {
    let roots = kg.list_categories(None).await?;
    let mut guide = String::from("Categorisation Guide:\n");
    for root in roots {
        guide.push_str(&format!("{} {}\n", root.id, root.name));
        let children = kg.list_categories(Some(root.id)).await?;
        for child in children {
            guide.push_str(&format!("  {} {}\n", child.id, child.name));
        }
    }

    Ok(format!(
        "You are a fact extractor. Read the user message and emit structured facts via the 'remember' tool.\n\n### Rules\n- Classify each fact as Explicit, Casual, or Correction.\n- For Corrections, set correction_scope to 'always' or an ISO-8601 datetime.\n- Flag health, financial, relationship, religious, political, or legal facts as is_sensitive=true. Mimir will validate your assessment in Rust.\n- Subject and object types must be one of: Person, Place, Event, Object, Concept, Organization, Activity, DateTime.\n- Assign 1-3 category IDs from the guide below to each fact. Use the MOST specific sub-category available.\n- Emit one fact per list item.\n{}\n### Predicate standards (critical)\nUse the EXACT predicate name below for the matching scenario. Do NOT invent synonyms.\n- Education\n  * Where someone studied   → studied_at (NOT 'attended')\n  * What someone studied    → studied\n  * Degree completed        → completed_degree\n  * Degree status           → educational_status\n- Employment\n  * Employer                → works_at\n  * Job title               → job_title\n  * Profession              → works_as\n- Residence\n  * Current city/country    → resides_in\n  * Previous city           → resides_in (with valid_until)\n- Personal\n  * Hobby (one per fact)    → hobby (NOT 'hobbies')\n  * Favourite thing         → favourite_{{thing}}\n  * Name                    → has_name\n  * Preferred name          → preferred_name\n  * Pet ownership           → has_pets\n- Family\n  * Sibling                 → has_sibling\n  * Partner                 → has_partner\n  * Parent                  → has_parent\n  * Child                   → has_child\n### Deduplication\nBefore emitting a fact, ask yourself: 'Have I already emitted a fact with the same subject and the same meaning?' If yes, do not emit the duplicate — instead strengthen the confidence by marking it Explicit.\nExample: If you already emitted studied_at='University of Auckland', do NOT also emit attended='University of Auckland'.\n### Output\nEmit ONLY via the 'remember' tool. Do not output free text.",
        guide
    ))
}

// ---------------------------------------------------------------------------
// Rich contextual prompt (Librarian)
// ---------------------------------------------------------------------------

/// Build the Librarian's extraction prompt: the KG-focused base
/// ([`build_base_prompt`]), the same core-facts block the core agent injects,
/// the recent conversation as labelled messages, and instructions to extract
/// only from user-authored messages and only facts not already known.
///
/// Identity is not a parameter: the user's canonical name and entity details
/// live in the condensed core-facts block, exactly as the core agent resolves
/// identity (#139). `messages` is a slice so the amount of conversation
/// context handed to the Librarian can be increased in future without
/// changing this signature.
pub(super) async fn build_extraction_prompt(
    kg: &KnowledgeGraph,
    condensed_memory: Option<&str>,
    messages: &[ConversationMessage],
) -> Result<String, KnowledgeError> {
    let base = build_base_prompt(kg).await?;

    // Core-facts block — identical header and framing to the core agent's
    // `Personality::system_prompt`, emitted only when non-empty.
    let memory = condensed_memory.map(str::trim).unwrap_or("").trim();
    let core_facts = if memory.is_empty() {
        String::new()
    } else {
        format!("\n\n{}\n{}", Personality::CORE_FACTS_HEADER, memory)
    };

    // Recent conversation as labelled messages. The Librarian extracts only
    // from [User] messages; [Assistant] messages are its own prior output.
    let mut transcript = String::from("\n\n## Recent conversation\n");
    for msg in messages {
        // Escape newlines so message content cannot forge a labelled line
        // (e.g. an embedded "[Assistant]: ...") and bypass source discipline.
        let escaped = msg.content.replace('\r', "\\r").replace('\n', "\\n");
        transcript.push_str(&format!("[{}]: {}\n", msg.label(), escaped));
    }

    // Source discipline + novelty check, governing the conversation and
    // core-facts block above.
    let instructions = "\n### Source discipline\n\
        Extract facts ONLY from messages labelled [User] in the Recent conversation above. \
        NEVER extract facts from messages labelled [Assistant] — those are your own prior \
        output to the user, not new information from the user.\n\
        \n### Novelty check\n\
        Before emitting a fact, check it against the Core facts block above. \
        Do NOT emit a fact that merely restates something already present there — \
        exact duplicates are discarded by Rust regardless of classification, so \
        reclassifying a duplicate does not strengthen anything. Emit a fact only when \
        it is genuinely new, or when it corrects/updates an existing one (use the \
        Correction classification for corrections).";

    Ok(format!(
        "{}{}{}{}",
        base, core_facts, transcript, instructions
    ))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod prompt_tests;
