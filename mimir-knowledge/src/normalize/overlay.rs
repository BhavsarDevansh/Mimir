//! Entity-locations overlay: geocode + upsert background worker (Phase 3 S3).

use chrono::{DateTime, Utc};
use mimir_core::geocoder::Geocoder;

use crate::normalize::types::NormalizedLocation;
use crate::queries;
// ---------------------------------------------------------------------------
// Entity-locations overlay derivation
// ---------------------------------------------------------------------------

/// Derive and persist an `entity_locations` row from a [`NormalizedLocation`]
/// overlay on a freshly-inserted fact (Phase 3 S3 / #193).
///
/// Fills the missing geo half via the supplied
/// [`Geocoder`](mimir_core::geocoder::Geocoder) when only one side is known
/// (address -> coords via forward, coords -> address via reverse), then upserts
/// the location for the subject entity with the fact's temporal bounds and
/// `source_fact_id = fact_id`. Geocoder errors and no-match results are logged
/// and tolerated — the location is stored with whatever data it carries and the
/// pipeline never aborts on a geocode failure. A location with neither address
/// nor coords is a no-op.
///
/// This runs on the background [`location_overlay_worker`] (not the ingestion
/// caller's task) so a connector batch of location facts is not gated on the
/// geocoder's rate limit. The non-sensitive (inserted) path enqueues an
/// [`OverlayJob::Apply`]; the pending-confirmation path (issue #226) rebuilds
/// the overlay from `pending_location_meta` on [`confirm_fact`](crate::extract::confirm_fact)
/// and calls this directly — a single user-initiated action, so the
/// synchronous call is not a throughput concern.
///
/// Returns `true` when the location was persisted (or was a no-op with no geo
/// data), and `false` when the `entity_locations` upsert failed. Callers that
/// consume persisted overlay state — e.g. the confirm path deleting
/// `pending_location_meta` — must only consume it on `true` so a failed write
/// can be retried instead of losing the only location payload.
pub(crate) async fn apply_location_overlay(
    pool: &sqlx::SqlitePool,
    write_lock: &std::sync::Arc<tokio::sync::Mutex<()>>,
    apply: LocationOverlayApply,
) -> bool {
    let LocationOverlayApply {
        geocoder,
        entity_id,
        mut location,
        valid_from,
        valid_until,
        fact_id,
        place_anchor,
    } = apply;
    if !location.has_geo_data() {
        return true;
    }

    let has_coords = location.latitude.is_some() && location.longitude.is_some();

    if location.address.is_some() && !has_coords {
        if let Some(geocoder) = geocoder {
            let query = location.address.as_deref().unwrap_or("");
            match geocoder.forward(query).await {
                Ok(Some(result)) => {
                    location.latitude = Some(result.latitude);
                    location.longitude = Some(result.longitude);
                }
                Ok(None) => tracing::debug!(
                    "geocoder found no match for address {:?}; storing address-only location",
                    location.address
                ),
                Err(error) => tracing::warn!(
                    "forward geocode failed for location overlay (fact {fact_id}): {error}"
                ),
            }
        }
    } else if location.address.is_none() && has_coords {
        if let Some(geocoder) = geocoder {
            let lat = location.latitude.unwrap();
            let lng = location.longitude.unwrap();
            match geocoder.reverse(lat, lng).await {
                Ok(Some(result)) => location.address = Some(result.display_name),
                Ok(None) => tracing::debug!(
                    "geocoder found no place for coords ({lat}, {lng}); storing coords-only location"
                ),
                Err(error) => tracing::warn!(
                    "reverse geocode failed for location overlay (fact {fact_id}): {error}"
                ),
            }
        }
    }

    // Serialise the DB writes with ingestion callers (issue #236): hold the
    // knowledge-graph write lock across the upsert + place-anchor so the
    // worker cannot commit between an ingestion caller's read-then-write
    // transaction (which would stale-snapshot it with an immediate,
    // un-retriable `SQLITE_BUSY`). Geocoding above stays outside the lock so
    // the rate-limited network call does not block ingestion.
    let _write_guard = write_lock.lock().await;

    let upsert_ok = match queries::entity::upsert_location(
        pool,
        entity_id,
        location.location_type as i16,
        location.address.as_deref(),
        location.latitude,
        location.longitude,
        location.timezone.as_deref(),
        valid_from,
        valid_until,
        Some(fact_id),
    )
    .await
    {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!("failed to persist location overlay for fact {fact_id}: {error}");
            false
        }
    };

    // Anchor the place entity's own coordinates (Phase 3 C2 / #196). Only for
    // facts whose object is a Place (e.g. a `took_photo_at <place>` connector
    // fact). The coords are read from the now-fully-populated overlay (the
    // geocode step above fills the missing half), so a place is anchored with
    // its resolved coordinates even when the producer supplied only one half.
    // Idempotent — repeated photos at the same place keep a single
    // `Geographic` row instead of accumulating move-history rows.
    if let Some(place_id) = place_anchor {
        if let (Some(lat), Some(lng)) = (location.latitude, location.longitude) {
            if let Err(error) =
                queries::entity::ensure_place_coordinates(pool, place_id, lat, lng, Some(fact_id))
                    .await
            {
                tracing::warn!(
                    "failed to anchor place {place_id} coordinates (fact {fact_id}): {error}"
                );
            }
        } else {
            // No resolved coordinates to anchor (e.g. a future address-only
            // caller with no configured geocoder): the `Place` entity is still
            // created, but gets no `Geographic` row. Log so this skip is
            // traceable rather than silently no-op'ing like the geocode
            // branches above do on every "no data"/error outcome.
            tracing::debug!(
                "place {place_id} not anchored: no resolved coordinates for fact {fact_id}"
            );
        }
    }

    upsert_ok
}

