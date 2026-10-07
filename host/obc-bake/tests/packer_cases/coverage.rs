//! Integration: the raw GEOS coverage wrapper (`geom::coverage_api`) that the semantic tiers
//! simplify and validate with, swept over degenerate inputs and under rayon.

use obc_draw::geom::{coverage_is_valid, coverage_simplify_vw, Geom};

fn probe_poly(ring: &[(f64, f64)]) -> Geom {
    let mut exterior = ring.to_vec();
    if exterior.first() != exterior.last() {
        exterior.push(exterior[0]);
    }
    Geom::Polygon { exterior, interiors: vec![] }
}

/// An axis-aligned box in raw degrees.
fn probe_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Geom {
    probe_poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// Every early-return / degenerate path of `geom::coverage_api`, back to back and repeatedly, so a
/// missing free or a double free shows as a crash or unbounded RSS growth under a leak checker.
/// `PROBE_ROUNDS` turns it up for a deliberate leak hunt.
#[test]
fn the_coverage_api_survives_degenerate_inputs() {
    let square = probe_rect(0.0, 0.0, 1.0, 1.0);
    let line = Geom::Line(vec![(0.0, 0.0), (1.0, 1.0)]);
    let empty = Geom::Empty;
    let stub = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], interiors: vec![] };
    let two_pt = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0)], interiors: vec![] };
    let bowtie = probe_poly(&[(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)]);
    let nan = probe_poly(&[(0.0, 0.0), (f64::NAN, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let inf = probe_poly(&[(0.0, 0.0), (f64::INFINITY, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let holed = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)],
        // an unclosed hole ring: `build_ring` closes it rather than refusing
        interiors: vec![vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]],
    };
    let bad_hole = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)],
        interiors: vec![vec![(1.0, 1.0), (2.0, 1.0)]], // too short: build_ring returns null
    };
    let multi = Geom::Multi(vec![probe_rect(0.0, 0.0, 1.0, 1.0), probe_rect(2.0, 0.0, 3.0, 1.0)]);
    let overlapping = [&square, &probe_rect(0.5, 0.5, 1.5, 1.5)];

    let rounds = std::env::var("PROBE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200u32);
    for _ in 0..rounds {
        assert!(coverage_simplify_vw(&[], 0.1, false).is_none(), "empty input");
        assert!(!coverage_is_valid(&[], 0.0), "empty input is not a valid coverage");

        for g in [&line, &empty, &stub, &two_pt, &nan, &inf, &bad_hole, &multi] {
            // Each of these makes `build_polygon` (or GEOS) refuse; the whole call must bail out
            // with everything freed rather than crash.
            let _ = coverage_simplify_vw(&[g], 0.1, false);
            let _ = coverage_is_valid(&[g], 0.0);
            let _ = coverage_simplify_vw(&[&square, g], 0.1, false);
            let _ = coverage_is_valid(&[&square, g], 0.0);
        }
        // Invalid-but-buildable geometry: GEOS accepts the polygon, then throws inside the
        // coverage algorithm.
        let _ = coverage_simplify_vw(&[&bowtie], 0.1, false);
        let _ = coverage_is_valid(&[&bowtie, &square], 0.0);
        // A ring GEOS has to close for us.
        assert!(coverage_simplify_vw(&[&holed], 0.1, false).is_some(), "an unclosed hole ring is closed, not lost");
        // Not a coverage at all: overlapping members.
        assert!(!coverage_is_valid(&overlapping, 0.0), "overlaps are not a valid coverage");
        let _ = coverage_simplify_vw(&overlapping, 0.1, false);
        // The happy path, so the loop also exercises the success free.
        let ok = coverage_simplify_vw(&[&square, &probe_rect(1.0, 0.0, 2.0, 1.0)], 0.01, false);
        assert_eq!(ok.expect("a real coverage simplifies").len(), 2, "element count and order are preserved");
        assert!(coverage_is_valid(&[&square, &probe_rect(1.0, 0.0, 2.0, 1.0)], 0.0));
    }
}

/// The same wrapper under rayon: many threads, each creating and destroying its own GEOS context.
#[test]
fn the_coverage_api_is_thread_safe_under_rayon() {
    use rayon::prelude::*;
    let out: Vec<usize> = (0..2000u32)
        .into_par_iter()
        .map(|i| {
            let x = i as f64;
            let a = probe_rect(x, 0.0, x + 1.0, 1.0);
            let b = probe_rect(x + 1.0, 0.0, x + 2.0, 1.0);
            let bad = Geom::Line(vec![(x, 0.0), (x + 1.0, 1.0)]);
            let _ = coverage_simplify_vw(&[&a, &bad], 0.01, false);
            let _ = coverage_is_valid(&[&a, &b], 0.0);
            coverage_simplify_vw(&[&a, &b], 0.01, false).map(|v| v.len()).unwrap_or(0)
        })
        .collect();
    assert!(out.iter().all(|&n| n == 2), "every parallel call returned both elements");
}
