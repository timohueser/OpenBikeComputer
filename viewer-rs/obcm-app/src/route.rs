//! Routes — the loadable rides shown in the Route menu.
//!
//! A route is described to the UI by a [`RouteSummary`] (name + totals + bbox +
//! start), defined by the [`obcm_route`] format crate. The **catalog** of summaries is
//! produced by the host — the simulator scans a folder of `.obcr` files, the firmware
//! scans the SD card — and handed to [`App::set_routes`](crate::App::set_routes); the
//! app owns a copy and the screens read it through [`Ctx`](crate::screen::Ctx) /
//! [`Render`](crate::screen::Render). The heavy route *geometry* (the polyline the Map
//! draws) stays host-owned and is streamed on demand through an
//! [`obcm_route::RouteReader`]; only the one active route is opened at a time.
//!
//! [`Activity::active_route`](crate::Activity::active_route) indexes into the catalog.

pub use obcm_route::RouteSummary;

/// Maximum routes the resident menu catalog holds. Sized for a comfortable SD card of
/// rides; each summary is ~80 bytes, so the cap costs a few KB of static RAM.
pub const MAX_ROUTES: usize = 64;

/// The app's resident route catalog: the summaries the Route menu lists and
/// [`Activity::active_route`](crate::Activity::active_route) indexes.
pub type Catalog = heapless::Vec<RouteSummary, MAX_ROUTES>;
