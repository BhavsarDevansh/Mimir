//! CalDAV connector end-to-end knowledge-graph integration tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{
    ActionResult, CalendarConnectorFactory, Connector, ConnectorAction, ConnectorError,
    ConnectorFactory, ConnectorMode, ConnectorRegistry, ConnectorSupervisor, FnConnectorFactory,
    HealthStatus, SupervisorConfig, SyncOptions, SyncOutcome, TriggerOutcome,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorStatus, ConnectorType, EventType,
};
use mimir_knowledge::normalize::NormalizedFact;

mod common;
use common::*;

#[tokio::test]
async fn calendar_sync_surfaces_upcoming_event_for_user() {
    let (kg, _dir) = init_kg().await;
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/personal/", server.uri());

    // Health probe (Online) — PROPFIND resourcetype for every cycle.
    Mock::given(wiremock::matchers::method("PROPFIND"))
        .and(wiremock::matchers::path("/cal/personal/"))
        .respond_with(
            ResponseTemplate::new(207).set_body_string(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
            ),
        )
        .mount(&server)
        .await;

    // A future-dated one-time event (now + 5 days) so it lands inside the
    // 30-day Upcoming horizon.
    let start = Utc::now() + ChronoDuration::days(5);
    let start_ical = start.format("%Y%m%dT%H%M%SZ").to_string();
    let ical = format!(
        "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\n\
UID:future-1@test\nSUMMARY:Conference\n\
DTSTART:{start_ical}\nLOCATION:London\nEND:VEVENT\nEND:VCALENDAR"
    );
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("upc-1", &[("/cal/future-1.ics", &ical)], &[]),
    )
    .await;

    let config = app_password_config(&cal_url);
    let _row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "calendar-personal".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: config.to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Calendar, "caldav", CalendarConnectorFactory)
        .unwrap();
    let (_shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx)
        .with_secret_store(store_with_app_password().await)
        .with_user_identity("Devansh");

    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the user entity + its `has_event` fact to land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let devansh = entity_id(&kg, "Devansh").await;
        if let Some(uid) = devansh {
            if !kg.get_facts_by_subject(uid, 100).await.unwrap().is_empty() {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "event fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let devansh = entity_id(&kg, "Devansh").await.expect("Devansh entity");
    let upcoming = kg.render_upcoming_section(devansh, 30, 10).await.unwrap();
    assert!(
        upcoming.contains("Conference"),
        "future event surfaces in Upcoming: {upcoming}"
    );

    // The event overlay is an Appointment (not a Reminder/Task).
    let fact = kg
        .get_facts_by_subject(devansh, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.object_literal.is_none())
        .expect("has_event fact");
    let event = kg
        .get_event_by_fact(fact.id)
        .await
        .unwrap()
        .expect("overlay");
    assert_eq!(event.event_type(), Some(EventType::Appointment));

    // Secondary facts (location/attendance) must not spawn their own
    // events-subsystem overlays — only the primary `has_event` fact drives
    // one (#198 review). The `located_in` fact (Event → Place) is the only
    // fact whose subject is the Conference event entity, so assert none of
    // its facts carry an overlay.
    let conference = entity_id(&kg, "Conference")
        .await
        .expect("Conference entity");
    for fact in kg.get_facts_by_subject(conference, 100).await.unwrap() {
        assert!(
            kg.get_event_by_fact(fact.id).await.unwrap().is_none(),
            "secondary fact {} must not spawn an event overlay",
            fact.id,
        );
    }

    supervisor.shutdown().await;
}

async fn entity_id(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    let results = kg.search_entities(name, 10).await.unwrap();
    results.into_iter().next().map(|r| r.entity.id)
}

/// Issue #247: a server-side deletion (CalDAV sync-collection tombstone)
/// must trash the corresponding connector-provenanced facts and stop the
/// event surfacing in "Upcoming".
#[tokio::test]
async fn calendar_server_side_deletion_trashes_facts_and_hides_upcoming_event() {
    let (kg, _dir) = init_kg().await;
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/personal/", server.uri());

    // Health probe (Online) — PROPFIND resourcetype for every cycle.
    Mock::given(wiremock::matchers::method("PROPFIND"))
        .and(wiremock::matchers::path("/cal/personal/"))
        .respond_with(
            ResponseTemplate::new(207).set_body_string(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
            ),
        )
        .mount(&server)
        .await;

    // Cycle 1: a future-dated one-time event (now + 5 days) so it lands
    // inside the 30-day Upcoming horizon.
    let start = Utc::now() + ChronoDuration::days(5);
    let start_ical = start.format("%Y%m%dT%H%M%SZ").to_string();
    let ical = format!(
        "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\n\
UID:future-1@test\nSUMMARY:Dentist\n\
DTSTART:{start_ical}\nLOCATION:London\nEND:VEVENT\nEND:VCALENDAR"
    );
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("tomb-1", &[("/cal/future-1.ics", &ical)], &[]),
    )
    .await;

    // Cycle 2: the same event is reported deleted (tombstone href only).
    mount_sync(
        &server,
        "/cal/personal/",
        Some("tomb-1"),
        sync_body("tomb-2", &[], &["/cal/future-1.ics"]),
    )
    .await;

    let config = app_password_config(&cal_url);
    let _row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "calendar-personal".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: config.to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Calendar, "caldav", CalendarConnectorFactory)
        .unwrap();
    let (_shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx)
        .with_secret_store(store_with_app_password().await)
        .with_user_identity("Devansh");

    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the user entity + its `has_event` fact to land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let devansh = loop {
        let devansh = entity_id(&kg, "Devansh").await;
        if let Some(uid) = devansh {
            if !kg.get_facts_by_subject(uid, 100).await.unwrap().is_empty() {
                break uid;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "event fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let upcoming = kg.render_upcoming_section(devansh, 30, 10).await.unwrap();
    assert!(
        upcoming.contains("Dentist"),
        "future event surfaces in Upcoming: {upcoming}"
    );
    let has_event_fact = kg
        .get_facts_by_subject(devansh, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.object_literal.is_none())
        .expect("has_event fact");
    let event = kg
        .get_event_by_fact(has_event_fact.id)
        .await
        .unwrap()
        .expect("overlay");
    assert_eq!(event.event_type(), Some(EventType::Appointment));

    // Wait for the tombstone cycle: the facts must be trashed and the event
    // must stop surfacing in Upcoming.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let empty = kg
            .get_facts_by_subject(devansh, 100)
            .await
            .unwrap()
            .is_empty();
        if empty {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "deleted event facts were never trashed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let upcoming = kg.render_upcoming_section(devansh, 30, 10).await.unwrap();
    assert!(
        !upcoming.contains("Dentist"),
        "deleted event must not surface in Upcoming: {upcoming}"
    );

    // The event's secondary facts (located_in) are trashed too, and the
    // events-subsystem overlay is gone with the fact (the `events.fact_id`
    // FK cascades), so no phantom event can keep surfacing.
    let dentist = entity_id(&kg, "Dentist").await;
    if let Some(id) = dentist {
        assert!(
            kg.get_facts_by_subject(id, 100).await.unwrap().is_empty(),
            "secondary facts of the deleted event must be trashed"
        );
    }
    assert!(
        kg.get_event_by_fact(has_event_fact.id)
            .await
            .unwrap()
            .is_none(),
        "deleted event's overlay must be cascade-removed"
    );

    // The trashed facts are recoverable from trash (30-day expiry).
    assert!(
        !kg.list_trash(100, 0).await.unwrap().is_empty(),
        "deleted event facts must land in trash"
    );

    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Issue #314: failure-safe sync-token advance
// ---------------------------------------------------------------------------

/// Delegating connector that fails the first `extract()` call, simulating a
/// transient extraction failure *after* `sync` already staged the changed
/// events. Every other operation — including the cursor adoption in
/// `on_cycle_succeeded` — delegates to the inner calendar connector, so the
/// wrapper only injects the failure (issue #314).
struct FailFirstExtractConnector {
    inner: Arc<dyn Connector>,
    /// Set once the injected extract failure has fired, so the test can wait
    /// for the failing cycle instead of racing the supervisor's first
    /// automatic cycle.
    failed_once: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Connector for FailFirstExtractConnector {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn connector_type(&self) -> ConnectorType {
        self.inner.connector_type()
    }
    fn mode(&self) -> ConnectorMode {
        self.inner.mode()
    }
    fn config_schema(&self) -> serde_json::Value {
        self.inner.config_schema()
    }
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.inner.authenticate().await
    }
    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        self.inner.health().await
    }
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        self.inner.sync(options).await
    }
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        if !self.failed_once.swap(true, Ordering::SeqCst) {
            return Err(ConnectorError::Parse(
                "injected transient extract failure".to_string(),
            ));
        }
        self.inner.extract().await
    }
    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        self.inner.extract_deletions().await
    }
    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        self.inner.acknowledge_deletions(deleted).await
    }
    async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
        self.inner.on_cycle_succeeded(new_cursor).await;
    }
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        self.inner.act(action).await
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        self.inner.forget().await
    }
}

