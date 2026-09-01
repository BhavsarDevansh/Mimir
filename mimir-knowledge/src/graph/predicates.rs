use crate::graph::KnowledgeGraph;
use crate::*;

/// Canonical relationship-type names the conversational extraction path
/// accepts (issue #401). Seeded by migrations 013/023/025/031/036/037, 050,
/// 051 and 053; the prompt-instructed `favourite_<thing>` family is an open
/// set handled separately by
/// [`KnowledgeGraph::resolve_canonical_relationship_type`].
///
/// Kept in sync with the seed by
/// `canonical_const_matches_seeded_relationship_types` in
/// `mimir-knowledge/tests/predicate_allowlist_test.rs`.
///
/// Migration 051 (issue #403) consolidated redundant verbs: `based_in` and
/// `lived_in` are aliases of `resides_in`, and `is_in` is an alias of
/// `located_in`. The abstract DAG parents seeded by 051 (`residence`,
/// `employment`, `education`, `containment`) are deliberately NOT in this
/// list — they are query-only subtree roots and must never be used as fact
/// predicates.
pub const CANONICAL_PREDICATES: &[&str] = &[
    // Migrations 013/023/025/031 (ids 2-12; id 1 `is_in` was consolidated
    // into `located_in` by migration 051).
    "visited",
    "owns",
    "works_as",
    "has_partner",
    "has_parent",
    "born_on",
    "died_on",
    "located_in",
    "created_on",
    "has_preference",
    "rejected_action",
    // Migrations 036/037 (ids 13-31).
    "studied_at",
    "hobby",
    "works_at",
    "resides_in",
    "has_pets",
    "has_sibling",
    "has_child",
    "preferred_name",
    "favourite_food",
    "favourite_colour",
    "health_condition",
    "has_name",
    "studied",
    "completed_degree",
    "educational_status",
    "job_title",
    "likes",
    "dislikes",
    // Migration 050.
    "skill",
    "has_appointment",
    "allergy",
    "medication",
    "diagnosis",
    "income",
    "salary",
    "password",
    "ssn",
    "social_security_number",
    "bank_account",
    "credit_card",
    "insurance",
    // Migration 053 (connector-emitted predicates, issue #412). Seeded so the
    // connector path never auto-creates a runtime row; also usable by the
    // conversational path now that they are canonical vocabulary.
    "has_event",
    "attending",
    "took_photo_at",
    "took_photo",
    "has_flight",
    "departs_from",
    "arrives_at",
    "operated_by",
    "has_booking",
    "has_order",
    "purchased_from",
    "has_delivery",
    "shipped_by",
    "delivered_to",
    "has_ticket",
    "issued_by",
];

/// Relationship-type names the connectors emit deterministically (issue
/// #412). Pinned in both directions:
/// `connector_emitted_predicates_are_seeded_canonical` in
/// `mimir-knowledge/tests/predicate_allowlist_test.rs` pins every entry to a
/// seeded canonical row (migration 053), and the `mimir-connectors`
/// registration tests assert every extractor-emitted predicate passes
/// [`is_canonical_predicate_name`] — canonical vocabulary, a superset of this
/// list, since `visited` (photos coords fallback, issue #250), `located_in`
/// (iCal + JSON-LD) and `has_appointment` (email LLM) are canonical since
/// migrations 013/050 and deliberately not listed here. A new connector
/// predicate must therefore be seeded canonical before the connector tests
/// pass; adding it here additionally pins the seed/const pair and documents
/// the emit surface. The email LLM layer validates its emitted predicates
/// against [`is_canonical_predicate_name`], so even the open LLM surface
/// cannot auto-create rows outside the prompt-instructed
/// `favourite_<thing>` family, which is canonical by design.
pub const CONNECTOR_EMITTED_PREDICATES: &[&str] = &[
    // Calendar / Email iMIP (mimir-connectors/src/ical/facts.rs) and JSON-LD
    // EventReservation (mimir-connectors/src/email/jsonld/reservations.rs).
    "has_event",
    "attending",
    // Photos (mimir-connectors/src/photos/scan.rs). `visited` is also
    // emitted (the coords-only fallback, issue #250) but has been canonical
    // since migration 013, so it is not listed here.
    "took_photo_at",
    "took_photo",
    // Email JSON-LD reservations (mimir-connectors/src/email/jsonld/).
    "has_flight",
    "departs_from",
    "arrives_at",
    "operated_by",
    "has_booking",
    "has_order",
    "purchased_from",
    "has_delivery",
    "shipped_by",
    "delivered_to",
    "has_ticket",
    "issued_by",
    // `located_in` (iCal + JSON-LD) and `has_appointment` (email LLM) are
    // also connector-emitted but were seeded canonically earlier (migrations
    // 013/050), so they are not listed here.
];

