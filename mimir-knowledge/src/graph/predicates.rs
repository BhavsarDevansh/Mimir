use crate::graph::KnowledgeGraph;
use crate::*;

/// Canonical leaf names the extraction schemas expose. The database remains
/// the resolver's source of truth; this const pins the current seed and is
/// used only by tests and deterministic connector registration checks.
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
    "prefers",
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
    "health_condition",
    "has_name",
    "studied",
    "completed_degree",
    "educational_status",
    "job_title",
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
/// the emit surface. The email LLM layer now validates against the DB-backed
/// extraction schema and re-checks that same list in Rust.
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
/// drift apart. Every entry is a
/// canonical predicate (pinned by `multi_valued_predicates_are_canonical` in
/// `mimir-knowledge/tests/predicate_allowlist_test.rs`).
pub const MULTI_VALUED_PREDICATES: &[&str] = &[
    "has_event",
    "prefers",
    "hobby",
    "dislikes",
    "skill",
    "has_pets",
    "has_child",
    "has_parent",
    "has_sibling",
    "has_partner",
];

/// Whether a predicate name is part of the canonical seed pinned by tests.
/// Runtime extraction resolves through the DB-backed taxonomy; aliases are the
/// database source of truth and may map legacy names onto a controlled leaf.
///
/// Deterministic connector tests use this list to pin their registration
/// surface. LLM extraction validates against the DB-backed schema and Rust
/// re-checks the same list.
pub fn is_canonical_predicate_name(name: &str) -> bool {
    let Some(normalized) = normalize_alias(name) else {
        return false;
    };
    CANONICAL_PREDICATES.contains(&normalized.as_str())
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
    /// This is the strict conversational extraction resolver. Resolution order:
    /// 1. Normalize the incoming name.
    /// 2. Query `relationship_type_aliases` for the normalized name; on a hit
    ///    the target row must be emit-eligible in the closed taxonomy.
    /// 3. Any other name is rejected with a clear error; no row is created.
    pub async fn resolve_canonical_relationship_type(
        &self,
        name: &str,
    ) -> Result<i16, KnowledgeError> {
        self.resolve_emit_eligible_relationship_type(name)
            .await?
            .ok_or_else(|| {
                KnowledgeError::Validation(format!(
                    "predicate '{name}' is not an emit-eligible taxonomy leaf; refusing to auto-create."
                ))
            })
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

    /// Resolve only an emit-eligible controlled relationship leaf.
    ///
    /// This is the strict ingestion resolver: it resolves seeded canonical
    /// names/aliases, but refuses query-only taxonomy nodes and never creates
    /// a new row.
    pub async fn resolve_emit_eligible_relationship_type(
        &self,
        name: &str,
    ) -> Result<Option<i16>, KnowledgeError> {
        let Some(id) = self.get_relationship_type_id(name).await? else {
            return Ok(None);
        };

        let row: Option<(bool,)> =
            sqlx::query_as("SELECT emit_eligible FROM relationship_types WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((true,)) => Ok(Some(id)),
            _ => Ok(None),
        }
    }

    /// Resolve an emit-eligible leaf or return the shared strict-ingestion
    /// validation error.
    pub(crate) async fn require_emit_eligible_relationship_type(
        &self,
        name: &str,
    ) -> Result<i16, KnowledgeError> {
        self.resolve_emit_eligible_relationship_type(name)
            .await?
            .ok_or_else(|| {
                KnowledgeError::Validation(format!(
                    "predicate '{name}' is not an emit-eligible taxonomy leaf"
                ))
            })
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