/// Issue #314: a cycle that fails *after* `sync` (extract error) must not
/// lose the staged changed events. The in-memory sync-token may only advance
/// once the supervisor persisted the new cursor, so the next in-process cycle
/// re-syncs from the last confirmed cursor and re-processes the failed
/// window.
#[tokio::test]
async fn failed_extract_cycle_reprocesses_staged_events_on_next_cycle() {
    let (kg, _dir) = init_kg().await;
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/personal/", server.uri());

    // Health probe (Online) — PROPFIND resourcetype for every cycle.
    Mock::given(wiremock::matchers::method("PROPFIND"))
        .and(wiremock::matchers::path("/cal/personal/"))
        .respond_with(
            ResponseTemplate::new(207).set_body_string(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
            ),
        )
        .mount(&server)
        .await;

    // Window 1 (no cursor): event A + token-1. Mounted twice — the failed
    // first cycle and the retry cycle both re-sync from "no cursor".
    for _ in 0..2 {
        mount_sync(
            &server,
            "/cal/personal/",
            None,
            sync_body("token-1", &[("/cal/a.ics", ICAL_EVENT)], &[]),
        )
        .await;
    }
    // Window 2 (token-1): event B + token-2. Only reachable after the
    // connector adopted token-1 — a premature in-memory advance (the bug)
    // would fetch this window instead of re-processing event A's window.
    mount_sync(
        &server,
        "/cal/personal/",
        Some("token-1"),
        sync_body("token-2", &[("/cal/b.ics", ICAL_RECURRING)], &[]),
    )
    .await;

    let kg = Arc::new(kg);
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "calendar-personal".to_string(),
            backend: "caldav-failing".to_string(),
            display_name: "Calendar".to_string(),
            config_json: app_password_config(&cal_url).to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();

    let failed_once = Arc::new(AtomicBool::new(false));
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Calendar,
            "caldav-failing",
            FnConnectorFactory::new({
                let failed_once = Arc::clone(&failed_once);
                move |config, ctx| {
                    let inner = CalendarConnectorFactory.create(config, ctx)?;
                    Ok(Arc::new(FailFirstExtractConnector {
                        inner,
                        failed_once: Arc::clone(&failed_once),
                    }) as Arc<dyn Connector>)
                }
            }),
        )
        .unwrap();
    let (_shutdown_tx, rx) = tokio::sync::watch::channel(false);
    // A deliberately slow backoff keeps the runner from auto-retrying the
    // failed cycle before the test's manual trigger arrives (the trigger
    // preempts the backoff sleep) — the test owns the cycle sequencing from
    // here on.
    let config = SupervisorConfig {
        max_failures: 5,
        base_backoff: Duration::from_secs(30),
        max_backoff: Duration::from_secs(30),
    };
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), config, rx)
        .with_secret_store(store_with_app_password().await);

    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the supervisor's first automatic cycle to fail at `extract`.
    // The runner is serial, so once the flag fires the failing cycle is over
    // and the retry trigger below runs a fresh cycle.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while !failed_once.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "injected extract failure never fired"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Retry cycle: must re-sync from the last confirmed cursor (none) and
    // re-process event A's window.
    let outcome = supervisor
        .trigger_sync_by_slug("calendar-personal", SyncOptions::default())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        TriggerOutcome::Ok {
            fetched: 1,
            new_cursor: Some("token-1".to_string()),
        }
    );

    // The failed window's facts landed in the knowledge graph.
    let event = entity_id(&kg, "Trip to Rome")
        .await
        .expect("event from the failed window must be re-processed");
    assert!(
        !kg.get_facts_by_subject(event, 100)
            .await
            .unwrap()
            .is_empty(),
        "the re-processed event's located_in fact must be inserted"
    );
    assert!(
        entity_id(&kg, "Rome").await.is_some(),
        "the located_in object (Rome) must resolve"
    );

    // The next cycle must be incremental from the adopted cursor (token-1):
    // it fetches only window 2 (event B).
    let outcome = supervisor
        .trigger_sync_by_slug("calendar-personal", SyncOptions::default())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        TriggerOutcome::Ok {
            fetched: 1,
            new_cursor: Some("token-2".to_string()),
        }
    );
    let row = kg
        .get_connector(row.id)
        .await
        .unwrap()
        .expect("connector row");
    assert_eq!(row.sync_cursor.as_deref(), Some("token-2"));

    supervisor.shutdown().await;
}
