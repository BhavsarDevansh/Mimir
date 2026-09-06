//! User-identity fact seeding and accidental-duplicate auto-merge.

use super::warn_err;

const ACCIDENTAL_DUPLICATE_FACT_THRESHOLD: i64 = 2;

/// Log a warning for a failed operation and return `None`, or pass through the
/// success value.
///
/// Used for optional best-effort operations (tool registration, alias wiring,
pub async fn seed_identity_facts(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    name: &str,
    preferred: &str,
) -> Result<(), mimir_knowledge::KnowledgeError> {
    use mimir_knowledge::models::fact::FactStatus;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;

    // Resolve predicate IDs through the closed taxonomy. The seeds are part of
    // migrations, so this path cannot create runtime vocabulary.
    let has_name_id = kg.resolve_canonical_relationship_type("has_name").await?;
    let pref_name_id = kg
        .resolve_canonical_relationship_type("preferred_name")
        .await?;

    // Targeted existence checks: query only the two relevant predicates.
    let has_name_facts = kg
        .get_facts_by_subject_and_predicate(subject_id, has_name_id)
        .await?;
    let pref_name_facts = kg
        .get_facts_by_subject_and_predicate(subject_id, pref_name_id)
        .await?;

    let has_name = has_name_facts.iter().any(|f| {
        f.status() == Some(FactStatus::Active)
            && f.object_literal
                .as_deref()
                .map(|lit| lit.to_lowercase() == name.to_lowercase())
                .unwrap_or(false)
    });
    let has_preferred = pref_name_facts.iter().any(|f| {
        f.status() == Some(FactStatus::Active)
            && f.object_literal
                .as_deref()
                .map(|lit| lit.to_lowercase() == preferred.to_lowercase())
                .unwrap_or(false)
    });

    // Collect facts to insert and perform the writes atomically.
    // Insert identity facts *before* alias/auto-merge so the canonical entity
    // always has at least as many facts as any qualifying duplicate, ensuring
    // auto_merge_pair preserves subject_id as the survivor.
    let mut facts_to_insert: Vec<NewFact> = Vec::with_capacity(2);

    if !has_name && !name.is_empty() {
        let mut nf = NewFact::new(subject_id, "has_name");
        nf.object_literal = Some(name.to_string());
        nf.source_type = SourceType::System;
        nf.category_ids = vec![110];
        facts_to_insert.push(nf);
    }

    if !preferred.is_empty() && preferred.to_lowercase() != name.to_lowercase() && !has_preferred {
        let mut nf = NewFact::new(subject_id, "preferred_name");
        nf.object_literal = Some(preferred.to_string());
        nf.source_type = SourceType::System;
        nf.category_ids = vec![110];
        facts_to_insert.push(nf);
    }

    if !facts_to_insert.is_empty() {
        kg.insert_facts_batch(facts_to_insert).await?;
    }

    // Alias logic (idempotent; safe to run outside the insert tx).
    if !preferred.is_empty() && preferred.to_lowercase() != name.to_lowercase() {
        warn_err(
            kg.add_alias(subject_id, preferred).await,
            &format!("Failed to add preferred-name alias '{preferred}'"),
        );

        auto_merge_accidental_duplicates(kg, subject_id, preferred).await;
    }

    Ok(())
}

/// Merge bare-name duplicate entities that look accidental (very few facts).
///
/// A threshold of 2 was chosen because a legitimate entity should have at least
/// a name fact and a preferred-name fact; anything less suggests an accidental
/// duplicate created before the alias was wired.
async fn auto_merge_accidental_duplicates(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    preferred: &str,
) {
    let candidates = warn_err(
        mimir_knowledge::queries::entity::get_by_name(kg.pool(), preferred).await,
        &format!("Failed to look up duplicates of '{preferred}'"),
    )
    .unwrap_or_default();

    for cand in candidates {
        try_merge_accidental_duplicate(kg, subject_id, preferred, cand).await;
    }
}

/// Evaluate a single candidate entity and merge it into `subject_id` if it looks
/// like an accidental duplicate (very few facts and same name).
async fn try_merge_accidental_duplicate(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    preferred: &str,
    cand: mimir_knowledge::queries::entity::AliasSearchResult,
) {
    if cand.entity.id == subject_id || cand.entity.name.to_lowercase() != preferred.to_lowercase() {
        return;
    }

    let fact_count = warn_err(
        kg.count_entity_facts(cand.entity.id).await,
        &format!(
            "Failed to count facts for candidate entity {} during auto-merge check",
            cand.entity.id
        ),
    )
    .unwrap_or(i64::MAX);

    if fact_count > ACCIDENTAL_DUPLICATE_FACT_THRESHOLD {
        return;
    }

    warn_err(
        mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), subject_id, cand.entity.id)
            .await,
        &format!(
            "Failed to auto-merge duplicate entity {} into {}",
            cand.entity.id, subject_id
        ),
    );
}
