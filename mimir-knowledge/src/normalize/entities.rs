//! Entity resolution policy: exact-name / exact-alias / fuzzy-threshold gates.

use crate::models::entity::{Entity, EntityType};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

use queries::entity::{AliasSearchResult, MatchKind};
// ---------------------------------------------------------------------------

/// Minimum normalised score (0..1) for an FTS5 fuzzy match to resolve to an
/// existing entity instead of creating a new one. Exact-name and exact-alias
/// matches always resolve regardless of score; this gate only governs the
/// fuzzy branch. The bar is intentionally high so that weak token overlaps
/// fall through to create-new rather than silently merging into the wrong
/// entity.
const FUZZY_RESOLVE_THRESHOLD: f32 = 0.9;

/// Pick the entity to resolve to from a set of (same-type) search results,
/// applying the resolution policy: exact-name and exact-alias always resolve;
/// a fuzzy match resolves only at score ≥ [`FUZZY_RESOLVE_THRESHOLD`]; a
/// below-threshold fuzzy (and an empty result set) yield `None` so the caller
/// creates a new entity.
///
/// `results` must be sorted by score descending, as [`queries::entity::get_by_name`]
/// / [`queries::entity::get_by_name_typed`] guarantee. Because the sort is
/// stable and exact-name is pushed before fuzzy at equal score, an exact name
/// always wins over a fuzzy hit scored 1.0.
pub(crate) fn pick_resolution(results: &[AliasSearchResult]) -> Option<&Entity> {
    // Results are sorted by score descending: exact alias (1.1) > exact name
    // (1.0) ≥ fuzzy (≤ 1.0), with a stable sort keeping exact name ahead of a
    // 1.0 fuzzy. The first element is therefore always the best candidate, so
    // only it needs inspecting.
    let result = results.first()?;
    match result.match_kind {
        MatchKind::ExactName | MatchKind::ExactAlias => Some(&result.entity),
        MatchKind::Fuzzy => {
            if result.score >= FUZZY_RESOLVE_THRESHOLD {
                Some(&result.entity)
            } else {
                // Below the threshold there is no better same-type match, so
                // resolve to None and let the caller create a new entity.
                None
            }
        }
    }
}

/// Same resolution policy as [`resolve_entity`], additionally reporting
/// whether a new entity was created.
///
/// The Obsidian import planner (issue #62) uses this so dry-run and apply
/// modes share the exact conversational/connector resolution chain and the
/// entity-creation accounting cannot drift from what the pipeline does.
pub(crate) async fn resolve_or_create(
    kg: &KnowledgeGraph,
    name: &str,
    entity_type: EntityType,
) -> Result<(Entity, bool), KnowledgeError> {
    let results = queries::entity::get_by_name_typed(kg.pool(), name, entity_type).await?;
    if let Some(entity) = pick_resolution(&results) {
        return Ok((entity.clone(), false));
    }
    let entity = queries::entity::create_entity(kg.pool(), name, entity_type, &[]).await?;
    Ok((entity, true))
}

/// Resolve a name to an entity via the full chain — exact name → alias → FTS5
/// fuzzy (score ≥ [`FUZZY_RESOLVE_THRESHOLD`]) → create new — restricted to
/// entities of the requested type (Phase 3 F5 / issue #182). Shared by chat
/// extraction and connector ingestion.
///
/// Note: entity names are globally unique (case-insensitive) at the schema
/// level, so the type filter guards against cross-type *fuzzy* / token-overlap
/// merges; an identical name of a different type cannot coexist, and the
/// create-on-miss path will return the existing same-name entity regardless of
/// type. Alias creation is not auto-learned from fuzzy matches.
pub(super) async fn resolve_entity(
    kg: &KnowledgeGraph,
    name: &str,
    entity_type: EntityType,
) -> Result<Entity, KnowledgeError> {
    Ok(resolve_or_create(kg, name, entity_type).await?.0)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "entities_tests.rs"]
mod entities_tests;
