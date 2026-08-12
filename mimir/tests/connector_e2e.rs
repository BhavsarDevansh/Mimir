//! End-to-end `mimir connector` cycle against a real in-process daemon.
//!
//! Uses the `gmail/test` mock connector (registered in the daemon via the
//! `mock-connector` feature, enabled in this crate's dev-dependencies) so
//! the full add → status → resume → sync → pause → resume → remove cycle
//! runs without any external service — the acceptance criterion of
//! issue #204.
//!
//! The fact-ingestion tests (T1 / issue #206) configure the mock's `facts`
//! knob and verify the full sync → `normalize_and_insert` → KB-query
//! pipeline through the real CLI + daemon: facts land with
//! `source_type=Connector`, provenance tied to the connector instance,
//! confidence from the connector reliability score, and the corroboration
//! path (a second independent instance boosts confidence; a plain re-sync
//! is a re-statement no-op).

mod common;

use common::TestDaemon;

/// Canned facts for the `gmail/test` mock backend: two literal facts with
/// explicit raw references and a static cursor so sync progress is
/// observable through `connector status`.
const FACTS_CONFIG: &str = r#"{
    "cursor": "v1",
    "facts": [
        {
            "subject": "Alice Mock",
            "subject_type": "Person",
            "relationship_type": "works_at",
            "object": "Acme",
            "raw_reference": "m-1"
        },
        {
            "subject": "Bob Mock",
            "subject_type": "Person",
            "relationship_type": "lives_in",
            "object": "London",
            "raw_reference": "m-2"
        }
    ]
}"#;

/// A single canned fact claiming `Alice Mock works_at Acme` with the given
/// raw reference, used to exercise corroboration across two instances.
fn alice_fact_config(raw_reference: &str) -> String {
    serde_json::json!({
        "facts": [{
            "subject": "Alice Mock",
            "subject_type": "Person",
            "relationship_type": "works_at",
            "object": "Acme",
            "raw_reference": raw_reference,
        }]
    })
    .to_string()
}

/// Assert the fact's confidence is within `tolerance` of `expected`.
fn assert_confidence(fact: &serde_json::Value, expected: f64) {
    let confidence = fact["confidence"]
        .as_f64()
        .unwrap_or_else(|| panic!("fact has no numeric confidence: {fact}"));
    assert!(
        (confidence - expected).abs() < 0.001,
        "expected confidence ≈ {expected}, got {confidence} (fact: {fact})"
    );
}

/// Assert `detail` (a `kb show --json` response) carries a `Connector` source
/// tied to `instance_id` with `raw_reference`.
fn assert_connector_source(detail: &serde_json::Value, instance_id: i64, raw_reference: &str) {
    let sources = detail["sources"]
        .as_array()
        .unwrap_or_else(|| panic!("kb show response has no sources array: {detail}"));
    assert!(
        sources.iter().any(|s| {
            s["source_type"] == "Connector"
                && s["connector_instance_id"] == instance_id
                && s["raw_reference"] == raw_reference
        }),
        "no Connector source with instance {instance_id} and raw_reference {raw_reference:?} in: {sources:?}"
    );
}

#[test]
fn connector_full_lifecycle_cycle() {
    let daemon = TestDaemon::start();

    // add → the instance starts in Setup/Unauthenticated.
    let created = daemon.run_cli_json(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo",
        "--name",
        "Demo",
        "--json",
    ]);
    assert_eq!(created["slug"], "demo");
    assert_eq!(created["status"], "setup");
    assert_eq!(created["auth_state"], "unauthenticated");
    let id = created["id"].as_i64().unwrap();

    // status shows the instance.
    let shown = daemon.run_cli_json(&["connector", "status", "demo", "--json"]);
    assert_eq!(shown["id"], id);
    assert_eq!(shown["status"], "setup");

    // auth completes the credential ingest on the existing instance (no
    // remove + re-add), flipping auth_state to authenticated.
    let authed = daemon.run_cli_json(&[
        "connector",
        "auth",
        "demo",
        "--token",
        "test-token",
        "--json",
    ]);
    assert_eq!(authed["id"], id);
    assert_eq!(authed["auth_state"], "authenticated");
    let shown = daemon.run_cli_json(&["connector", "status", "demo", "--json"]);
    assert_eq!(shown["auth_state"], "authenticated");

    // list includes it.
    let listed = daemon.run_cli_json(&["connector", "list", "--json"]);
    assert!(
        listed["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["slug"] == "demo"),
        "list should include demo:\n{listed}"
    );

    // resume activates the runner.
    let resumed = daemon.run_cli_json(&["connector", "resume", "demo", "--json"]);
    assert_eq!(resumed["status"], "active");

    // sync runs a manual cycle against the mock backend.
    let synced = daemon.run_cli_json(&["connector", "sync", "demo", "--json"]);
    assert_eq!(synced["status"], "ok");
    assert_eq!(synced["fetched"], 0);

    // pause stops the runner; resume re-activates it.
    let paused = daemon.run_cli_json(&["connector", "pause", "demo", "--json"]);
    assert_eq!(paused["status"], "paused");
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "resume", "demo"]);
    assert!(
        status.success(),
        "second resume failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("resumed"),
        "expected resume summary:\n{stdout}"
    );

    // remove tears the instance down (provenance detached; no facts existed).
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "remove", "demo", "--yes"]);
    assert!(
        status.success(),
        "connector remove failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("removed"),
        "expected remove summary:\n{stdout}"
    );

    // forget is the cascade variant: add a second instance and forget it.
    let (stdout, stderr, status) = daemon.run_cli(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo2",
    ]);
    assert!(
        status.success(),
        "second connector add failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let forgotten = daemon.run_cli_json(&["connector", "forget", "demo2", "--yes", "--json"]);
    assert_eq!(forgotten["forgotten_count"], 0);

    daemon.stop();
}

