# Connector Management (server) — `mimir-server::routes::connectors`

> **Phase:** 3 — Connectors (A1 / issue #202)
> **Status:** Implemented. Connector activation/pause/resume/OAuth and the
> `forget` cascade are A2 (#203); the `mimir connector …` CLI is A3 (#204);
> OAuth PKCE flow is A4 (#205).
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

Wires the `ConnectorRegistry` and `ConnectorSupervisor` into the daemon's
`AppState` and exposes the connector instance CRUD/status HTTP surface. This
is the first server-facing piece of the connector framework: everything below
(F2–F13, C1–C7) was library-only until A1.

## Startup wiring (`AppState::from_config_with_llm`)

After the knowledge graph and its geocoder are built, the daemon:

1. Constructs an empty `ConnectorRegistry` and registers the built-in
   factories, gated by forwarded cargo features on `mimir-server`
   (`photos` → `PhotosConnectorFactory` under backend `"local"`, `calendar`
   → `CalendarConnectorFactory` under `"caldav"`, `gmail` →
   `EmailConnectorFactory` under `"imap"`). The mock factory is registered
   only under `#[cfg(test)]` so a release daemon never advertises a test
   connector.
2. Constructs a `ConnectorSupervisor` over the registry and knowledge graph,
   subscribing to the daemon-wide `shutdown_tx` watch channel, and chains the
   builders: `with_secret_store(FileSecretStore)` (best-effort — a missing
   secrets dir disables credentials but does not abort startup),
   `with_geocoder` (the same `Arc<dyn Geocoder>` the KG holds), `with_user_identity(cfg.identity.name)`
   (C4 / #198), and `with_llm_backend(llm_client)` (C7 / #201 — enables the
   Email prose-extraction system-queue path).
3. Calls `supervisor.restore()` to spawn a runner for every `Active` instance;
   restore is best-effort and never aborts daemon startup.
4. Stores `connector_registry` and `connector_supervisor` on `AppState`
   behind `Arc`.

On `AppState::shutdown()` (called from the graceful-drain path after the
shared shutdown watch fires), the supervisor's `shutdown()` aborts every
runner and awaits its termination before the runtime tears down, persisting
the last completed sync cursor.

## Routes (`routes/connectors.rs`)

| Method | Path | Handler | Notes |
|--------|------|---------|------|
| `GET` | `/connectors` | `connectors_list_handler` | One `count_sources_by_connector` (`GROUP BY`) query for the whole list |
| `POST` | `/connectors` | `connector_add_handler` | Validates `(type, backend)` against the registry; atomic create-only insert (`409` on existing slug via the `slug UNIQUE` index); creates in `Setup` |
| `GET` | `/connectors/{id}` | `connector_show_handler` | `404` when missing |
| `DELETE` | `/connectors/{id}` | `connector_remove_handler` | `stop(id)`, delete the slug-keyed secret-store entry (idempotent), then `delete_connector`; `204` (`404` missing, `500` if credential deletion fails and the row is left intact) |

### Wire types (`mimir-api-types`)

`AddConnectorRequest`, `ConnectorResponse` (carries `item_count`,
`status`/`auth_state` as lowercase strings, RFC-3339 timestamps),
`ConnectorListResponse`. `mimir-api-types` stays decoupled from
`mimir-knowledge`, so the connector kind/status are strings, mapped to the
enums in the route layer (`parse_connector_type`).

## Knowledge-graph additions (`mimir-knowledge`)

- `KnowledgeGraph::count_sources_for_connector(id) -> i64` — the derived
  "items ingested" metric for a single instance (`SELECT COUNT(*) FROM sources
  WHERE connector_instance_id = ?`); used by the show route.
- `KnowledgeGraph::count_sources_by_connector() -> HashMap<i32, i64>` — every
  instance's count in one `GROUP BY connector_instance_id` query; used by the
  list route so item counts stay O(1) round-trips regardless of instance count.
- `KnowledgeGraph::delete_connector(id)` — nulls every
  `sources.connector_instance_id` referencing the row (the FK has no `ON
  DELETE` clause, so a raw `DELETE` would violate it), then deletes the row,
  in one transaction. Facts survive with degraded provenance; the full
  `forget` cascade is A2 / #203. Returns `ConnectorNotFound` when no row
  matches.

## Supervisor addition (`mimir-connectors`)

`ConnectorSupervisor::stop(id) -> bool` aborts a single *live* runner,
awaits its termination, and removes it from the handle map. A stale handle
whose task already finished naturally (e.g. an unauthenticated connector whose
runner exited at the auth handshake) is cleaned up but reports `false`, as do
never-spawned or already-stopped ids; only a genuinely running task reports
`true`. This is the per-instance counterpart of `shutdown()`; `DELETE` uses it
so a mid-cycle sync cannot write back to a vanishing row.

`ConnectorSupervisor::secret_store() -> Option<Arc<dyn SecretStore>>` exposes
the shared credential store (injected via `with_secret_store`) so the removal
route can delete a connector's slug-keyed secret. The `DELETE` route deletes
the secret *before* the row: a secret-deletion failure aborts the removal
(`500`) and leaves the instance intact, so the database and secret store are
never left in an ambiguous state and a later same-slug connector can never
load a deleted instance's stored credentials.

## Feature forwarding

`mimir-server` declares `photos`/`calendar`/`gmail` features (default = all
three) that forward to `mimir-connectors`, so the route layer can
`#[cfg(feature = "...")]`-gate each factory registration against the same
flag that compiles the backend module. Disabling a feature removes both the
backend and its daemon registration.

## Tests

- `mimir-server` integration tests (`routes/connectors.rs` via `oneshot`):
  add/list/show/remove round-trip, `409` on existing slug, `400` on
  unregistered backend, `400` on unknown connector type, a concurrent
  same-slug `POST` test verifying exactly one `201` and one `409`, `404` on
  deleting an unknown id, and a removal test verifying the slug-keyed
  secret-store entry is deleted (and cannot be loaded by a later same-slug
  connector).
- `mimir-knowledge`: `count_sources_for_connector` (zero for unknown),
  `count_sources_by_connector` (one `GROUP BY` query), `delete_connector`
  (removes row, `ConnectorNotFound` on missing), `create_connector`
  (atomic insert, `ConnectorSlugConflict` on a duplicate, concurrent same-slug
  creates yield one winner), and provenance-detach preservation (facts survive,
  FK nulled, `connector_type_id` retained).
- `mimir-connectors`: `ConnectorSupervisor::stop` aborts one live runner
  without affecting the others, returns `false` for an already-finished runner
  while cleaning up its stale handle, and reports no action on re-stop /
  unknown id.

## Out of scope (tracked)

- Activation / pause / resume / manual sync / OAuth token ingest routes + the
  `forget` cascade — A2 / #203.
- `mimir connector …` CLI plumbing — A3 / #204.
- OAuth PKCE loopback callback flow — A4 / #205.
