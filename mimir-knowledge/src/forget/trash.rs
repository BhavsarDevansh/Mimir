//! Bulk forget machinery: matching, batching, backup, and trash writes.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::KnowledgeError;
use crate::models::audit_log::ChangedBy;

use super::cascade::{evaluate_children, forget_fact_inner, forget_fact_tx};
use super::{ForgetFilters, ForgetOptions, ForgetResult};

const TRASH_BATCH_SIZE: usize = 50;

/// Trash every fact in `ids` in transactional batches, then evaluate the
/// inferred children they leave behind.
///
/// Shared by [`forget_facts`], [`forget_all`], and
/// [`forget_facts_for_connector`] so the batching, transaction boundary, and
/// child-deduplication rules have one definition.
async fn trash_ids_in_batches(
    pool: &SqlitePool,
    ids: &[i32],
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    let mut all_children: Vec<(i32, bool)> = Vec::new();
    for chunk in ids.chunks(TRASH_BATCH_SIZE) {
        let mut tx = pool.begin().await?;
        for fact_id in chunk {
            all_children.extend(forget_fact_tx(&mut tx, *fact_id, changed_by, now).await?);
        }
        tx.commit().await?;
    }

    evaluate_collected_children(pool, all_children, now).await
}

/// Deduplicate the inferred children collected across trash batches and
/// evaluate the orphans they leave behind.
///
/// Shared by every trash path so the batching boundary never re-evaluates
/// (or double-trashes) a child reported by two parents in one operation.
async fn evaluate_collected_children(
    pool: &SqlitePool,
    children: Vec<(i32, bool)>,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    let mut seen = HashSet::new();
    let deduped: Vec<(i32, bool)> = children
        .into_iter()
        .filter(|(id, _)| seen.insert(*id))
        .collect();

    evaluate_children(pool, deduped, now).await
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Soft-delete a single fact (existing API preserved).
pub async fn forget_fact(
    pool: &SqlitePool,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    forget_fact_inner(pool, fact_id, changed_by, now).await
}

/// Bulk forget dispatch.
pub async fn forget_facts(
    pool: &SqlitePool,
    filters: ForgetFilters,
    opts: ForgetOptions,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    if filters.is_full_reset() {
        return forget_all(pool, opts, changed_by, now).await;
    }

    let ids = query_matching_fact_ids(pool, &filters).await?;
    if ids.is_empty() {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    let count = ids.len() as u64;

    if count > 100 && !opts.yes {
        return Err(KnowledgeError::Validation(format!(
            "Refusing to forget {} facts. Use --yes to confirm.",
            count
        )));
    }

    let sensitive = has_sensitive_match(pool, &filters).await?;
    if sensitive && !opts.confirm_sensitive {
        return Err(KnowledgeError::Validation(
            "This includes sensitive facts. Use --confirm-sensitive.".to_string(),
        ));
    }

    trash_ids_in_batches(pool, &ids, changed_by, now).await?;

    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: None,
    })
}

/// Soft-delete (trash) every fact sourced from a single connector instance
/// (Phase 3 A2 / #203).
///
/// The connector `forget` cascade: selects every fact id whose `sources`
/// row carries `connector_instance_id = instance_id`, then trashes each via
/// `forget_fact_tx` — the same trash machinery as [`forget_facts`] — so the
/// facts are recoverable from trash (30-day expiry) rather than hard-deleted.
/// Unlike the generic [`forget_facts`], no `--yes` / `--confirm-sensitive`
/// gate applies: a connector `forget` is an explicit admin action that
/// removes *all* of the connector's facts, sensitive or not. Inferred child
/// facts are evaluated via `evaluate_children` as usual. A fact sourced
/// from *both* the connector and an independent source (e.g. a chat turn) is
/// trashed wholesale — the connector source is the trigger and the fact is
/// recoverable from trash — so the cascade does not preserve facts that a
/// connector corroborated.
///
/// The caller (the server route) deletes the connector row and its stored
/// secret separately after this returns.
pub async fn forget_facts_for_connector(
    pool: &SqlitePool,
    instance_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    let ids: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT so.fact_id FROM sources so WHERE so.connector_instance_id = ?",
    )
    .bind(instance_id)
    .fetch_all(pool)
    .await?;

    let count = ids.len() as u64;
    if count == 0 {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    trash_ids_in_batches(pool, &ids, changed_by, now).await?;

    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: None,
    })
}