/// Predicates that represent a collection of independent values, so a
/// comma-separated object literal means multiple facts and distinct objects
/// coexist instead of superseding one another.
///
/// Single source of truth shared by the extraction list splitter
/// (`split_list_objects` in `extract/parse.rs`) and the insert overlap logic
/// (`insert_fact_in_tx` in `queries/fact/insert.rs`), so the two can never
/// drift apart. The open `favourite_<thing>` family is multi-valued through
/// the same shared `is_favourite_family_predicate` helper. Every entry is a
/// canonical predicate (pinned by `multi_valued_predicates_are_canonical` in
/// `mimir-knowledge/tests/predicate_allowlist_test.rs`).
pub const MULTI_VALUED_PREDICATES: &[&str] = &[
    "hobby",
    "likes",
    "dislikes",
    "favourite_colour",
    "favourite_food",
    "skill",
    "has_pets",
    "has_child",
    "has_parent",
    "has_sibling",
    "has_partner",
];

/// Prefix of the prompt-instructed `favourite_<thing>` predicate family.
const FAVOURITE_PREDICATE_PREFIX: &str = "favourite_";

/// Whether a predicate name belongs to the open `favourite_<thing>` family
/// (a non-empty thing after the prefix). Shared by the strict resolver and
/// the list splitter so the family's shape is defined in one place.
pub(crate) fn is_favourite_family_predicate(name: &str) -> bool {
    name.strip_prefix(FAVOURITE_PREDICATE_PREFIX)
        .is_some_and(|thing| !thing.is_empty())
}