// ---------------------------------------------------------------------------
// Entity-locations background worker
// ---------------------------------------------------------------------------

/// A unit of work for the location-overlay background worker.
///
/// `Apply` carries everything the worker needs to geocode + upsert a location
/// without touching the [`KnowledgeGraph`](crate::KnowledgeGraph) (a geocoder clone read at submit
/// time, an owned [`NormalizedLocation`], and the *inserted fact's* temporal
/// bounds). `Flush` is a barrier the worker signals once every prior `Apply`
/// job has completed, used by [`KnowledgeGraph::flush_location_overlays`](crate::KnowledgeGraph::flush_location_overlays) for
/// deterministic shutdown / tests.
pub(crate) enum OverlayJob {
    Apply(LocationOverlayApply),
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// Parameters for an [`OverlayJob::Apply`] — everything the worker needs to
/// geocode + upsert a location without touching the [`KnowledgeGraph`](crate::KnowledgeGraph) (Phase
/// 3 S3 / #193; `place_anchor` added Phase 3 C2 / #196). Bundling these into a
/// struct keeps the worker function's argument list small.
pub(crate) struct LocationOverlayApply {
    /// Geocoder clone read at submit time.
    pub geocoder: Option<std::sync::Arc<dyn Geocoder>>,
    /// Subject entity whose location is being recorded.
    pub entity_id: i32,
    /// The location overlay from the inserted fact.
    pub location: NormalizedLocation,
    /// Inserted fact's `valid_from`.
    pub valid_from: Option<DateTime<Utc>>,
    /// Inserted fact's `valid_until`.
    pub valid_until: Option<DateTime<Utc>>,
    /// Inserted fact's id (links the `entity_locations` row back to it).
    pub fact_id: i32,
    /// When the fact's object is a `Place` entity, its id — so the worker can
    /// anchor the place's own coordinates (Phase 3 C2 / #196). `None` for
    /// non-place facts (the owner-only overlay path).
    pub place_anchor: Option<i32>,
}

/// Spawn the single location-overlay background worker and return the sender
/// used to enqueue [`OverlayJob`]s.
///
/// The worker owns a clone of the pool and processes jobs strictly in
/// submission order (an unbounded FIFO channel), which preserves move /
/// supersession ordering both within a batch and across separate
/// [`normalize_and_insert`] calls. A single worker loses no geocode throughput
/// versus parallelism: the default Nominatim backend is rate-limited to
/// ~1 req/sec regardless, so serial processing is already on the throughput
/// floor. The returned sender is stored on [`KnowledgeGraph`](crate::KnowledgeGraph); dropping it
/// closes the channel and the worker exits cleanly.
pub(crate) fn start_location_overlay_worker(
    pool: sqlx::SqlitePool,
    write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
) -> tokio::sync::mpsc::UnboundedSender<OverlayJob> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(location_overlay_worker(rx, pool, write_lock));
    tx
}

/// Drain [`OverlayJob`]s in FIFO order, geocoding + upserting each location.
///
/// The DB-write half of each `Apply` job is performed under the shared
/// [`KnowledgeGraph::write_lock`](crate::KnowledgeGraph::write_lock) (issue #236) so it cannot interleave with
/// an ingestion caller's write transaction; the geocode half stays unlocked
/// to preserve off-thread throughput.
async fn location_overlay_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<OverlayJob>,
    pool: sqlx::SqlitePool,
    write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
) {
    while let Some(job) = rx.recv().await {
        match job {
            OverlayJob::Apply(apply) => {
                apply_location_overlay(&pool, &write_lock, apply).await;
            }
            OverlayJob::Flush(tx) => {
                let _ = tx.send(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