/// Soft-delete (trash) the facts of one connector instance whose
/// `sources.raw_reference` is in `raw_references` (issue #247).
///
/// The server-side-deletion (tombstone) path: a connector reports the set of
/// raw items its service removed since the last cycle, and every fact that
/// instance authored with one of those raw references is trashed via the
/// shared trash machinery (30-day recovery, inferred-child evaluation,
/// audit) — **unless the fact is still corroborated by another source**: only
/// the matching `sources` rows are removed, and the fact itself is trashed
/// only when no sources remain, so a tombstone from one connector instance
/// never deletes a fact another connector or a non-connector source still
/// supports (PR #313 review). Idempotent: a raw reference whose facts were
/// already trashed (or that never existed) simply contributes nothing —
/// mirroring the `delete_event` 404-is-success semantics. The
/// instance-scoped filter means a deletion can never touch another connector
/// instance's facts, even when two instances share a raw reference.
pub async fn forget_facts_for_connector_raw_references(
    pool: &SqlitePool,
    instance_id: i32,
    raw_references: &[String],
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    let mut seen = HashSet::<&str>::new();
    let refs: Vec<&str> = raw_references
        .iter()
        .map(String::as_str)
        .filter(|r| !r.is_empty() && seen.insert(*r))
        .collect();
    if refs.is_empty() {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT DISTINCT so.fact_id FROM sources so WHERE so.connector_instance_id = ",
    );
    builder.push_bind(instance_id);
    builder.push(" AND so.raw_reference IN (");
    let mut separated = builder.separated(", ");
    for r in &refs {
        separated.push_bind(r);
    }
    separated.push_unseparated(")");
    let ids: Vec<i32> = builder.build_query_scalar().fetch_all(pool).await?;

    if ids.is_empty() {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    // Remove only the matching `sources` rows, then trash a fact only when no
    // sources remain — both inside the same transaction — so the
    // preserve-or-trash decision is atomic with the source removal.
    let mut all_children: Vec<(i32, bool)> = Vec::new();
    let mut trashed: u64 = 0;
    for chunk in ids.chunks(TRASH_BATCH_SIZE) {
        let mut tx = pool.begin().await?;
        for fact_id in chunk {
            let mut delete =
                sqlx::QueryBuilder::<sqlx::Sqlite>::new("DELETE FROM sources WHERE fact_id = ");
            delete.push_bind(*fact_id);
            delete.push(" AND connector_instance_id = ");
            delete.push_bind(instance_id);
            delete.push(" AND raw_reference IN (");
            let mut separated = delete.separated(", ");
            for r in &refs {
                separated.push_bind(r);
            }
            separated.push_unseparated(")");
            let removed = delete.build().execute(&mut *tx).await?;
            if removed.rows_affected() == 0 {
                continue;
            }

            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE fact_id = ?")
                    .bind(fact_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if remaining > 0 {
                // The fact is still corroborated by another source: keep the
                // fact, only the tombstoned source row is gone.
                continue;
            }

            all_children.extend(forget_fact_tx(&mut tx, *fact_id, changed_by, now).await?);
            trashed += 1;
        }
        tx.commit().await?;
    }

    evaluate_collected_children(pool, all_children, now).await?;
    Ok(ForgetResult {
        forgotten_count: trashed,
        backup_path: None,
    })
}

/// Hard-delete all facts after creating a backup.
async fn forget_all(
    pool: &SqlitePool,
    opts: ForgetOptions,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    if opts.confirmation_phrase.as_deref() != Some("DELETE EVERYTHING") {
        return Err(KnowledgeError::Validation(
            "Full reset requires typing 'DELETE EVERYTHING'.".to_string(),
        ));
    }

    let backup_path = create_backup(pool).await?;

    if opts.archive {
        let ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM facts")
            .fetch_all(pool)
            .await?;
        let count = ids.len() as u64;
        trash_ids_in_batches(pool, &ids, changed_by, now).await?;
        return Ok(ForgetResult {
            forgotten_count: count,
            backup_path: Some(backup_path),
        });
    }

    let count = hard_delete_all_facts(pool).await?;
    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: Some(backup_path),
    })
}

