//! Conversation-aware extraction prompts (the Librarian).

use mimir_core::conversation::ConversationMessage;
use mimir_core::personality::Personality;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::graph::EmitEligiblePredicate;
use crate::models::category::Category;
use crate::{KnowledgeError, KnowledgeGraph};

/// Render a category name as one prompt-safe line.
fn category_display_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render every category and its descendants for the extraction prompt.
///
/// The guide is DB-driven so taxonomy changes cannot silently diverge from the
/// categories the model is allowed to use.
async fn build_category_guide(kg: &KnowledgeGraph) -> Result<String, KnowledgeError> {
    let categories = kg.list_all_categories().await?;
    let mut children_by_parent: HashMap<Option<i32>, Vec<&Category>> = HashMap::new();
    for category in &categories {
        children_by_parent
            .entry(category.parent_id)
            .or_default()
            .push(category);
    }
    let mut guide = String::from("Categorisation Guide:\n");
    let mut visited = HashSet::new();
    let mut stack: Vec<(&Category, usize)> = children_by_parent
        .get(&None)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .rev()
        .map(|category| (*category, 0))
        .collect();

    while let Some((category, depth)) = stack.pop() {
        if !visited.insert(category.id) {
            continue;
        }

        for _ in 0..depth {
            guide.push_str("  ");
        }
        guide.push_str(&format!(
            "{} {}\n",
            category.id,
            category_display_name(&category.name)
        ));

        let children = children_by_parent
            .get(&Some(category.id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for child in children.iter().rev() {
            stack.push((child, depth + 1));
        }
    }

    Ok(guide)
}

/// Role statement for the KG-focused extraction prompts.
const EXTRACTION_ROLE: &str = "You are a fact extractor. Read the user \
message and emit structured facts via the 'remember' tool.";

/// KG-focused extraction rules, independent of any per-call context.
const EXTRACTION_RULES: &str = "### Rules\n\
    - Classify each fact as Explicit, Casual, or Correction.\n\
    - For Corrections, set correction_scope to 'always' or an ISO-8601 datetime.\n\
    - Flag health, financial, relationship, religious, political, or legal facts as is_sensitive=true. Mimir will validate your assessment in Rust.\n\
    - Subject and object types must be one of: Person, Place, Event, Object, Concept, Organization, Activity, DateTime.\n\
    - Assign 1-3 valid category IDs from the guide below to each fact. Use the MOST specific sub-category available.\n\
    - Emit one fact per list item.";

/// Duplicate-suppression contract, kept behavioural (no example predicates)
/// so it cannot drift from the taxonomy.
const DEDUPLICATION_RULES: &str = "### Deduplication\n\
    Before emitting a fact, ask yourself: 'Have I already emitted a fact \
    with the same subject and the same meaning?' If yes, do not emit the \
    duplicate — emit only the fact that carries genuinely new information.";

/// Output contract for every extraction prompt.
const OUTPUT_CONTRACT: &str = "### Output\n\
    Emit ONLY via the 'remember' tool. Do not output free text.";

/// Render the DB-derived predicate standards section.
///
/// The closed taxonomy in `relationship_types` is the single source of truth:
/// the prompt must never carry a second hand-maintained copy of the
/// vocabulary, because drift between prompt and tool schema silently changes
/// what the LLM is told to emit (issue #598).
fn build_predicate_standards(predicates: &[EmitEligiblePredicate]) -> String {
    let mut standards = String::from(
        "### Predicate standards (critical)\n\
         Use ONLY the controlled predicates below for relationship_type. \
         Do NOT invent synonyms — an unrecognised predicate is staged for \
         review and never inserted.\n",
    );
    let mut current_root: Option<&str> = None;
    for predicate in predicates {
        if current_root != Some(predicate.root_name.as_str()) {
            current_root = Some(&predicate.root_name);
            let _ = writeln!(standards, "\n- {}", capitalise(&predicate.root_name));
        }
        if predicate.guidance.is_empty() {
            let _ = writeln!(standards, "  * {}", predicate.name);
        } else {
            let _ = writeln!(standards, "  * {} — {}", predicate.name, predicate.guidance);
        }
    }
    standards
}

/// Uppercase the first character of a taxonomy label; an empty label renders
/// as `General` so orphaned leaves still group under a heading.
fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::from("General"),
    }
}

/// Assemble the base prompt from its readable sections.
fn render_base_prompt(guide: &str, predicates: &[EmitEligiblePredicate]) -> String {
    format!(
        "{EXTRACTION_ROLE}\n\n{EXTRACTION_RULES}\n{guide}\n{}{DEDUPLICATION_RULES}\n{OUTPUT_CONTRACT}",
        build_predicate_standards(predicates)
    )
}

/// Build the KG-focused extraction rules, category guide, and output contract.
///
/// Every section is rendered from code or the database; only the taxonomy
/// leaves (with their DB descriptions) drive the predicate standards, keeping
/// the prompt in lockstep with the `remember` tool schema.
///
/// Shared by the simple [`extract_facts`] path (no contextual inputs) and the
/// rich [`build_extraction_prompt`] (which layers the core-facts block and
/// recent conversation on top).
pub(super) async fn build_base_prompt(kg: &KnowledgeGraph) -> Result<String, KnowledgeError> {
    let guide = build_category_guide(kg).await?;
    let predicates = kg.list_emit_eligible_relationship_types().await?;

    Ok(render_base_prompt(&guide, &predicates))
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
