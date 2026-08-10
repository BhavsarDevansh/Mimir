use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Category delegates
    // ------------------------------------------------------------------

    pub async fn list_categories(
        &self,
        parent_id: Option<i32>,
    ) -> Result<Vec<models::category::Category>, KnowledgeError> {
        queries::category::list_categories(&self.pool, parent_id).await
    }

    pub async fn get_category(
        &self,
        id: i32,
    ) -> Result<Option<models::category::CategoryWithCount>, KnowledgeError> {
        queries::category::get_category(&self.pool, id).await
    }

    pub async fn get_category_children(
        &self,
        parent_id: i32,
    ) -> Result<Vec<models::category::Category>, KnowledgeError> {
        queries::category::get_children(&self.pool, parent_id).await
    }

    pub async fn insert_category(
        &self,
        new_category: models::category::NewCategory,
    ) -> Result<models::category::Category, KnowledgeError> {
        queries::category::insert_category(&self.pool, &new_category, self.now()).await
    }

    pub async fn delete_category(&self, id: i32) -> Result<(), KnowledgeError> {
        queries::category::delete_category(&self.pool, id).await
    }

    pub async fn get_categories_for_fact(
        &self,
        fact_id: i32,
    ) -> Result<Vec<models::category::Category>, KnowledgeError> {
        queries::category::get_categories_for_fact(&self.pool, fact_id).await
    }

    pub async fn get_facts_in_category(
        &self,
        category_id: i32,
        limit: i64,
    ) -> Result<Vec<models::category::FactWithCategories>, KnowledgeError> {
        queries::category::get_facts_in_category(&self.pool, category_id, limit).await
    }

    pub async fn get_facts_matching_all_categories(
        &self,
        category_ids: &[i32],
        limit: i64,
    ) -> Result<Vec<models::category::FactWithCategories>, KnowledgeError> {
        queries::category::get_facts_matching_all_categories(&self.pool, category_ids, limit).await
    }

    pub async fn get_facts_matching_any_categories(
        &self,
        category_ids: &[i32],
        limit: i64,
    ) -> Result<Vec<models::category::FactWithCategories>, KnowledgeError> {
        queries::category::get_facts_matching_any_categories(&self.pool, category_ids, limit).await
    }

    pub async fn get_top_level_catalogue(
        &self,
    ) -> Result<Vec<models::category::Category>, KnowledgeError> {
        queries::category::list_categories(&self.pool, None).await
    }

    /// Resolve a natural-language category alias to a category id.
    pub async fn resolve_category_alias(&self, alias: &str) -> Result<Option<i32>, KnowledgeError> {
        queries::category::resolve_category_alias(&self.pool, alias).await
    }

    /// List category aliases, optionally filtered by category id.
    pub async fn list_category_aliases(
        &self,
        category_id: Option<i32>,
    ) -> Result<Vec<models::category::CategoryAlias>, KnowledgeError> {
        queries::category::list_category_aliases(&self.pool, category_id).await
    }

    /// Insert a category alias. Idempotent for the same alias→category mapping;
    /// rejects empty aliases, unknown category ids, and rebinding an existing
    /// alias to a different category.
    pub async fn insert_category_alias(
        &self,
        alias: &str,
        category_id: i32,
    ) -> Result<(), KnowledgeError> {
        queries::category::insert_category_alias(&self.pool, alias, category_id).await
    }

    /// Return all descendant category ids of `root_id` (exclusive of root).
    pub async fn get_descendant_category_ids(
        &self,
        root_id: i32,
    ) -> Result<Vec<i32>, KnowledgeError> {
        queries::category::get_descendant_category_ids(&self.pool, root_id).await
    }

    /// Get facts anywhere in a category subtree (root + all descendants).
    pub async fn get_facts_in_category_subtree(
        &self,
        root_id: i32,
        limit: i64,
    ) -> Result<Vec<models::category::FactWithCategories>, KnowledgeError> {
        queries::category::get_facts_in_category_subtree(&self.pool, root_id, limit).await
    }
}