/// Create a timestamped backup of the database.
async fn create_backup(pool: &SqlitePool) -> Result<PathBuf, KnowledgeError> {
    let data_dir = mimir_core::paths::data_dir().map_err(|e| {
        KnowledgeError::Validation(format!("Could not resolve data directory: {}", e))
    })?;
    let backup_dir = data_dir.join("backups");
    tokio::fs::create_dir_all(&backup_dir).await?;

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let backup_path = backup_dir.join(format!("knowledge.db.bak-{}", timestamp));

    let path_str = backup_path.display().to_string().replace("'", "''");
    let query = format!("VACUUM INTO '{}'", path_str);
    sqlx::query(sqlx::AssertSqlSafe(query))
        .execute(pool)
        .await?;

    Ok(backup_path)
}

/// Hard-delete every fact, entity, preference, queue, and trash row.
async fn hard_delete_all_facts(pool: &SqlitePool) -> Result<u64, KnowledgeError> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM fact_dependencies")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sources").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM fact_audit_log")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preference_sources")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preference_contexts")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preferences")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_locations")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_aliases")
        .execute(&mut *tx)
        .await?;
    let delete_result = sqlx::query("DELETE FROM facts").execute(&mut *tx).await?;
    let count = delete_result.rows_affected();
    sqlx::query("DELETE FROM entities")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trash").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM dedup_queue")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_merge_queue")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(count)
}

// ---------------------------------------------------------------------------
// Core per-fact forget logic
// ---------------------------------------------------------------------------

/// Push the shared [`ForgetFilters`] WHERE clauses onto `builder`.
///
/// Both [`query_matching_fact_ids`] and [`has_sensitive_match`] build the
/// same `facts f JOIN entities s … LEFT JOIN relationship_types rt` base and
/// filter set; only the `SELECT` prefix, the `WHERE rt.sensitive` prefix, and
/// the `LIMIT 1` suffix differ. Keeping the clauses in one helper guarantees
/// the two queries cannot drift when a filter field is added.
fn push_forget_filters(builder: &mut sqlx::QueryBuilder<sqlx::Sqlite>, filters: &ForgetFilters) {
    if let Some(id) = filters.fact_id {
        builder.push(" AND f.id = ");
        builder.push_bind(id);
    }
    if let Some(ref pred) = filters.predicate {
        builder.push(" AND rt.name = ");
        builder.push_bind(pred);
    }
    if let Some(ref subj) = filters.subject {
        builder.push(" AND s.name = ");
        builder.push_bind(subj);
    }
    if let Some(ref ent) = filters.entity {
        builder.push(" AND (s.name = ");
        builder.push_bind(ent);
        builder.push(" OR o.name = ");
        builder.push_bind(ent);
        builder.push(")");
    }
    if let Some(ref src) = filters.source {
        builder.push(" AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_instance_id IN (SELECT id FROM connectors WHERE slug = ");
        builder.push_bind(src);
        builder.push(") OR so.source_type_id = (SELECT id FROM source_types WHERE name = ");
        builder.push_bind(src);
        builder.push("))");
    }
    if let Some(from) = filters.from {
        builder.push(" AND f.created_at >= ");
        builder.push_bind(from);
    }
    if let Some(to) = filters.to {
        builder.push(" AND f.created_at <= ");
        builder.push_bind(to);
    }
}