/// Whether a predicate name is part of the canonical vocabulary the strict
/// resolver accepts — the seeded [`CANONICAL_PREDICATES`] allow-list or the
/// open `favourite_<thing>` family — after alias normalisation. Pure Rust
/// check, no database access.
///
/// Used by the email LLM connector layer (issue #412) so an LLM-emitted
/// predicate can never auto-create a `relationship_types` row on first sync:
/// the same [`CANONICAL_PREDICATES`] const the conversational boundary's
/// strict resolver checks (plus the open `favourite_<thing>` family).
pub fn is_canonical_predicate_name(name: &str) -> bool {
    let Some(normalized) = normalize_alias(name) else {
        return false;
    };
    CANONICAL_PREDICATES.contains(&normalized.as_str())
        || is_favourite_family_predicate(&normalized)
}

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Predicate registry
    // ------------------------------------------------------------------

    /// Look up a relationship type by name without creating it.
    /// Returns `None` if the type does not exist.
    pub async fn relationship_type_id(&self, name: &str) -> Option<i16> {
        match self.get_relationship_type_id(name).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("relationship_type_id lookup failed for '{}': {}", name, e);
                None
            }
        }
    }

    /// Resolve a relationship-type name to its canonical id, rejecting
    /// predicates outside the canonical allow-list instead of auto-creating
    /// rows (issue #401).
    ///
    /// This is the strict counterpart to [`Self::ensure_relationship_type`]
    /// used at the conversational extraction boundary. Resolution order:
    /// 1. Normalize the incoming name.
    /// 2. The prompt-instructed `favourite_<thing>` family is accepted and
    ///    resolved via [`Self::ensure_relationship_type`] (auto-creating the
    ///    specific favourite on first use); a bare `favourite_` with no thing
    ///    is rejected like any other unknown predicate.
    /// 3. Query `relationship_type_aliases` for the normalized name; on a hit
    ///    the canonical name must be in [`CANONICAL_PREDICATES`] — a type that
    ///    was auto-created at runtime (e.g. a connector-emitted predicate) is
    ///    rejected.
    /// 4. Any other name is rejected with a clear error; no row is created.
    pub async fn resolve_canonical_relationship_type(
        &self,
        name: &str,
    ) -> Result<i16, KnowledgeError> {
        let Some(normalized) = normalize_alias(name) else {
            return Err(KnowledgeError::Validation(
                "relationship type name cannot be empty".to_string(),
            ));
        };

        if is_favourite_family_predicate(&normalized) {
            return self.ensure_relationship_type(&normalized).await;
        }

        let Some(id) = self.resolve_relationship_type_alias(&normalized).await? else {
            return Err(KnowledgeError::Validation(format!(
                "predicate '{name}' is not a canonical relationship type; refusing to auto-create. Use a predicate from the extraction prompt's predicate standards or a registered alias."
            )));
        };

        let canonical = self.relationship_type_name(id).await;
        if canonical
            .as_deref()
            .is_some_and(|c| CANONICAL_PREDICATES.contains(&c))
        {
            return Ok(id);
        }

        Err(KnowledgeError::Validation(format!(
            "predicate '{name}' resolves to an auto-created relationship type, not a canonical predicate; refusing to insert. Use a predicate from the extraction prompt's predicate standards or a registered alias."
        )))
    }

    /// Ensure a relationship type exists in the database, returning its stable id.
    /// Creates the row silently if missing.
    ///
    /// Resolution order:
    /// 1. Normalize the incoming name.
    /// 2. Query `relationship_type_aliases` for the normalized name; return the
    ///    canonical id on hit.
    /// 3. Fall back to creating a new canonical type and register the normalized
    ///    name as its own alias.
    pub async fn ensure_relationship_type(&self, name: &str) -> Result<i16, KnowledgeError> {
        let mut tx = self.pool.begin().await?;
        let id = self.ensure_relationship_type_in_tx(&mut tx, name).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Same as [`Self::ensure_relationship_type`] but operates inside an existing transaction.
    pub(crate) async fn ensure_relationship_type_in_tx(
        &self,
        tx: &mut sqlx::SqliteTransaction<'_>,
        name: &str,
    ) -> Result<i16, KnowledgeError> {
        let Some(normalized) = normalize_alias(name) else {
            return Err(KnowledgeError::Validation(
                "relationship type name cannot be empty".to_string(),
            ));
        };

        // 1. In-memory cache.
        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(&id) = cache.alias_to_id.get(&normalized) {
                return Ok(id);
            }
        }

        // 2. Alias table is the single source of truth.
        let row: Option<(i16,)> = sqlx::query_as(
            "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
        )
        .bind(&normalized)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some((id,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.alias_to_id.insert(normalized.clone(), id);
            cache.name_to_id.insert(normalized, id);
            return Ok(id);
        }

        // 3. Alias miss: create new canonical type, then register self-alias.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO relationship_types (name, description) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET name = relationship_types.name RETURNING id",
        )
        .bind(&normalized)
        .bind(format!("Auto-created relationship_type: {}", normalized))
        .fetch_one(&mut **tx)
        .await?;
        let id = id as i16;

        // Use INSERT OR IGNORE because concurrent transactions may race to create
        // the same new canonical type; both can upsert `relationship_types`, but
        // only one can insert the self-alias. The loser must commit cleanly.
        sqlx::query(
            "INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id) VALUES (?, ?)",
        )
        .bind(&normalized)
        .bind(id)
        .execute(&mut **tx)
        .await?;

        let mut cache = self.relationship_type_cache.write().await;
        cache.name_to_id.insert(normalized.clone(), id);
        cache.alias_to_id.insert(normalized, id);
        Ok(id)
    }

    /// Resolve the fact memory priority for a relationship type, caching the
    /// constant result for subsequent inserts.
    pub(crate) async fn default_memory_priority_id_in_tx(
        &self,
        tx: &mut sqlx::SqliteTransaction<'_>,
        relationship_type_id: i16,
    ) -> Result<i16, KnowledgeError> {
        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(&priority_id) = cache.default_memory_priority_id.get(&relationship_type_id)
            {
                return Ok(priority_id);
            }
        }

        let priority_id =
            crate::queries::fact::memory_priority_id_in_tx(&mut **tx, relationship_type_id).await?;

        let mut cache = self.relationship_type_cache.write().await;
        cache
            .default_memory_priority_id
            .insert(relationship_type_id, priority_id);
        Ok(priority_id)
    }

    /// Look up a relationship type id by name without creating it.
    ///
    /// The alias table is the single source of truth: aliases resolve to their
    /// canonical relationship type id, and every canonical name is also a
    /// self-alias.
    pub async fn get_relationship_type_id(
        &self,
        name: &str,
    ) -> Result<Option<i16>, KnowledgeError> {
        let Some(normalized) = normalize_alias(name) else {
            return Ok(None);
        };

        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(&id) = cache.alias_to_id.get(&normalized) {
                return Ok(Some(id));
            }
        }

        let row: Option<(i16,)> = sqlx::query_as(
            "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
        )
        .bind(&normalized)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.alias_to_id.insert(normalized, id);
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Reverse lookup: get the relationship_type name for a given id.
    pub async fn relationship_type_name(&self, id: i16) -> Option<String> {
        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(name) = cache.id_to_name.get(&id) {
                return Some(name.clone());
            }
        }

        let row: Option<(String,)> =
            match sqlx::query_as("SELECT name FROM relationship_types WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("relationship_type_name lookup failed for id {}: {}", id, e);
                    return None;
                }
            };

        if let Some((ref name,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.name_to_id.insert(name.clone(), id);
            cache.id_to_name.insert(id, name.clone());
        }

        row.map(|r| r.0)
    }
}
