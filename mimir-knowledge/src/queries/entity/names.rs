//! Bulk entity-name lookups.

use sqlx::SqlitePool;

use crate::KnowledgeError;

pub async fn get_entity_names(
    pool: &SqlitePool,
    ids: &[u32],
) -> Result<std::collections::HashMap<u32, String>, KnowledgeError> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT id, name FROM entities WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as::<_, (i32, String)>(sqlx::AssertSqlSafe(&*sql));
    for &id in ids {
        query = query.bind(id as i32);
    }
    let rows = query.fetch_all(pool).await?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for (id, name) in rows {
        map.insert(id as u32, name);
    }
    Ok(map)
}