/// T1 / #206: a mock connector configured with `facts` lands queryable facts
/// with `source_type=Connector`, provenance tied to the instance, and
/// confidence from the connector reliability score (Gmail = 0.85).
#[test]
fn mock_sync_ingests_facts_with_provenance_and_confidence() {
    let daemon = TestDaemon::start();

    let created = daemon.run_cli_json(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo",
        "--config-json",
        FACTS_CONFIG,
        "--json",
    ]);
    let id = created["id"].as_i64().unwrap();

    daemon.run_cli_json(&[
        "connector",
        "auth",
        "demo",
        "--token",
        "test-token",
        "--json",
    ]);
    daemon.run_cli_json(&["connector", "resume", "demo", "--json"]);

    // The manual sync stages both canned facts and persists the cursor.
    let synced = daemon.run_cli_json(&["connector", "sync", "demo", "--json"]);
    assert_eq!(synced["status"], "ok");
    assert_eq!(synced["fetched"], 2);
    let status = daemon.run_cli_json(&["connector", "status", "demo", "--json"]);
    assert_eq!(status["sync_cursor"], "v1");
    assert_eq!(status["item_count"], 2);
    assert!(
        status["last_sync_at"].is_string(),
        "last_sync_at should be set after a successful sync: {status}"
    );

    // Alice's fact is queryable with the Gmail reliability score (0.85).
    let alice = daemon.run_cli_json(&["kb", "query", "Alice Mock", "--json"]);
    assert_eq!(alice["total"], 1);
    let fact = &alice["facts"][0];
    assert_eq!(fact["predicate"], "works_at");
    assert_eq!(fact["object"], "Acme");
    assert_confidence(fact, 0.85);
    let fact_id = fact["id"].as_i64().unwrap();

    // Provenance: a Connector source tied to the instance + raw reference.
    let fact_id_str = fact_id.to_string();
    let detail = daemon.run_cli_json(&["kb", "show", &fact_id_str, "--json"]);
    assert_connector_source(&detail, id, "m-1");

    // Bob's fact landed too.
    let bob = daemon.run_cli_json(&["kb", "query", "Bob Mock", "--json"]);
    assert_eq!(bob["total"], 1);
    assert_eq!(bob["facts"][0]["object"], "London");

    daemon.stop();
}

/// T1 / #206: a second connector instance corroborating the same claim adds
/// an independent source and boosts confidence (+0.05, capped at 0.95);
/// a plain re-sync of the same instance is a re-statement no-op.
#[test]
fn second_connector_corroboration_boosts_confidence() {
    let daemon = TestDaemon::start();

    // First instance: Alice works_at Acme (raw reference m-1).
    let created = daemon.run_cli_json(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo",
        "--config-json",
        &alice_fact_config("m-1"),
        "--json",
    ]);
    let demo_id = created["id"].as_i64().unwrap();
    daemon.run_cli_json(&[
        "connector",
        "auth",
        "demo",
        "--token",
        "test-token",
        "--json",
    ]);
    daemon.run_cli_json(&["connector", "resume", "demo", "--json"]);
    let synced = daemon.run_cli_json(&["connector", "sync", "demo", "--json"]);
    assert_eq!(synced["fetched"], 1);

    let alice = daemon.run_cli_json(&["kb", "query", "Alice Mock", "--json"]);
    assert_eq!(alice["total"], 1);
    assert_confidence(&alice["facts"][0], 0.85);
    let fact_id = alice["facts"][0]["id"].as_i64().unwrap();

    // Second instance corroborates the same claim with independent provenance.
    let created2 = daemon.run_cli_json(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo2",
        "--config-json",
        &alice_fact_config("m-2"),
        "--json",
    ]);
    let demo2_id = created2["id"].as_i64().unwrap();
    daemon.run_cli_json(&[
        "connector",
        "auth",
        "demo2",
        "--token",
        "test-token",
        "--json",
    ]);
    daemon.run_cli_json(&["connector", "resume", "demo2", "--json"]);
    let synced2 = daemon.run_cli_json(&["connector", "sync", "demo2", "--json"]);
    assert_eq!(synced2["fetched"], 1);

    // Entity resolution merged the claims: still one fact row, boosted.
    let alice = daemon.run_cli_json(&["kb", "query", "Alice Mock", "--json"]);
    assert_eq!(alice["total"], 1);
    assert_eq!(alice["facts"][0]["id"], fact_id);
    assert_confidence(&alice["facts"][0], 0.90);

    // Two independent sources on the same fact, one per instance.
    let fact_id_str = fact_id.to_string();
    let detail = daemon.run_cli_json(&["kb", "show", &fact_id_str, "--json"]);
    let sources = detail["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_connector_source(&detail, demo_id, "m-1");
    assert_connector_source(&detail, demo2_id, "m-2");

    // A plain re-sync of the first instance is a re-statement, not a new
    // corroboration: no extra source, no further boost.
    daemon.run_cli_json(&["connector", "sync", "demo", "--json"]);
    let detail = daemon.run_cli_json(&["kb", "show", &fact_id_str, "--json"]);
    assert_eq!(detail["sources"].as_array().unwrap().len(), 2);
    let alice = daemon.run_cli_json(&["kb", "query", "Alice Mock", "--json"]);
    assert_confidence(&alice["facts"][0], 0.90);

    daemon.stop();
}
