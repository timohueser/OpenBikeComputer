//! Routes — the loadable rides shown in the Route menu.
//!
//! A route is described to the UI by a [`RouteSummary`] (name + totals + bbox +
//! start), defined by the [`obc_route`] format crate. The **catalog** of summaries is
//! produced by the host — the simulator scans a folder of `.obcr` files, the firmware
//! scans the SD card — and handed to [`App::set_routes`](crate::App::set_routes); the
//! app owns a copy and the screens read it through [`Ctx`](crate::screen::Ctx) /
//! [`Render`](crate::screen::Render). The heavy route *geometry* (the polyline the Map
//! draws) stays host-owned and is streamed on demand through an
//! [`obc_route::RouteReader`]; only the one active route is opened at a time.
//!
//! [`Activity::active_route`](crate::Activity::active_route) indexes into the catalog.

use obc_render::{OverlayChunk, RouteOverlaySource};
use obc_route::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

pub use obc_route::RouteSummary;

/// Maximum routes the resident menu catalog holds. Each summary is ~80 bytes, so the cap costs a
/// few KB of static RAM. The constrained `nrf-mem` profile trims it (epic #116 R4 — the on-device
/// router's ~14 KB of `.bss` statics squeezed the combined `ble` build's stack region): the cap
/// also sizes the board's parallel filename/id tables and the boot scan's stack-side catalog, so
/// each trimmed slot pays back roughly twice. 48 stays far past any sane card (the board's
/// `scan_routes` warns and lists the first 48 if a card somehow exceeds it); the 512 KB LM20
/// restores the generous cap.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_ROUTES: usize = 64;
#[cfg(feature = "nrf-mem")]
pub const MAX_ROUTES: usize = 48;

/// The app's resident route catalog: the summaries the Route menu lists and
/// [`Activity::active_route`](crate::Activity::active_route) indexes.
pub type Catalog = heapless::Vec<RouteSummary, MAX_ROUTES>;

/// The route-overlay seam adapter (issue #332): presents a [`RouteReader`] to the renderer as
/// [`obc_render::RouteOverlaySource`] — chunked `(lon, lat)` microdegree polylines with per-chunk
/// bbox + cumulative distance — so `obc-render` never depends on the OBCR format. A zero-cost
/// wrapper (the orphan rule forbids implementing the foreign trait on the foreign reader directly).
pub struct RouteOverlay<'a, 'b>(pub &'a RouteReader<'b>);

/// Decode chunk `k` into `(lon, lat)` pairs. Split out `#[inline(never)]` so the
/// `RoutePoint` decode scratch (~3 KB, the same buffer `draw_route` used to keep in its own
/// frame) lives in a frame that is **popped before** `visit` descends into the deep
/// stroke/fill path — the measured stack peak on the 256 KB DK must not grow.
#[inline(never)]
fn decode_lonlat(rr: &RouteReader, k: usize, out: &mut [(i32, i32); MAX_POINTS_PER_CHUNK]) -> Option<usize> {
    let mut pts = heapless::Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    rr.decode_chunk(k, &mut pts).ok()?;
    for (dst, p) in out.iter_mut().zip(pts.iter()) {
        *dst = (p.lon, p.lat);
    }
    Some(pts.len())
}

impl RouteOverlaySource for RouteOverlay<'_, '_> {
    fn chunk_count(&self) -> usize {
        self.0.chunks().len()
    }

    fn chunk(&self, k: usize) -> OverlayChunk {
        let cm = &self.0.chunks()[k];
        OverlayChunk { bbox: cm.bbox, cum_distance_m: cm.cum_distance_m }
    }

    fn total_distance_m(&self) -> u32 {
        self.0.total_distance_m
    }

    fn visit_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        // Stack, not heap (`no_std`): a 2 KB `(lon, lat)` staging array in this frame; the
        // `RoutePoint` decode scratch lives (and dies) in `decode_lonlat`'s frame. A failed
        // decode (flaky SD) skips `visit`, per the trait contract.
        let mut ll = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        if let Some(n) = decode_lonlat(self.0, k, &mut ll) {
            visit(&ll[..n]);
        }
    }
}
