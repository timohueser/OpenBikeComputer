//! Breadcrumb bounds + decimation: the two tiers stay within their fixed caps no matter how
//! long the ride, the whole-ride start stays visible, and `clear` empties it.

use obc_app::Breadcrumb;

/// At lat 0, 1 µdeg of longitude ≈ 0.111 m, so stepping `lon` walks a measurable distance
/// while `cos(lat)` ≈ 1 keeps the maths simple.
const LAT0: i32 = 0;

#[test]
fn empty_then_cleared() {
    let mut bc = Breadcrumb::new();
    assert!(bc.is_empty());
    bc.push(0, LAT0);
    bc.push(1000, LAT0);
    assert!(!bc.is_empty());
    bc.clear();
    assert!(bc.is_empty());
    assert_eq!(bc.spine_iter().count(), 0);
    assert_eq!(bc.recent_iter().count(), 0);
}

#[test]
fn short_ride_is_all_recent_one_line() {
    let mut bc = Breadcrumb::new();
    // Five points ~22 m apart: the ring isn't full, so nothing has aged into the spine — the
    // whole short trail is full-resolution `recent`, and `points()` is just those 5 in order.
    for i in 0..5 {
        bc.push(i * 200, LAT0);
    }
    assert_eq!(bc.recent_iter().count(), 5);
    assert_eq!(bc.spine_iter().count(), 0, "nothing has aged out of the ring yet");
    assert_eq!(bc.points().count(), 5);
    assert_eq!(bc.points().next(), Some((0, LAT0)), "the ride start is the first point");
    assert_eq!(bc.points().last(), Some((800, LAT0)), "…and the latest fix is the last");
}

#[test]
fn long_ride_stays_bounded_and_keeps_the_start() {
    let mut bc = Breadcrumb::new();
    // ~30 000 points × ~5.5 m ≈ 165 km — far past both caps.
    for i in 0..30_000 {
        bc.push(i * 50, LAT0);
    }
    // Documented caps (breadcrumb.rs): recent 512, spine 768 — bounded regardless of length.
    let (recent, spine, all) = (bc.recent_iter().count(), bc.spine_iter().count(), bc.points().count());
    assert!(recent <= 512, "recent tier bounded: {recent}");
    assert!(spine <= 768, "spine tier bounded: {spine}");
    assert!(spine > 100, "old points have aged into the spine: {spine}");
    assert_eq!(all, recent + spine, "points() chains the two disjoint tiers — no overlap");
    // The whole-ride start survives (it aged into the spine, whose index 0 is never thinned
    // away); the chained trail ends at the latest fix.
    assert_eq!(bc.points().next(), Some((0, LAT0)));
    assert_eq!(bc.points().last(), Some((29_999 * 50, LAT0)));
}
