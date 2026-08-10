use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Events & reminders delegates (issue #74)
    // ------------------------------------------------------------------

    /// Insert an event overlay on a fact.
    pub async fn insert_event(
        &self,
        new: models::event::NewEvent,
    ) -> Result<models::event::Event, KnowledgeError> {
        queries::event::insert_event(&self.pool, &new).await
    }

    /// Insert an event overlay on a fact only if none exists yet.
    ///
    /// Returns `Some` when a new overlay was created, `None` when one already
    /// existed for the fact (idempotent). Used by the derive scan and the
    /// sensitive-fact confirmation path to avoid duplicate-overlay races.
    pub async fn insert_event_if_absent(
        &self,
        new: models::event::NewEvent,
    ) -> Result<Option<models::event::Event>, KnowledgeError> {
        queries::event::insert_event_if_absent(&self.pool, &new).await
    }

    /// Fetch an event overlay by its underlying fact id.
    pub async fn get_event_by_fact(
        &self,
        fact_id: i32,
    ) -> Result<Option<models::event::Event>, KnowledgeError> {
        queries::event::get_by_fact(&self.pool, fact_id).await
    }

    /// Transition an event overlay to a new lifecycle status.
    pub async fn update_event_status(
        &self,
        event_id: i32,
        status: models::enums::EventStatus,
    ) -> Result<models::event::Event, KnowledgeError> {
        queries::event::update_status(&self.pool, event_id, status, self.now()).await
    }

    /// Soft-delete an event overlay (mark `Dismissed`).
    pub async fn dismiss_event(
        &self,
        event_id: i32,
    ) -> Result<models::event::Event, KnowledgeError> {
        queries::event::soft_delete(&self.pool, event_id, self.now()).await
    }

    /// Active events for an entity within a `[from, to]` trigger-date window.
    pub async fn get_active_events(
        &self,
        entity_id: i32,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<models::event::Event>, KnowledgeError> {
        queries::event::get_active_events(&self.pool, entity_id, from, to).await
    }

    /// Active events for an entity that are past their trigger date.
    pub async fn get_overdue_events(
        &self,
        entity_id: i32,
    ) -> Result<Vec<models::event::Event>, KnowledgeError> {
        queries::event::get_overdue_events(&self.pool, entity_id, self.now()).await
    }

    /// Run the `events.upcoming_scan` job (derive + auto-complete + advance).
    pub async fn run_events_scan(
        &self,
        horizon_days: i64,
    ) -> Result<events::ScanSummary, KnowledgeError> {
        events::run_upcoming_scan(self, horizon_days).await
    }
}
