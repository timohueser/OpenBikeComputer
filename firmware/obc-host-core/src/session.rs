//! The parsed active-route session — a host holds one for the session's lifetime and reparses the
//! ~6.7 KB [`RouteIndex`] only when the active route's *bytes* actually change (a selection change,
//! a re-route rewrite, an import). The web demo used to reparse every frame; this is the
//! acceptance-criterion fix (#801): no per-frame route-index parse.

use obc_app::App;
use obc_route::{RouteIndex, RouteReader};

use crate::RouteRepository;

/// The resident parse of the active route's `RouteIndex`, kept across frames. Its cache identity
/// (the [`RouteIndex`]'s non-persisted `identity`, #799) rides with the parse, so a settled Map view
/// re-uses one parse indefinitely.
///
/// The ~8 KB `RouteIndex` is **boxed**: a [`HostLoop`](crate::HostLoop) embeds this session, and a
/// host that holds its loop as a stack value (the deep sim tour test) must not carry a resident
/// 8 KB inline field down the render call chain. The heap slot keeps the loop pointer-small.
#[derive(Default)]
pub struct ActiveRouteSession {
    index: Option<Box<RouteIndex>>,
}

impl ActiveRouteSession {
    /// A session with nothing parsed yet.
    pub const fn new() -> Self {
        ActiveRouteSession { index: None }
    }

    /// Reparse the active route's index **only when the store just re-read its bytes** (`changed`,
    /// the [`RouteRepository::sync_active`] return) — otherwise keep the resident parse. A cleared
    /// active source drops the index. Call right after `sync_active`.
    pub fn reparse(&mut self, changed: bool, routes: &dyn RouteRepository) {
        if !changed {
            return;
        }
        self.index = routes.active_source().and_then(|s| RouteIndex::read(&s).ok()).map(Box::new);
    }

    /// The resident parse, for building a [`RouteReader`] over the store's active bytes.
    pub fn index(&self) -> Option<&RouteIndex> {
        self.index.as_deref()
    }
}

/// Fill an open Route overview's decimated shape preview (#678 rework 3) — the once-per-entry cue
/// every frame-stepped host shares. `nav_preview_missing` is false again the moment the copy lands,
/// so this is a per-frame no-op otherwise.
pub fn fill_nav_preview(app: &mut App, route: Option<&RouteReader>) {
    if app.nav_preview_missing() {
        if let Some(r) = route {
            app.set_nav_preview(&r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>());
        }
    }
}
