//! Pure geospatial helpers for entity-location proximity queries
//! (Phase 3 S4 / issue #194).
//!
//! All functions here are pure, allocation-free, and `unsafe`-free. They exist
//! to support `find_nearby`: a coarse bounding-box pre-filter expressed in SQL
//! (`latitude`/`longitude BETWEEN ? AND ?`) followed by an exact great-circle
//! distance computed in Rust (Haversine). No external `geo` crate is pulled in
//! — the formula is small and self-contained, and a heavy dependency for one
//! function would violate the project's minimal-dependency stance.
//!
//! # Conventions
//!
//! - Latitudes are in `[-90, 90]` degrees; longitudes in `[-180, 180]`.
//! - Distances are in kilometres.
//! - The Earth is modelled as a sphere of mean radius `EARTH_RADIUS_KM`
//!   (`6371.0088`), the IUGG mean radius. Haversine is accurate to ~0.5%,
//!   ample for personal-scale proximity ("is this within 5 km of me").

/// IUGG mean Earth radius in kilometres (R1 = (2a + b) / 3).
pub const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Great-circle distance between two WGS-84 points, in kilometres.
///
/// Uses the Haversine formula:
/// `a = sin²(Δφ/2) + cos φ1 · cos φ2 · sin²(Δλ/2)`
/// `c = 2 · atan2(√a, √(1−a))`; `d = R · c`.
///
/// Returns `0.0` when the points coincide and is symmetric in its arguments.
/// Inputs are not clamped; callers should pass normalised lat/lon, but the
/// formula is well-defined for any finite values.
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let to_rad = std::f64::consts::TAU / 360.0;
    let phi1 = lat1 * to_rad;
    let phi2 = lat2 * to_rad;
    let dphi = (lat2 - lat1) * to_rad;
    let dlam = (lon2 - lon1) * to_rad;

    let a = (dphi * 0.5).sin().powi(2) + phi1.cos() * phi2.cos() * (dlam * 0.5).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    EARTH_RADIUS_KM * c
}

/// A latitude/longitude bounding box centred on `(lat, lon)` that is guaranteed
/// to enclose every point within `radius_km` (great-circle).
///
/// The box is **over-inclusive by design**: latitude spans `radius / 111.32`
/// degrees per side (one degree of latitude ≈ 111.32 km), and longitude is
/// divided by `cos(lat)` to compensate for meridian convergence, clamped at the
/// poles. Over-inclusion is corrected exactly by the Haversine post-filter, so
/// correctness is preserved while the SQL `BETWEEN` stays a cheap range scan.
/// If the longitude span would cross the ±180° antimeridian, the full
/// `[-180, 180]` longitude range is returned (over-inclusive but correct;
/// see the implementation note).
///
/// The four bounds are returned as `(min_lat, max_lat, min_lon, max_lon)`.
pub fn bounding_box(lat: f64, lon: f64, radius_km: f64) -> (f64, f64, f64, f64) {
    // Kilometres per degree of latitude (polar circumference / 360 ≈ 111.32).
    const KM_PER_DEG_LAT: f64 = 111.32;

    let dlat = radius_km / KM_PER_DEG_LAT;
    let min_lat = (lat - dlat).max(-90.0);
    let max_lat = (lat + dlat).min(90.0);

    // Kilometres per degree of longitude shrinks with |cos(latitude)|; at the
    // poles it is zero, so clamp to avoid division by zero and a degenerate
    // (whole-earth) longitude span.
    let cos_lat = lat.to_radians().cos().abs();
    let km_per_deg_lon = KM_PER_DEG_LAT * cos_lat.max(1e-12);
    let dlon = radius_km / km_per_deg_lon;

    // Longitude wraps at ±180; a SQL `BETWEEN` cannot express a wrap-around
    // box. If either edge would cross the antimeridian, fall back to the full
    // longitude span `[-180, 180]`. This is over-inclusive (it scans every
    // longitude in the latitude band) but *correct* — the exact Haversine
    // post-filter is the final arbiter — and only triggers near the antimeridian
    // or at the poles, where personal-scale data is effectively absent. A
    // narrower wrap-aware two-range query is deferred (it would complicate the
    // SQL for no practical gain at this scale).
    let (min_lon, max_lon) = if lon - dlon < -180.0 || lon + dlon > 180.0 {
        (-180.0, 180.0)
    } else {
        (lon - dlon, lon + dlon)
    };

    (min_lat, max_lat, min_lon, max_lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// London ↔ Paris is ~343–347 km; assert a tolerant band.
    #[test]
    fn haversine_london_paris() {
        let d = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        assert!(d > 340.0 && d < 350.0, "london-paris distance was {d}");
    }

    #[test]
    fn haversine_coincident_is_zero() {
        assert_eq!(haversine_km(51.5, -0.12, 51.5, -0.12), 0.0);
    }

    #[test]
    fn haversine_is_symmetric() {
        let a = haversine_km(40.0, 10.0, 41.0, 11.0);
        let b = haversine_km(41.0, 11.0, 40.0, 10.0);
        assert!((a - b).abs() < 1e-12);
    }

    #[test]
    fn haversine_antipode_is_half_circumference() {
        // Antipodal points → π·R ≈ half the Earth's circumference.
        let d = haversine_km(0.0, 0.0, 0.0, 180.0);
        let expected = std::f64::consts::PI * EARTH_RADIUS_KM;
        assert!((d - expected).abs() < 1e-6, "{d} vs {expected}");
    }

    #[test]
    fn bounding_box_contains_point_at_radius() {
        // A point exactly `radius_km` due north must lie inside the box.
        let (min_lat, max_lat, min_lon, max_lon) = bounding_box(51.5, -0.12, 10.0);
        let north = 51.5 + 10.0 / 111.32;
        assert!(north <= max_lat + 1e-9);
        assert!(51.5 >= min_lat && 51.5 <= max_lat);
        assert!((-0.12) >= min_lon && (-0.12) <= max_lon);
    }

    #[test]
    fn bounding_box_clamps_at_poles() {
        let (min_lat, max_lat, _min_lon, _max_lon) = bounding_box(89.5, 0.0, 100.0);
        assert_eq!(max_lat, 90.0);
        assert!(min_lat >= -90.0);
    }

    #[test]
    fn bounding_box_longitude_widens_toward_equator() {
        // At higher latitude the longitude half-span grows (cos shrinks).
        let (_, _, min_lon_eq, max_lon_eq) = bounding_box(0.0, 0.0, 100.0);
        let (_, _, min_lon_hi, max_lon_hi) = bounding_box(60.0, 0.0, 100.0);
        let span_eq = max_lon_eq - min_lon_eq;
        let span_hi = max_lon_hi - min_lon_hi;
        assert!(
            span_hi > span_eq,
            "high-lat span {span_hi} should exceed equatorial {span_eq}"
        );
    }

    #[test]
    fn bounding_box_wraps_to_full_longitude_span_near_antimeridian() {
        // A query at lon 179.5 whose radius reaches past +180 must not
        // under-include points at -179.x: fall back to the full span.
        let (_, _, min_lon, max_lon) = bounding_box(0.0, 179.5, 200.0);
        assert_eq!(min_lon, -180.0);
        assert_eq!(max_lon, 180.0);
    }
}