async fn query_matching_fact_ids(
    pool: &SqlitePool,
    filters: &ForgetFilters,
) -> Result<Vec<i32>, KnowledgeError> {
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT f.id FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE 1=1",
    );
    push_forget_filters(&mut builder, filters);

    let ids: Vec<i32> = builder.build_query_scalar::<i32>().fetch_all(pool).await?;
    Ok(ids)
}

async fn has_sensitive_match(
    pool: &SqlitePool,
    filters: &ForgetFilters,
) -> Result<bool, KnowledgeError> {
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT 1 FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE rt.sensitive = TRUE",
    );
    push_forget_filters(&mut builder, filters);
    builder.push(" LIMIT 1");

    let row = builder.build().fetch_optional(pool).await?;
    Ok(row.is_some())
}

pub async fn hard_delete_expired_trash(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<u64, KnowledgeError> {
    let result = sqlx::query("DELETE FROM trash WHERE expires_at < ? AND original_table = 'facts'")
        .bind(now)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KnowledgeGraph;
    use crate::models::connector::UpsertConnectorInput;
    use crate::models::entity::EntityType;
    use crate::models::enums::ConnectorType;
    use crate::models::fact::NewFact;
    use crate::models::source::{ExtractionMethod, SourceType};
    use chrono::Duration;
    use sqlx::Execute;

    fn all_filters() -> ForgetFilters {
        ForgetFilters {
            fact_id: Some(7),
            predicate: Some("visited".to_string()),
            subject: Some("Alice".to_string()),
            entity: Some("London".to_string()),
            source: Some("gmail".to_string()),
            from: Some(Utc::now() - Duration::days(30)),
            to: Some(Utc::now()),
            all: false,
        }
    }

    #[test]
    fn push_forget_filters_emits_identical_clauses_for_both_bases() {
        let mut ids_builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT f.id FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE 1=1",
        );
        push_forget_filters(&mut ids_builder, &all_filters());
        let ids_sql = ids_builder.build().sql();

        let mut sensitive_builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT 1 FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE rt.sensitive = TRUE",
        );
        push_forget_filters(&mut sensitive_builder, &all_filters());
        sensitive_builder.push(" LIMIT 1");
        let sensitive_sql = sensitive_builder.build().sql();

        let expected_clauses = " AND f.id = ? AND rt.name = ? AND s.name = ? AND (s.name = ? OR o.name = ?) AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_instance_id IN (SELECT id FROM connectors WHERE slug = ?) OR so.source_type_id = (SELECT id FROM source_types WHERE name = ?)) AND f.created_at >= ? AND f.created_at <= ?";
        assert_eq!(
            ids_sql,
            "SELECT f.id FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE 1=1".to_string() + expected_clauses
        );
        assert_eq!(
            sensitive_sql,
            "SELECT 1 FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE rt.sensitive = TRUE".to_string() + expected_clauses + " LIMIT 1"
        );
    }

    async fn insert_fact(
        kg: &KnowledgeGraph,
        subject_id: i32,
        relationship_type: &str,
        object_id: Option<i32>,
        object_literal: Option<String>,
        source_type: SourceType,
        connector_instance_id: Option<i32>,
    ) -> i32 {
        kg.insert_fact(NewFact {
            subject_id,
            relationship_type: relationship_type.to_string(),
            object_id,
            object_literal,
            valid_from: None,
            valid_until: None,
            source_type,
            connector_instance_id,
            connector_type: connector_instance_id.map(|_| ConnectorType::Calendar),
            raw_reference: connector_instance_id.map(|_| "evt-1".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn matching_and_sensitive_queries_agree_on_every_filter_field() {
        let dir = tempfile::tempdir().unwrap();
        let kg = KnowledgeGraph::init(&dir.path().join("forget_filter_test.db"))
            .await
            .unwrap();
        let pool = kg.pool();

        let alice = kg
            .create_entity("Alice", EntityType::Person, &[])
            .await
            .unwrap()
            .id;
        let london = kg
            .create_entity("London", EntityType::Place, &[])
            .await
            .unwrap()
            .id;
        let paris = kg
            .create_entity("Paris", EntityType::Place, &[])
            .await
            .unwrap()
            .id;

        let visited = insert_fact(
            &kg,
            alice,
            "visited",
            Some(london),
            None,
            SourceType::UserEdit,
            None,
        )
        .await;
        let allergy = insert_fact(
            &kg,
            alice,
            "allergy",
            None,
            Some("peanuts".to_string()),
            SourceType::UserEdit,
            None,
        )
        .await;
        let allergy_pred_id = kg
            .get_relationship_type_id("allergy")
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE relationship_types SET sensitive = TRUE WHERE id = ?")
            .bind(allergy_pred_id)
            .execute(pool)
            .await
            .unwrap();

        let instance = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Calendar,
                slug: "cal".to_string(),
                backend: "caldav".to_string(),
                display_name: "cal".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap()
            .id;
        let in_paris = insert_fact(
            &kg,
            alice,
            "is_in",
            Some(paris),
            None,
            SourceType::Connector,
            Some(instance),
        )
        .await;

        // Pin created_at to known RFC3339 timestamps so the from/to filters
        // are deterministic (sqlx binds DateTime<Utc> as RFC3339 text).
        let now = Utc::now();
        for (id, age_days) in [(visited, 10), (allergy, 5), (in_paris, 1)] {
            sqlx::query("UPDATE facts SET created_at = ? WHERE id = ?")
                .bind(now - Duration::days(age_days))
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
        }

        let cases: Vec<(ForgetFilters, Vec<i32>, bool)> = vec![
            (
                ForgetFilters {
                    fact_id: Some(visited),
                    ..Default::default()
                },
                vec![visited],
                false,
            ),
            (
                ForgetFilters {
                    predicate: Some("allergy".to_string()),
                    ..Default::default()
                },
                vec![allergy],
                true,
            ),
            (
                ForgetFilters {
                    subject: Some("Alice".to_string()),
                    ..Default::default()
                },
                vec![visited, allergy, in_paris],
                true,
            ),
            (
                ForgetFilters {
                    entity: Some("London".to_string()),
                    ..Default::default()
                },
                vec![visited],
                false,
            ),
            (
                ForgetFilters {
                    source: Some("UserEdit".to_string()),
                    ..Default::default()
                },
                vec![visited, allergy],
                true,
            ),
            (
                ForgetFilters {
                    source: Some("cal".to_string()),
                    ..Default::default()
                },
                vec![in_paris],
                false,
            ),
            (
                ForgetFilters {
                    from: Some(now - Duration::days(7)),
                    ..Default::default()
                },
                vec![allergy, in_paris],
                true,
            ),
            (
                ForgetFilters {
                    to: Some(now - Duration::days(7)),
                    ..Default::default()
                },
                vec![visited],
                false,
            ),
            (
                ForgetFilters {
                    from: Some(now - Duration::days(7)),
                    to: Some(now - Duration::days(2)),
                    ..Default::default()
                },
                vec![allergy],
                true,
            ),
        ];

        for (filters, mut expected_ids, expected_sensitive) in cases {
            let mut ids = query_matching_fact_ids(pool, &filters).await.unwrap();
            ids.sort_unstable();
            expected_ids.sort_unstable();
            assert_eq!(ids, expected_ids, "matching ids for {filters:?}");
            assert_eq!(
                has_sensitive_match(pool, &filters).await.unwrap(),
                expected_sensitive,
                "sensitive match for {filters:?}"
            );
        }
    }
}
