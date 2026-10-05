//! Host timings of the route walks, to compare a read-side change against its base. Only public
//! API that both sides share is used, so the file runs unchanged on either. Run in release:
//!
//! `cargo test --release -p obc-route --test main walk_timing -- --ignored --nocapture`
//!
//! Wall time on a shared host varies by tens of percent between runs. For a steady comparison,
//! count instructions instead: set `WALK_TIMING_ROW` to one row's name and run the test binary
//! under `valgrind --tool=callgrind --toggle-collect='*median_us*'`.

use std::time::Instant;

use obc_formats::io::SliceSource;
use obc_formats::obcr::{RouteSourceKey, VisitDescriptor, VISIT_DESCRIPTOR_VERSION};
use obc_route::{Profile, RouteCache, RouteIndex, RouteReader};

use crate::common::convert;

/// A ~9 k-point route converted from a deterministic wandering track, with a visit descriptor
/// appended so the assistant preview walks its two spans.
fn route() -> Vec<u8> {
    let mut seed = 9u64;
    let mut next = move |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let (mut lat, mut lon, mut ele, mut heading) = (46.5f64, 8.3f64, 1_200.0f64, 0.0f64);
    let mut gpx = String::from("<gpx><trk><trkseg>\n");
    for _ in 0..9_000 {
        heading += next(81) as f64 - 40.0;
        lat += 0.0002 * heading.to_radians().cos();
        lon += 0.0002 * heading.to_radians().sin();
        ele += (next(63) as f64 - 30.0) * 0.5;
        gpx += &format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{ele:.1}</ele></trkpt>\n");
    }
    let mut bytes = convert("timing", &(gpx + "</trkseg></trk></gpx>\n"));
    let total = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
    let descriptor = VisitDescriptor {
        original: RouteSourceKey { store: [1; 16], object: 1, revision: 1 },
        original_anchors_m: [total / 4, total / 2, total * 3 / 4],
        accepted_anchors_m: [total / 4, total / 2, total * 3 / 4],
        target_id: 1,
        target_lon: 8_300_000,
        target_lat: 46_500_000,
        target_kind: 1,
    }
    .encode()
    .unwrap();
    let at = bytes.len() as u32;
    bytes[118] = VISIT_DESCRIPTOR_VERSION;
    bytes[120..124].copy_from_slice(&at.to_le_bytes());
    bytes[124..128].copy_from_slice(&(descriptor.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&descriptor);
    bytes
}

/// The median over passes of the mean time per call, in microseconds. Out of line, so callgrind
/// can collect exactly the timed calls.
#[inline(never)]
fn median_us(call: &mut dyn FnMut()) -> f64 {
    const PASSES: usize = 9;
    const CALLS: u32 = 30;
    let mut passes: Vec<f64> = (0..PASSES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..CALLS {
                call();
            }
            start.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS)
        })
        .collect();
    passes.sort_by(f64::total_cmp);
    passes[PASSES / 2]
}

#[test]
#[ignore = "a timing report, not a check"]
fn walk_timing() {
    let bytes = route();
    let src = SliceSource(&bytes);
    let index = RouteIndex::read(&src).unwrap();
    let cache = RouteCache::new();
    let total = index.total_distance_m;
    println!("route: {} points, {} chunks, {} m", index.point_count, index.chunks().len(), total);
    let only = std::env::var("WALK_TIMING_ROW").ok();
    let row = |label: &str, what: &str, call: &mut dyn FnMut()| {
        let name = format!("{label} {what}");
        if only.as_ref().is_none_or(|only| *only == name) {
            println!("{name:42} {:9.1} us", median_us(call));
        }
    };
    for cached in [false, true] {
        let route = if cached { RouteReader::new_cached(&index, &src, &cache) } else { RouteReader::new(&index, &src) };
        let label = if cached { "cached" } else { "uncached" };
        row(label, "profile+climbs", &mut || {
            let mut profile = Profile::EMPTY;
            std::hint::black_box(route.elevation_profile_and_climbs_into(&mut profile));
        });
        row(label, "preview_polyline<64>", &mut || _ = std::hint::black_box(route.preview_polyline::<64>()));
        row(label, "assistant_preview_polyline<64>", &mut || {
            _ = std::hint::black_box(route.assistant_preview_polyline::<64>().unwrap())
        });
        row(label, "visit_points_between", &mut || {
            let mut n = 0;
            route.visit_points_between(0, total, |points| n += points.len());
            std::hint::black_box(n);
        });
        row(label, "position_at x100", &mut || {
            for i in 0..100 {
                std::hint::black_box(route.position_at(total / 100 * i));
            }
        });
    }
}
